//! Running a command: routing, credentials, typed requests and cancellation.
//!
//! The preflight checks live here too, because refusing to invoke is part of
//! invoking.

use super::*;

pub(super) fn preflight_required_tools(
    request: &InvocationRequest,
    required_tools: &[RequiredTool],
) -> Result<(), RuntimeError> {
    for tool in required_tools {
        if tool.name.trim().is_empty() {
            continue;
        }
        let check_args = if tool.check_args.is_empty() {
            vec!["--version".to_owned()]
        } else {
            tool.check_args.clone()
        };
        if !core::run_command_ok(&tool.name, &check_args) {
            return Err(RuntimeError::DependencyMissing {
                domain: request.domain.clone(),
                operation: infer_operation(&request.domain, &request.argv),
                tool: tool.name.clone(),
                reason: tool.reason.clone(),
            });
        }
    }
    Ok(())
}

pub(super) fn preflight_typed_required_tools(
    command: &str,
    domain: &str,
    required_tools: &[RequiredTool],
) -> Result<(), RuntimeError> {
    for tool in required_tools {
        if tool.name.trim().is_empty() {
            continue;
        }
        let check_args = if tool.check_args.is_empty() {
            vec!["--version".to_owned()]
        } else {
            tool.check_args.clone()
        };
        if !core::run_command_ok(&tool.name, &check_args) {
            return Err(RuntimeError::DependencyMissing {
                domain: domain.to_owned(),
                operation: Some(command.to_owned()),
                tool: tool.name.clone(),
                reason: tool.reason.clone(),
            });
        }
    }
    Ok(())
}

pub(super) fn require_secret_slots(
    command: &str,
    slots: &[SecretSlot],
    credentials: Option<&serde_json::Map<String, serde_json::Value>>,
) -> Result<(), RuntimeError> {
    if let Some(slot) = slots.iter().find(|slot| {
        slot.required && !credentials.is_some_and(|value| value.contains_key(&slot.name))
    }) {
        return Err(RuntimeError::SecretRequired {
            command: command.to_owned(),
            slot: slot.name.clone(),
        });
    }
    Ok(())
}

pub(super) fn map_secret_resolver_error(
    error: SecretResolverError,
    command: &str,
    slot: &str,
    id: &str,
) -> RuntimeError {
    match error {
        SecretResolverError::NotFound => RuntimeError::SecretNotFound {
            command: command.to_owned(),
            slot: slot.to_owned(),
            id: id.to_owned(),
        },
        SecretResolverError::VaultLocked => RuntimeError::VaultLocked {
            command: command.to_owned(),
            slot: slot.to_owned(),
            id: id.to_owned(),
        },
        SecretResolverError::VaultKeyUnavailable => RuntimeError::VaultKeyUnavailable {
            command: command.to_owned(),
            slot: slot.to_owned(),
            id: id.to_owned(),
        },
    }
}

pub(super) fn infer_operation(domain: &str, argv: &[String]) -> Option<String> {
    argv.first().map(|command| format!("{domain}.{command}"))
}

pub(super) fn domain_key(domain: &str) -> String {
    domain.trim().to_ascii_lowercase()
}

impl PluginManager {
    pub fn invoke(
        &self,
        domain: &str,
        argv: Vec<String>,
        globals: GlobalOptionsWire,
    ) -> Result<InvocationResponse, RuntimeError> {
        self.invoke_observed(domain, argv, globals)
            .map(|observation| observation.response)
    }

    pub fn invoke_observed(
        &self,
        domain: &str,
        argv: Vec<String>,
        globals: GlobalOptionsWire,
    ) -> Result<InvocationObservation, RuntimeError> {
        self.invoke_credentialed(domain, argv, globals, &BTreeMap::new())
    }

    /// Runs one legacy argv invocation with `--credential SLOT=ID` mappings
    /// resolved host-side. The plugin sees only the resolved values, never the
    /// argv form, and validates that each slot is one it accepts.
    pub fn invoke_credentialed(
        &self,
        domain: &str,
        argv: Vec<String>,
        globals: GlobalOptionsWire,
        credentials: &BTreeMap<String, String>,
    ) -> Result<InvocationObservation, RuntimeError> {
        let domain = domain_key(domain);
        let request = InvocationRequest {
            domain: domain.clone(),
            argv,
            globals,
            resolved_secrets: self.resolve_credentials(&domain, credentials)?,
        };
        if let Some(plugin) = self.dynamic_plugins.get(&domain) {
            if self.is_domain_disabled(&domain) {
                return Err(RuntimeError::DomainDisabled(domain.clone()));
            }
            preflight_required_tools(&request, &plugin.metadata.required_tools)?;
            return plugin
                .invoke(&request)
                .map(InvocationObservation::without_outcome);
        }
        if let Some(plugin) = self.builtin_plugins.get(&domain) {
            if self.is_domain_disabled(&domain) {
                return Err(RuntimeError::DomainDisabled(domain.clone()));
            }
            let required_tools = plugin.required_tools(&request);
            preflight_required_tools(&request, &required_tools)?;
            return Ok(plugin.invoke_observed(&request));
        }

        Err(RuntimeError::DomainNotFound(domain))
    }

    pub(super) fn typed_registry(&self) -> Result<Arc<TypedRegistry>, RuntimeError> {
        if let Some(registry) = self
            .typed_registry
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_ref()
            .cloned()
        {
            return Ok(registry);
        }

        let mut cached = self
            .typed_registry
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(registry) = cached.as_ref() {
            return Ok(Arc::clone(registry));
        }
        let built = Arc::new(self.build_typed_registry()?);
        self.registry_build_count.fetch_add(1, Ordering::AcqRel);
        *cached = Some(Arc::clone(&built));
        Ok(built)
    }

    pub(super) fn build_typed_registry(&self) -> Result<TypedRegistry, RuntimeError> {
        let mut commands = Vec::new();
        for plugin in self.host_plugins.values() {
            let metadata = plugin.metadata();
            if let Some(catalog) = plugin.command_catalog() {
                append_typed_catalog(
                    &mut commands,
                    metadata,
                    PluginSource::Builtin,
                    TypedCommandRoute::Host,
                    catalog,
                )?;
            }
        }
        for (domain, plugin) in &self.builtin_plugins {
            if self.dynamic_plugins.contains_key(domain) {
                continue;
            }
            let metadata = plugin.metadata();
            if let Some(catalog) = plugin.command_catalog() {
                append_typed_catalog(
                    &mut commands,
                    metadata,
                    PluginSource::Builtin,
                    TypedCommandRoute::Builtin,
                    catalog,
                )?;
            }
        }
        for plugin in self.dynamic_plugins.values() {
            if let Some(catalog) = plugin.command_catalog.clone() {
                append_typed_catalog(
                    &mut commands,
                    plugin.metadata.clone(),
                    PluginSource::Dynamic,
                    TypedCommandRoute::Dynamic,
                    catalog,
                )?;
            }
        }

        commands.sort_by(|left, right| {
            left.registered
                .descriptor
                .id
                .cmp(&right.registered.descriptor.id)
                .then_with(|| {
                    left.registered
                        .plugin
                        .plugin_name
                        .cmp(&right.registered.plugin.plugin_name)
                })
        });
        let mut by_id = HashMap::with_capacity(commands.len());
        for command in &commands {
            let command_id = command.registered.descriptor.id.clone();
            if by_id
                .insert(command_id.clone(), Arc::clone(command))
                .is_some()
            {
                return Err(RuntimeError::InvalidCommandCatalog {
                    domain: command.registered.plugin.domain.clone(),
                    reason: format!(
                        "duplicate command id '{command_id}' across registered plugins"
                    ),
                });
            }
        }
        Ok(TypedRegistry { commands, by_id })
    }

    pub(super) fn invalidate_typed_registry(&mut self) {
        *self
            .typed_registry
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
        self.catalog_revision.fetch_add(1, Ordering::AcqRel);
    }

    #[cfg(test)]
    pub(super) fn registry_build_count(&self) -> u64 {
        self.registry_build_count.load(Ordering::Acquire)
    }

    pub fn command_catalog_for_domain(
        &self,
        domain: &str,
    ) -> Result<Option<CommandCatalog>, RuntimeError> {
        let domain = domain_key(domain);
        let metadata = if let Some(plugin) = self.dynamic_plugins.get(&domain) {
            plugin.metadata.clone()
        } else if let Some(plugin) = self.builtin_plugins.get(&domain) {
            plugin.metadata()
        } else {
            return Ok(None);
        };
        let registry = self.typed_registry()?;
        let commands: Vec<CommandDescriptor> = registry
            .commands
            .iter()
            .filter(|command| {
                command.route != TypedCommandRoute::Host
                    && command
                        .registered
                        .plugin
                        .domain
                        .eq_ignore_ascii_case(&domain)
            })
            .map(|command| command.registered.descriptor.clone())
            .collect();
        if commands.is_empty() {
            return Ok(None);
        }
        Ok(Some(CommandCatalog::new(
            metadata.plugin_name,
            metadata.domain,
            commands,
        )))
    }

    pub fn invoke_typed(
        &self,
        request: &TypedInvocationRequest,
    ) -> Result<TypedInvocationResponse, RuntimeError> {
        let registry = self.typed_registry()?;
        let command = registry
            .by_id
            .get(&request.command)
            .cloned()
            .ok_or_else(|| RuntimeError::TypedCommandNotFound(request.command.clone()))?;
        let domain = command.registered.plugin.domain.as_str();
        if command.route != TypedCommandRoute::Host && self.is_domain_disabled(domain) {
            return Err(RuntimeError::DomainDisabled(domain.to_owned()));
        }
        let request = self.prepare_typed_request(&command, request)?;
        let request = request.as_ref();

        let response = match command.route {
            TypedCommandRoute::Host => {
                let plugin = self
                    .host_plugins
                    .get(&domain_key(domain))
                    .ok_or_else(|| RuntimeError::TypedCommandNotFound(request.command.clone()))?;
                let required_tools = plugin.required_tools_typed(request);
                preflight_typed_required_tools(&request.command, domain, &required_tools)?;
                plugin.invoke_typed(request)
            }
            TypedCommandRoute::Builtin => {
                let plugin = self
                    .builtin_plugins
                    .get(&domain_key(domain))
                    .ok_or_else(|| RuntimeError::TypedCommandNotFound(request.command.clone()))?;
                let required_tools = plugin.required_tools_typed(request);
                preflight_typed_required_tools(&request.command, domain, &required_tools)?;
                plugin.invoke_typed(request)
            }
            TypedCommandRoute::Dynamic => {
                let plugin = self
                    .dynamic_plugins
                    .get(&domain_key(domain))
                    .ok_or_else(|| RuntimeError::TypedCommandNotFound(request.command.clone()))?;
                preflight_typed_required_tools(
                    &request.command,
                    domain,
                    &plugin.metadata.required_tools,
                )?;
                plugin.invoke_typed(request)?
            }
        };
        typed::validate_response_with(&request.command, &command.output_validator, &response)?;
        Ok(response)
    }

    pub(super) fn resolve_credentials(
        &self,
        command: &str,
        credentials: &BTreeMap<String, String>,
    ) -> Result<BTreeMap<String, ResolvedSecret>, RuntimeError> {
        let mut resolved = BTreeMap::new();
        for (slot, id) in credentials {
            let Some(resolver) = self.secret_resolver.as_ref() else {
                return Err(RuntimeError::VaultKeyUnavailable {
                    command: command.to_owned(),
                    slot: slot.clone(),
                    id: id.clone(),
                });
            };
            let secret = resolver
                .resolve(id)
                .map_err(|error| map_secret_resolver_error(error, command, slot, id))?;
            resolved.insert(slot.clone(), secret);
        }
        Ok(resolved)
    }

    pub(super) fn prepare_typed_request<'a>(
        &self,
        command: &RegisteredTypedCommand,
        request: &'a TypedInvocationRequest,
    ) -> Result<Cow<'a, TypedInvocationRequest>, RuntimeError> {
        let request = if request.resolved_secrets.is_empty() {
            Cow::Borrowed(request)
        } else {
            Cow::Owned(request.clone().with_resolved_secrets(BTreeMap::new()))
        };
        let public_request = request.as_ref();
        let slots = &command.registered.descriptor.secret_slots;
        if slots.is_empty() {
            typed::validate_arguments_with(
                &public_request.command,
                &command.input_validator,
                &public_request.arguments,
            )?;
            return Ok(request);
        }

        let mut schema_arguments = public_request.arguments.clone();
        if let Some(arguments) = schema_arguments.as_object_mut() {
            arguments.remove("credentials");
        }
        typed::validate_arguments_with(
            &public_request.command,
            &command.input_validator,
            &schema_arguments,
        )?;

        let arguments = public_request.arguments.as_object().ok_or_else(|| {
            RuntimeError::TypedInvocation(format!(
                "arguments for '{}' must be a JSON object",
                public_request.command
            ))
        })?;
        let credentials = match arguments.get("credentials") {
            None => None,
            Some(serde_json::Value::Object(credentials)) => Some(credentials),
            Some(_) => {
                return Err(RuntimeError::TypedInvocation(format!(
                    "credentials for '{}' must be a JSON object",
                    public_request.command
                )));
            }
        };
        let Some(credentials) = credentials else {
            require_secret_slots(&public_request.command, slots, None)?;
            return Ok(request);
        };

        let declared = slots
            .iter()
            .map(|slot| slot.name.as_str())
            .collect::<HashSet<_>>();
        let mut undeclared = credentials
            .keys()
            .filter(|name| !declared.contains(name.as_str()))
            .collect::<Vec<_>>();
        undeclared.sort();
        if let Some(slot) = undeclared.first() {
            return Err(RuntimeError::TypedInvocation(format!(
                "credentials for '{}' contain undeclared slot '{}'",
                public_request.command, slot
            )));
        }
        require_secret_slots(&public_request.command, slots, Some(credentials))?;

        if credentials.is_empty() {
            return Ok(request);
        }
        let resolver = self.secret_resolver.as_ref();
        let mut resolved = BTreeMap::new();
        for slot in slots {
            let Some(id) = credentials.get(&slot.name) else {
                continue;
            };
            let id = id.as_str().ok_or_else(|| {
                RuntimeError::TypedInvocation(format!(
                    "credential id for slot '{}' in '{}' must be a string",
                    slot.name, public_request.command
                ))
            })?;
            let secret = match resolver {
                Some(resolver) => resolver.resolve(id).map_err(|error| {
                    map_secret_resolver_error(error, &public_request.command, &slot.name, id)
                })?,
                None => {
                    return Err(RuntimeError::VaultKeyUnavailable {
                        command: public_request.command.clone(),
                        slot: slot.name.clone(),
                        id: id.to_owned(),
                    });
                }
            };
            if !slot.accepted_kinds.contains(&secret.kind) {
                return Err(RuntimeError::SecretKindMismatch {
                    command: public_request.command.clone(),
                    slot: slot.name.clone(),
                    id: id.to_owned(),
                    kind: secret.kind,
                    accepted_kinds: slot.accepted_kinds.clone(),
                });
            }
            resolved.insert(slot.name.clone(), secret);
        }

        Ok(Cow::Owned(
            request.into_owned().with_resolved_secrets(resolved),
        ))
    }

    pub fn cancel_typed(&self, command: &str, request_id: &str) -> bool {
        let Ok(registry) = self.typed_registry() else {
            return false;
        };
        let Some(registered) = registry.by_id.get(command) else {
            return false;
        };
        let domain = domain_key(&registered.registered.plugin.domain);
        match registered.route {
            TypedCommandRoute::Host => self
                .host_plugins
                .get(&domain)
                .is_some_and(|plugin| plugin.cancel_typed(request_id)),
            TypedCommandRoute::Builtin => self
                .builtin_plugins
                .get(&domain)
                .is_some_and(|plugin| plugin.cancel_typed(request_id)),
            TypedCommandRoute::Dynamic => self
                .dynamic_plugins
                .get(&domain)
                .is_some_and(|plugin| plugin.cancel_typed(request_id)),
        }
    }
}
