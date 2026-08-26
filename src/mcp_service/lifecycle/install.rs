//! Installing and reconciling the service: write the definition, register the
//! scheduled task, and prove the two agree.

use super::*;

impl<S: SchedulerAdapter, R: RuntimeControl> LifecycleService<S, R> {
    pub(super) fn install_locked(
        &self,
        options: &InstallOptions,
    ) -> Result<MutationOutput, AppError> {
        if options.max_active == 0 || options.default_timeout_ms == 0 {
            return Err(AppError::invalid_argument(
                "--max-active and --default-timeout-ms must be positive",
            ));
        }
        if options.options.limit == Some(0) {
            return Err(AppError::invalid_argument("--limit must be positive"));
        }
        self.store.ensure_directories()?;
        let cwd = normalize_absolute_path(
            &std::env::current_dir().map_err(|source| AppError::cwd(PathBuf::from("."), source))?,
            None,
        )?;
        let executable = current_executable_path()?;
        let config = ConfigContext::load()?;
        let config_dir = normalize_absolute_path(&config.paths().config_dir, Some(&cwd))?;
        let user_sid = current_user_sid()?;
        let expected_task_path = task_path(&user_sid);
        let current_document = self.store.read_current();
        let current = match current_document {
            Document::Missing => None,
            Document::Valid(current) => Some(current),
            Document::Invalid(message) => {
                return Err(AppError::external("MCP_SERVICE_STATE_INVALID", message));
            }
            Document::UnsupportedVersion(version) => {
                return Err(AppError::external(
                    "MCP_SERVICE_STATE_INVALID",
                    format!("unsupported managed MCP schema version {version}"),
                ));
            }
        };
        // Reject corrupt or newer runtime documents before changing the task or
        // current pointer. A valid stale runtime remains diagnostic input.
        let _ = self.store.read_runtime().valid()?;
        let observation = self.scheduler.inspect(&expected_task_path)?;
        let was_installed = !matches!(observation, TaskObservation::Missing);
        if matches!(observation, TaskObservation::Foreign { .. }) {
            return Err(AppError::external(
                "MCP_SERVICE_TASK_CONFLICT",
                format!("Task Scheduler path '{expected_task_path}' is not owned by AIHelper"),
            ));
        }

        let task_definition = match &observation {
            TaskObservation::Owned(observed) => {
                Some(self.require_marker_definition(&observed.spec.marker)?)
            }
            TaskObservation::Missing | TaskObservation::Foreign { .. } => None,
        };
        if let (Some(task), Some(current)) = (&task_definition, &current)
            && task.service_id != current.service_id
        {
            return Err(AppError::external(
                "MCP_SERVICE_INSTALLATION_CONFLICT",
                "registered task and current pointer identify different services",
            ));
        }
        let pointer_definition = if task_definition.is_none() {
            current
                .as_ref()
                .map(|pointer| self.require_pointer_definition(pointer))
                .transpose()?
        } else {
            None
        };
        let existing = task_definition.as_ref().or(pointer_definition.as_ref());
        if let Some(existing) = existing
            && !paths_equal(&existing.executable_path, &executable)
        {
            return Err(AppError::external(
                "MCP_SERVICE_INSTALLATION_CONFLICT",
                format!(
                    "managed MCP service belongs to executable '{}'",
                    existing.executable_path.display()
                ),
            ));
        }

        let service_id = existing
            .map(|definition| definition.service_id)
            .unwrap_or_else(Uuid::new_v4);
        let endpoint = ServiceEndpoint::loopback(options.port)?;
        let server = ServerDefinition {
            limit: options.options.limit,
            max_active: options.max_active,
            default_timeout_ms: options.default_timeout_ms,
        };
        let configuration_id = existing
            .filter(|existing| {
                configuration_matches(
                    existing,
                    &user_sid,
                    &executable,
                    &cwd,
                    &config_dir,
                    &self.store.paths().runtime,
                    &self.store.paths().instance_lock,
                    &endpoint,
                    &server,
                )
            })
            .map(|existing| existing.configuration_id)
            .unwrap_or_else(Uuid::new_v4);
        let definition = ServiceDefinition {
            schema_version: SCHEMA_VERSION,
            task_spec_version: TASK_SPEC_VERSION,
            service_id,
            configuration_id,
            user_sid: user_sid.clone(),
            executable_path: executable.clone(),
            working_directory: cwd.clone(),
            config_directory: config_dir,
            runtime_state_path: self.store.paths().runtime.clone(),
            instance_lock_path: self.store.paths().instance_lock.clone(),
            expected_version: env!("CARGO_PKG_VERSION").to_owned(),
            endpoint,
            server,
        };
        definition.validate()?;
        let definition_path = self.store.paths().definition(configuration_id);
        let definition_created = self
            .store
            .write_immutable_definition(&definition_path, &definition)?;
        let marker = TaskMarker::from_definition(&definition, definition_path.clone());
        let desired_task = DesiredTaskSpec::canonical(
            expected_task_path.clone(),
            user_sid,
            marker,
            executable,
            cwd,
        );
        let task_changed = match observation {
            TaskObservation::Missing => {
                let observed = self.scheduler.register(&desired_task)?;
                require_no_drift(&desired_task, &observed)?;
                true
            }
            TaskObservation::Owned(observed) => {
                if semantic_drift(&desired_task, &observed.spec).is_empty() {
                    false
                } else {
                    let observed = self.scheduler.register(&desired_task)?;
                    require_no_drift(&desired_task, &observed)?;
                    true
                }
            }
            TaskObservation::Foreign { .. } => unreachable!(),
        };
        let pointer = CurrentPointer {
            schema_version: SCHEMA_VERSION,
            service_id,
            configuration_id,
            definition_path: definition_path.clone(),
            task_path: expected_task_path.clone(),
        };
        let pointer_changed = current.as_ref() != Some(&pointer);
        if pointer_changed {
            self.store.write_current(&pointer)?;
        }
        self.cleanup_definitions(&pointer)?;

        let mut changed = definition_created || task_changed || pointer_changed;
        let runtime = if options.no_start {
            self.observed_runtime_status(&definition)
        } else {
            if let Some(existing) = existing
                && existing.configuration_id != definition.configuration_id
            {
                self.prepare_configuration_replacement(existing)?;
            }
            let start = self.start_definition(&definition, &desired_task)?;
            changed |= start.changed;
            start.runtime
        };
        Ok(MutationOutput {
            command: "mcp.service.install".to_owned(),
            schema_version: SCHEMA_VERSION,
            changed,
            action: if !was_installed {
                "installed"
            } else if changed {
                "updated"
            } else {
                "unchanged"
            }
            .to_owned(),
            service_id,
            configuration_id,
            task_path: expected_task_path,
            endpoint: definition.endpoint.mcp_url,
            registration: "installed".to_owned(),
            runtime,
        })
    }

    pub(super) fn prepare_configuration_replacement(
        &self,
        old_definition: &ServiceDefinition,
    ) -> Result<(), AppError> {
        let Some(runtime) = self.store.read_runtime().valid()? else {
            return Ok(());
        };
        if runtime.configuration_id == old_definition.configuration_id
            && matches!(
                runtime.phase,
                RuntimePhase::Starting | RuntimePhase::Ready | RuntimePhase::Stopping
            )
        {
            if !self.instance_lease_is_occupied() {
                return Ok(());
            }
            let readiness = self
                .readiness
                .inspect(old_definition, Some(&runtime), false);
            if readiness.status != ReadinessStatus::Ready {
                return Err(AppError::external(
                    "MCP_SERVICE_RESTART_REQUIRED",
                    "old managed MCP instance cannot be identified for controlled replacement",
                ));
            }
            match self.readiness.shutdown(old_definition, runtime.instance_id) {
                ShutdownReceipt::Accepted => {}
                ShutdownReceipt::Failed { detail } => {
                    return Err(AppError::external("MCP_SERVICE_RESTART_REQUIRED", detail));
                }
            }
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                match lock::try_acquire(&self.store.paths().instance_lock)? {
                    Some(lease) => {
                        drop(lease);
                        return Ok(());
                    }
                    None if Instant::now() < deadline => thread::sleep(self.poll_interval),
                    None => {
                        return Err(AppError::external(
                            "MCP_SERVICE_RESTART_REQUIRED",
                            "old managed MCP instance did not release the instance lease",
                        ));
                    }
                }
            }
        }
        Ok(())
    }
}
