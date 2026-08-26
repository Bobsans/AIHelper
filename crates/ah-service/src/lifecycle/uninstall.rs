//! Removing the service: prove it inactive, then delete only what this
//! installation owns.

use super::*;

impl<S: SchedulerAdapter, R: RuntimeControl> LifecycleService<S, R> {
    pub(crate) fn uninstall_locked(&self) -> Result<UninstallOutput, AppError> {
        let user_sid = current_user_sid()?;
        let expected_task_path = task_path(&user_sid);
        let observation = self.scheduler.inspect(&expected_task_path)?;
        if matches!(observation, TaskObservation::Foreign { .. }) {
            return Err(AppError::external(
                "MCP_SERVICE_TASK_CONFLICT",
                "managed MCP task path is occupied by a foreign task",
            ));
        }

        let current_document = self.store.read_current();
        let runtime_document = self.store.read_runtime();
        let current = match current_document {
            Document::Valid(value) => Some(value),
            Document::Missing | Document::Invalid(_) => None,
            Document::UnsupportedVersion(version) => {
                return Err(AppError::external(
                    "MCP_SERVICE_STATE_INVALID",
                    format!("unsupported managed MCP current schema version {version}"),
                ));
            }
        };
        let runtime = match runtime_document {
            Document::Valid(value) => Some(value),
            Document::Missing | Document::Invalid(_) => None,
            Document::UnsupportedVersion(version) => {
                return Err(AppError::external(
                    "MCP_SERVICE_STATE_INVALID",
                    format!("unsupported managed MCP runtime schema version {version}"),
                ));
            }
        };

        let task_marker = match &observation {
            TaskObservation::Owned(observed) => Some(observed.spec.marker.clone()),
            TaskObservation::Missing | TaskObservation::Foreign { .. } => None,
        };
        let trusted_service_id = task_marker
            .as_ref()
            .map(|marker| marker.service_id)
            .or_else(|| current.as_ref().map(|pointer| pointer.service_id));
        let definition_paths = self.verified_definition_deletion_set(
            &user_sid,
            trusted_service_id,
            task_marker.as_ref(),
            current.as_ref(),
            runtime.as_ref(),
        )?;

        let mut last_definition = None;
        if let Some(marker) = &task_marker
            && let Document::Valid(definition) = self.store.read_definition(&marker.definition_path)
        {
            last_definition = Some(definition);
        }
        if last_definition.is_none()
            && let Some(pointer) = &current
            && let Document::Valid(definition) =
                self.store.read_definition(&pointer.definition_path)
        {
            last_definition = Some(definition);
        }

        let stopped = match observation {
            TaskObservation::Owned(observed) => {
                if let Some(definition) = last_definition.clone() {
                    match self.context_from_owned_task(
                        expected_task_path.clone(),
                        observed.as_ref().clone(),
                        definition,
                    ) {
                        Ok(context) => self.stop_installed(context)?,
                        Err(_) => self.prove_inactive_owned_task(
                            &expected_task_path,
                            observed.as_ref().clone(),
                        )?,
                    }
                } else {
                    self.prove_inactive_owned_task(&expected_task_path, observed.as_ref().clone())?
                }
            }
            TaskObservation::Missing => self.stop_orphan()?,
            TaskObservation::Foreign { .. } => unreachable!(),
        };

        let ownership = stopped.ownership.clone();
        let mut changed = stopped.changed;
        if let Some(ownership) = ownership {
            changed |= self.scheduler.delete_owned(&ownership)?.deleted;
            if !matches!(
                self.scheduler.inspect(&expected_task_path)?,
                TaskObservation::Missing
            ) {
                return Err(AppError::external(
                    "MCP_SERVICE_UNINSTALL_INCOMPLETE",
                    "managed MCP task is still present after deletion",
                ));
            }
        }

        (|| -> Result<(), AppError> {
            changed |= self.store.remove_runtime()?;
            for path in definition_paths {
                changed |= self.store.remove_definition(&path)?;
            }
            changed |= self.store.remove_current()?;
            Ok(())
        })()
        .map_err(|error| {
            AppError::external(
                "MCP_SERVICE_UNINSTALL_INCOMPLETE",
                format!(
                    "managed MCP metadata cleanup failed: {}",
                    error.detail_message()
                ),
            )
        })?;

        let service_id = task_marker
            .as_ref()
            .map(|marker| marker.service_id)
            .or_else(|| current.as_ref().map(|pointer| pointer.service_id));
        let configuration_id = task_marker
            .as_ref()
            .map(|marker| marker.configuration_id)
            .or_else(|| current.as_ref().map(|pointer| pointer.configuration_id));
        let endpoint = last_definition.map(|definition| definition.endpoint.mcp_url);
        drop(stopped.proof_guard);
        Ok(UninstallOutput {
            command: "mcp.service.uninstall".to_owned(),
            schema_version: SCHEMA_VERSION,
            changed,
            action: if changed {
                "uninstalled"
            } else {
                "already_uninstalled"
            }
            .to_owned(),
            service_id,
            configuration_id,
            task_path: expected_task_path,
            endpoint,
            registration: "not_installed".to_owned(),
            runtime: RuntimeStatus::Stopped,
        })
    }

    pub(crate) fn context_from_owned_task(
        &self,
        task_path: String,
        observed: ObservedTask,
        definition: ServiceDefinition,
    ) -> Result<InstalledContext, AppError> {
        let expected_definition_path = self
            .store
            .paths()
            .definition(observed.spec.marker.configuration_id);
        if definition.user_sid != current_user_sid()?
            || observed.spec.marker.service_id != definition.service_id
            || observed.spec.marker.configuration_id != definition.configuration_id
            || !paths_equal(
                &observed.spec.marker.definition_path,
                &expected_definition_path,
            )
            || !paths_equal(&definition.runtime_state_path, &self.store.paths().runtime)
            || !paths_equal(
                &definition.instance_lock_path,
                &self.store.paths().instance_lock,
            )
        {
            return Err(AppError::external(
                "MCP_SERVICE_STATE_INVALID",
                "owned task identity does not match its managed definition",
            ));
        }
        let desired = DesiredTaskSpec::canonical(
            task_path.clone(),
            definition.user_sid.clone(),
            observed.spec.marker.clone(),
            definition.executable_path.clone(),
            definition.working_directory.clone(),
        );
        let current = CurrentPointer {
            schema_version: SCHEMA_VERSION,
            service_id: definition.service_id,
            configuration_id: definition.configuration_id,
            definition_path: observed.spec.marker.definition_path.clone(),
            task_path,
        };
        Ok(InstalledContext {
            current,
            definition,
            observed,
            desired,
        })
    }

    pub(crate) fn prove_inactive_owned_task(
        &self,
        task_path: &str,
        observed: ObservedTask,
    ) -> Result<StopResult, AppError> {
        if observed.spec.task_path != task_path {
            return Err(AppError::external(
                "MCP_SERVICE_TASK_CHANGED",
                "owned task path changed before uninstall",
            ));
        }
        let guard = lock::try_acquire(&self.store.paths().instance_lock)?.ok_or_else(|| {
            AppError::external(
                "MCP_SERVICE_STOP_UNSAFE",
                "owned task has no valid definition and the managed instance lease is occupied",
            )
        })?;
        if matches!(
            observed.scheduler_state,
            SchedulerState::Running | SchedulerState::Queued
        ) {
            return Err(AppError::external(
                "MCP_SERVICE_STOP_UNSAFE",
                "owned task has no valid definition and is still active",
            ));
        }
        let desired = observed.spec.clone();
        if !self.scheduler.instances(&desired)?.is_empty() {
            return Err(AppError::external(
                "MCP_SERVICE_STOP_UNSAFE",
                "owned task has active Scheduler instances without a valid definition",
            ));
        }
        let ownership = ExpectedTaskOwnership::from(&observed.spec);
        Ok(StopResult {
            ownership: Some(ownership),
            context: None,
            changed: false,
            action: "already_stopped".to_owned(),
            old_instance_id: None,
            proof_guard: guard,
        })
    }

    pub(crate) fn verified_definition_deletion_set(
        &self,
        user_sid: &str,
        trusted_service_id: Option<Uuid>,
        marker: Option<&TaskMarker>,
        current: Option<&CurrentPointer>,
        runtime: Option<&RuntimeState>,
    ) -> Result<BTreeSet<PathBuf>, AppError> {
        let mut paths = BTreeSet::new();
        if let Some(marker) = marker {
            self.insert_definition_candidate(
                &mut paths,
                marker.configuration_id,
                &marker.definition_path,
            )?;
        }
        if let Some(current) = current {
            self.insert_definition_candidate(
                &mut paths,
                current.configuration_id,
                &current.definition_path,
            )?;
        }
        if let Some(runtime) = runtime
            && Some(runtime.service_id) == trusted_service_id
        {
            paths.insert(self.store.paths().definition(runtime.configuration_id));
        }
        for path in self.store.definition_files()? {
            match self.store.read_definition(&path) {
                Document::Valid(definition)
                    if Some(definition.service_id) == trusted_service_id
                        && definition.user_sid == user_sid
                        && paths_equal(
                            &definition.runtime_state_path,
                            &self.store.paths().runtime,
                        )
                        && paths_equal(
                            &definition.instance_lock_path,
                            &self.store.paths().instance_lock,
                        )
                        && paths_equal(
                            &path,
                            &self.store.paths().definition(definition.configuration_id),
                        ) =>
                {
                    paths.insert(path);
                }
                Document::Missing
                | Document::Invalid(_)
                | Document::UnsupportedVersion(_)
                | Document::Valid(_) => {}
            }
        }
        Ok(paths)
    }

    pub(crate) fn insert_definition_candidate(
        &self,
        paths: &mut BTreeSet<PathBuf>,
        configuration_id: Uuid,
        candidate: &Path,
    ) -> Result<(), AppError> {
        let expected = self.store.paths().definition(configuration_id);
        if !paths_equal(candidate, &expected) {
            return Err(AppError::external(
                "MCP_SERVICE_STATE_INVALID",
                "managed MCP definition deletion candidate is outside the canonical store",
            ));
        }
        paths.insert(expected);
        Ok(())
    }
}
