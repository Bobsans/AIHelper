use std::sync::{Arc, Mutex, Weak};

use ah_plugin_api::{
    AH_PLUGIN_ABI_VERSION, CommandCatalog, CommandDescriptor, CommandEffect, CommandEffects,
    CommandError, CommandExample, InvocationRequest, InvocationResponse, PluginCompatibility,
    PluginManual, PluginMetadata, Reversibility, RiskLevel, TypedInvocationRequest,
    TypedInvocationResponse, plugin_capabilities,
};
use ah_runtime::{BuiltinPlugin, PluginManager};
use serde_json::{Value, json};

use crate::{
    ai,
    cli::PluginStateFilter,
    error::AppError,
    plugin_settings::PluginSettings,
    secrets::{SecretKind, VaultStore},
};

pub(crate) fn builtins(
    manager: Weak<PluginManager>,
    settings: Arc<Mutex<PluginSettings>>,
    vault: Arc<VaultStore>,
) -> Vec<Arc<dyn BuiltinPlugin>> {
    vec![
        Arc::new(AiHostPlugin {
            manager: manager.clone(),
        }),
        Arc::new(PluginsHostPlugin { manager, settings }),
        Arc::new(SecretsHostPlugin { vault }),
    ]
}

struct AiHostPlugin {
    manager: Weak<PluginManager>,
}

impl BuiltinPlugin for AiHostPlugin {
    fn metadata(&self) -> PluginMetadata {
        host_metadata("host-ai", "ai", "AIHelper agent manual host commands")
    }

    fn manual(&self) -> PluginManual {
        empty_manual(&self.metadata())
    }

    fn invoke(&self, _request: &InvocationRequest) -> InvocationResponse {
        InvocationResponse::error(
            "TYPED_COMMAND_REQUIRED",
            "host command is available through the typed runtime",
        )
    }

    fn command_catalog(&self) -> Option<CommandCatalog> {
        Some(CommandCatalog::new(
            "host-ai",
            "ai",
            vec![CommandDescriptor::new(
                "ai.info",
                "AIHelper agent manual",
                "Return the AI-oriented manual for all enabled domains or one selected domain.",
                json!({
                    "type": "object",
                    "properties": {
                        "domain": {
                            "type": "string",
                            "minLength": 1,
                            "description": "Optional domain filter."
                        }
                    },
                    "additionalProperties": false
                }),
                ai_info_output_schema(),
                CommandEffects::new(
                    true,
                    false,
                    true,
                    false,
                    vec![CommandEffect::ConfigurationRead],
                    RiskLevel::Low,
                    "Reads the in-memory command registry and returns documentation only.",
                    Reversibility::Yes,
                ),
            )],
        ))
    }

    fn invoke_typed(&self, request: &TypedInvocationRequest) -> TypedInvocationResponse {
        let Some(manager) = self.manager.upgrade() else {
            return host_unavailable("ai", &request.command);
        };
        let domain = request.arguments.get("domain").and_then(Value::as_str);
        match ai::typed_info_value(&manager, domain) {
            Ok(data) => TypedInvocationResponse::success(
                data,
                Some("Returned the AIHelper agent manual.".to_owned()),
            ),
            Err(error) => app_error_response("ai", &request.command, error),
        }
    }
}

struct PluginsHostPlugin {
    manager: Weak<PluginManager>,
    settings: Arc<Mutex<PluginSettings>>,
}

struct SecretsHostPlugin {
    vault: Arc<VaultStore>,
}

impl BuiltinPlugin for SecretsHostPlugin {
    fn metadata(&self) -> PluginMetadata {
        host_metadata(
            "host-secrets",
            "secrets",
            "AIHelper redacted secret discovery host commands",
        )
    }

    fn manual(&self) -> PluginManual {
        empty_manual(&self.metadata())
    }

    fn invoke(&self, _request: &InvocationRequest) -> InvocationResponse {
        InvocationResponse::error(
            "TYPED_COMMAND_REQUIRED",
            "host command is available through the typed runtime",
        )
    }

    fn command_catalog(&self) -> Option<CommandCatalog> {
        Some(CommandCatalog::new(
            "host-secrets",
            "secrets",
            vec![secrets_list_descriptor()],
        ))
    }

    fn invoke_typed(&self, request: &TypedInvocationRequest) -> TypedInvocationResponse {
        if request.command != "secrets.list" {
            return TypedInvocationResponse::error(CommandError::new(
                Some("secrets".to_owned()),
                Some(request.command.clone()),
                "TYPED_COMMAND_NOT_FOUND",
                "Unknown secret host command",
                "the command is not present in the host command catalog",
                2,
                false,
            ));
        }
        let kind = match request.arguments.get("kind").and_then(Value::as_str) {
            Some(value) => match value.parse::<SecretKind>() {
                Ok(kind) => Some(kind),
                Err(()) => {
                    return app_error_response(
                        "secrets",
                        &request.command,
                        AppError::invalid_argument(format!("unsupported secret kind: {value}")),
                    );
                }
            },
            None => None,
        };
        match self.vault.list_metadata() {
            Ok(mut secrets) => {
                if let Some(kind) = kind {
                    secrets.retain(|secret| secret.kind == kind);
                }
                let count = secrets.len();
                TypedInvocationResponse::success(
                    json!({"secrets": secrets}),
                    Some(format!("Returned {count} secret(s).")),
                )
            }
            Err(error) if error.code() == "VAULT_NOT_INITIALIZED" => {
                TypedInvocationResponse::success(
                    json!({"secrets": []}),
                    Some("Returned 0 secret(s).".to_owned()),
                )
            }
            Err(error) => app_error_response(
                "secrets",
                &request.command,
                AppError::external(error.code(), error.to_string()),
            ),
        }
    }
}

impl BuiltinPlugin for PluginsHostPlugin {
    fn metadata(&self) -> PluginMetadata {
        host_metadata(
            "host-plugins",
            "plugins",
            "AIHelper live plugin management host commands",
        )
    }

    fn manual(&self) -> PluginManual {
        empty_manual(&self.metadata())
    }

    fn invoke(&self, _request: &InvocationRequest) -> InvocationResponse {
        InvocationResponse::error(
            "TYPED_COMMAND_REQUIRED",
            "host command is available through the typed runtime",
        )
    }

    fn command_catalog(&self) -> Option<CommandCatalog> {
        Some(CommandCatalog::new(
            "host-plugins",
            "plugins",
            vec![
                plugins_list_descriptor(),
                plugins_enable_descriptor(),
                plugins_disable_descriptor(),
                plugins_reset_descriptor(),
            ],
        ))
    }

    fn invoke_typed(&self, request: &TypedInvocationRequest) -> TypedInvocationResponse {
        let Some(manager) = self.manager.upgrade() else {
            return host_unavailable("plugins", &request.command);
        };
        let result = match request.command.as_str() {
            "plugins.list" => self.list(&manager, request),
            "plugins.enable" => self.mutate(&manager, request, PluginMutation::Enable),
            "plugins.disable" => self.mutate(&manager, request, PluginMutation::Disable),
            "plugins.reset" => self.mutate(&manager, request, PluginMutation::Reset),
            _ => {
                return TypedInvocationResponse::error(CommandError::new(
                    Some("plugins".to_owned()),
                    Some(request.command.clone()),
                    "TYPED_COMMAND_NOT_FOUND",
                    "Unknown plugin host command",
                    "the command is not present in the host command catalog",
                    2,
                    false,
                ));
            }
        };
        result.unwrap_or_else(|error| app_error_response("plugins", &request.command, error))
    }
}

impl PluginsHostPlugin {
    fn list(
        &self,
        manager: &PluginManager,
        request: &TypedInvocationRequest,
    ) -> Result<TypedInvocationResponse, AppError> {
        let state_filter = match request.arguments.get("state").and_then(Value::as_str) {
            Some("enabled") => Some(PluginStateFilter::Enabled),
            Some("disabled") => Some(PluginStateFilter::Disabled),
            Some(value) => {
                return Err(AppError::invalid_argument(format!(
                    "unsupported plugins state value: {value}"
                )));
            }
            None => None,
        };
        let plugins = crate::collect_plugin_list_entries(manager, state_filter)?;
        let count = plugins.len();
        Ok(TypedInvocationResponse::success(
            json!({"plugins": plugins}),
            Some(format!("Returned {count} registered plugin(s).")),
        ))
    }

    fn mutate(
        &self,
        manager: &PluginManager,
        request: &TypedInvocationRequest,
        mutation: PluginMutation,
    ) -> Result<TypedInvocationResponse, AppError> {
        let mut settings = self
            .settings
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let raw_domain = request.arguments.get("domain").and_then(Value::as_str);
        let all = request
            .arguments
            .get("all")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if matches!(mutation, PluginMutation::Reset) {
            match (raw_domain.is_some(), all) {
                (true, true) => {
                    return Err(AppError::invalid_argument(
                        "plugins.reset accepts domain or all=true, not both",
                    ));
                }
                (false, false) => {
                    return Err(AppError::invalid_argument(
                        "plugins.reset requires domain or all=true",
                    ));
                }
                _ => {}
            }
        }

        let (command, domain, changed, message) = match mutation {
            PluginMutation::Enable => {
                let domain = crate::validate_known_domain(
                    manager,
                    raw_domain.expect("validated input contains domain"),
                )?;
                let changed = settings.update(|candidate| candidate.enable_domain(&domain))?;
                (
                    "plugins.enable",
                    Some(domain.clone()),
                    changed,
                    if changed {
                        format!("Enabled plugin domain '{domain}'.")
                    } else {
                        format!("Plugin domain '{domain}' is already enabled.")
                    },
                )
            }
            PluginMutation::Disable => {
                let domain = crate::validate_known_domain(
                    manager,
                    raw_domain.expect("validated input contains domain"),
                )?;
                let changed = settings.update(|candidate| candidate.disable_domain(&domain))?;
                (
                    "plugins.disable",
                    Some(domain.clone()),
                    changed,
                    if changed {
                        format!("Disabled plugin domain '{domain}'.")
                    } else {
                        format!("Plugin domain '{domain}' is already disabled.")
                    },
                )
            }
            PluginMutation::Reset if all => {
                let changed = settings.update(|candidate| Ok(candidate.clear_all()))?;
                (
                    "plugins.reset",
                    None,
                    changed,
                    if changed {
                        "Reset all plugin domain overrides.".to_owned()
                    } else {
                        "No plugin domain overrides to reset.".to_owned()
                    },
                )
            }
            PluginMutation::Reset => {
                let raw_domain = raw_domain.ok_or_else(|| {
                    AppError::invalid_argument("plugins.reset requires domain or all=true")
                })?;
                let domain = crate::validate_known_domain(manager, raw_domain)?;
                let changed = settings.update(|candidate| candidate.reset_domain(&domain))?;
                (
                    "plugins.reset",
                    Some(domain.clone()),
                    changed,
                    if changed {
                        format!("Reset plugin domain '{domain}' to its default enabled state.")
                    } else {
                        format!("Plugin domain '{domain}' has no override.")
                    },
                )
            }
        };

        let disabled_domains = settings.disabled_domains().cloned().collect::<Vec<_>>();
        manager.set_disabled_domains(disabled_domains.clone());
        let data = json!({
            "command": command,
            "domain": domain,
            "changed": changed,
            "config_path": crate::normalize_path(settings.path()),
            "disabled_domains": disabled_domains
        });
        Ok(TypedInvocationResponse::success(data, Some(message)))
    }
}

#[derive(Clone, Copy)]
enum PluginMutation {
    Enable,
    Disable,
    Reset,
}

fn host_metadata(plugin_name: &str, domain: &str, description: &str) -> PluginMetadata {
    PluginMetadata {
        plugin_name: plugin_name.to_owned(),
        domain: domain.to_owned(),
        description: description.to_owned(),
        abi_version: AH_PLUGIN_ABI_VERSION,
        required_tools: Vec::new(),
        compatibility: PluginCompatibility::current()
            .with_capability(plugin_capabilities::TYPED_COMMANDS_V1),
    }
}

fn empty_manual(metadata: &PluginMetadata) -> PluginManual {
    PluginManual {
        plugin_name: metadata.plugin_name.clone(),
        domain: metadata.domain.clone(),
        description: metadata.description.clone(),
        commands: Vec::new(),
        notes: Vec::new(),
    }
}

fn plugins_list_descriptor() -> CommandDescriptor {
    CommandDescriptor::new(
        "plugins.list",
        "List AIHelper plugins",
        "List registered plugins, live enabled state, and MCP exposure status.",
        json!({
            "type": "object",
            "description": "Exactly one of domain or all=true is required.",
            "properties": {
                "state": {
                    "type": "string",
                    "enum": ["enabled", "disabled"],
                    "description": "Optional enabled-state filter."
                }
            },
            "additionalProperties": false
        }),
        json!({
            "type": "object",
            "properties": {
                "plugins": {
                    "type": "array",
                    "items": plugin_list_entry_schema()
                }
            },
            "required": ["plugins"],
            "additionalProperties": false
        }),
        CommandEffects::new(
            true,
            false,
            true,
            false,
            vec![CommandEffect::ConfigurationRead],
            RiskLevel::Low,
            "Reads the loaded plugin registry and persisted enabled-state overrides.",
            Reversibility::Yes,
        ),
    )
}

fn secret_kind_names() -> Vec<&'static str> {
    SecretKind::ALL.iter().map(|kind| kind.as_str()).collect()
}

fn secrets_list_descriptor() -> CommandDescriptor {
    CommandDescriptor::new(
        "secrets.list",
        "List redacted secrets",
        "List secret metadata without secret values or field-state information.",
        json!({
            "type": "object",
            "properties": {
                "kind": {
                    "type": "string",
                    "enum": secret_kind_names(),
                    "description": "Optional secret-kind filter."
                }
            },
            "additionalProperties": false
        }),
        json!({
            "type": "object",
            "properties": {
                "secrets": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "id": {"type": "string"},
                            "kind": {
                                "type": "string",
                                "enum": secret_kind_names()
                            },
                            "label": {"type": "string"},
                            "description": {"type": ["string", "null"]}
                        },
                        "required": ["id", "kind", "label", "description"],
                        "additionalProperties": false
                    }
                }
            },
            "required": ["secrets"],
            "additionalProperties": false
        }),
        CommandEffects::new(
            true,
            false,
            true,
            false,
            vec![CommandEffect::ConfigurationRead],
            RiskLevel::Low,
            "Reads and decrypts the local vault, then returns redacted metadata only.",
            Reversibility::Yes,
        ),
    )
    .with_example(CommandExample::new(
        "Filter PostgreSQL secrets",
        json!({"kind": "postgres"}),
    ))
}

fn plugins_enable_descriptor() -> CommandDescriptor {
    plugin_domain_mutation_descriptor(
        "plugins.enable",
        "Enable AIHelper plugin",
        "Enable one loaded plugin domain and persist the override.",
        "May add tools to the live MCP catalog and writes the plugin settings file.",
    )
}

fn plugins_disable_descriptor() -> CommandDescriptor {
    plugin_domain_mutation_descriptor(
        "plugins.disable",
        "Disable AIHelper plugin",
        "Disable one loaded plugin domain and persist the override.",
        "Removes the domain's tools from the live MCP catalog and writes the plugin settings file.",
    )
}

fn plugins_reset_descriptor() -> CommandDescriptor {
    CommandDescriptor::new(
        "plugins.reset",
        "Reset AIHelper plugin overrides",
        "Reset one plugin domain override or all overrides to the default enabled state.",
        json!({
            "type": "object",
            "properties": {
                "domain": {
                    "type": "string",
                    "minLength": 1,
                    "description": "Plugin domain to reset."
                },
                "all": {
                    "type": "boolean",
                    "const": true,
                    "description": "Reset every plugin domain override."
                }
            },
            "additionalProperties": false
        }),
        plugin_mutation_output_schema(),
        CommandEffects::new(
            false,
            false,
            true,
            false,
            vec![
                CommandEffect::ConfigurationWrite,
                CommandEffect::FilesystemWrite,
            ],
            RiskLevel::Medium,
            "May add multiple tools to the live MCP catalog and writes the plugin settings file.",
            Reversibility::Yes,
        ),
    )
}

fn plugin_domain_mutation_descriptor(
    id: &str,
    title: &str,
    description: &str,
    impact: &str,
) -> CommandDescriptor {
    CommandDescriptor::new(
        id,
        title,
        description,
        json!({
            "type": "object",
            "properties": {
                "domain": {
                    "type": "string",
                    "minLength": 1,
                    "description": "Loaded plugin domain."
                }
            },
            "required": ["domain"],
            "additionalProperties": false
        }),
        plugin_mutation_output_schema(),
        CommandEffects::new(
            false,
            false,
            true,
            false,
            vec![
                CommandEffect::ConfigurationWrite,
                CommandEffect::FilesystemWrite,
            ],
            RiskLevel::Medium,
            impact,
            Reversibility::Yes,
        ),
    )
}

fn plugin_mutation_output_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "command": {"type": "string"},
            "domain": {"type": ["string", "null"]},
            "changed": {"type": "boolean"},
            "config_path": {"type": "string"},
            "disabled_domains": {
                "type": "array",
                "items": {"type": "string"}
            }
        },
        "required": [
            "command",
            "domain",
            "changed",
            "config_path",
            "disabled_domains"
        ],
        "additionalProperties": false
    })
}

fn plugin_list_entry_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "plugin_name": {"type": "string"},
            "domain": {"type": "string"},
            "description": {"type": "string"},
            "abi_version": {"type": "integer", "minimum": 0},
            "required_tools": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "name": {"type": "string"},
                        "check_args": {
                            "type": "array",
                            "items": {"type": "string"}
                        },
                        "reason": {"type": "string"}
                    },
                    "required": ["name", "check_args", "reason"],
                    "additionalProperties": false
                }
            },
            "source": {"type": "string", "enum": ["builtin", "dynamic"]},
            "state": {"type": "string", "enum": ["enabled", "disabled"]},
            "mcp_exposed": {"type": "boolean"},
            "mcp_omission_reason": {"type": "string"}
        },
        "required": [
            "plugin_name",
            "domain",
            "description",
            "abi_version",
            "required_tools",
            "source",
            "state",
            "mcp_exposed"
        ],
        "additionalProperties": false
    })
}

fn ai_info_output_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "command": {"type": "string", "const": "ai.info"},
            "domain_filter": {"type": ["string", "null"]},
            "global_options": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "flag": {"type": "string"},
                        "description": {"type": "string"}
                    },
                    "required": ["flag", "description"],
                    "additionalProperties": false
                }
            },
            "host_commands": {
                "type": "array",
                "items": host_command_schema()
            },
            "plugin_count": {"type": "integer", "minimum": 0},
            "plugins": {
                "type": "array",
                "items": plugin_manual_schema()
            }
        },
        "required": [
            "command",
            "domain_filter",
            "global_options",
            "host_commands",
            "plugin_count",
            "plugins"
        ],
        "additionalProperties": false
    })
}

fn host_command_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "name": {"type": "string"},
            "summary": {"type": "string"},
            "usage": {"type": "string"},
            "examples": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "description": {"type": "string"},
                        "command": {"type": "string"}
                    },
                    "required": ["description", "command"],
                    "additionalProperties": false
                }
            }
        },
        "required": ["name", "summary", "usage", "examples"],
        "additionalProperties": false
    })
}

fn plugin_manual_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "plugin_name": {"type": "string"},
            "domain": {"type": "string"},
            "description": {"type": "string"},
            "commands": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "name": {"type": "string"},
                        "summary": {"type": "string"},
                        "usage": {"type": "string"},
                        "examples": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "description": {"type": "string"},
                                    "argv": {
                                        "type": "array",
                                        "items": {"type": "string"}
                                    }
                                },
                                "required": ["description", "argv"],
                                "additionalProperties": false
                            }
                        }
                    },
                    "required": ["name", "summary", "usage", "examples"],
                    "additionalProperties": false
                }
            },
            "notes": {
                "type": "array",
                "items": {"type": "string"}
            }
        },
        "required": ["plugin_name", "domain", "description", "commands", "notes"],
        "additionalProperties": false
    })
}

fn host_unavailable(domain: &str, command: &str) -> TypedInvocationResponse {
    TypedInvocationResponse::error(CommandError::new(
        Some(domain.to_owned()),
        Some(command.to_owned()),
        "HOST_RUNTIME_UNAVAILABLE",
        "AIHelper host runtime is unavailable",
        "the MCP server runtime was already released",
        1,
        false,
    ))
}

fn app_error_response(domain: &str, command: &str, error: AppError) -> TypedInvocationResponse {
    let code = error.code().to_owned();
    let retryable = code == "PERSISTENCE_LOCK_TIMEOUT";
    let message = error.user_message();
    let cause = error.detail_message();
    TypedInvocationResponse::error(CommandError::new(
        Some(domain.to_owned()),
        Some(command.to_owned()),
        code,
        message,
        cause,
        error.exit_code(),
        retryable,
    ))
}

#[cfg(test)]
mod tests {
    use std::{
        path::Path,
        sync::{Arc, Mutex},
    };

    use ah_plugin_api::{ExecutionContextWire, TypedInvocationRequest};
    use ah_runtime::PluginManager;
    use serde_json::json;

    use super::builtins;
    use crate::{
        plugin_settings::PluginSettings,
        plugins,
        secrets::{KeyProvider, NewSecret, VaultError, VaultStore},
    };

    struct FixedKey;

    impl KeyProvider for FixedKey {
        fn load_or_create(&self) -> Result<[u8; 32], VaultError> {
            Ok([29; 32])
        }
    }

    fn request(command: &str, arguments: serde_json::Value) -> TypedInvocationRequest {
        TypedInvocationRequest::new(
            command,
            arguments,
            ExecutionContextWire::new("host-test", ".", None, 1_000),
        )
    }

    fn runtime(settings: Arc<Mutex<PluginSettings>>, config_dir: &Path) -> Arc<PluginManager> {
        runtime_with_vault(settings, config_dir).0
    }

    fn runtime_with_vault(
        settings: Arc<Mutex<PluginSettings>>,
        config_dir: &Path,
    ) -> (Arc<PluginManager>, Arc<VaultStore>) {
        let vault = Arc::new(VaultStore::at(config_dir, Arc::new(FixedKey)));
        vault.initialize().unwrap();
        let manager = Arc::new_cyclic(|weak| {
            let mut manager = PluginManager::new();
            for plugin in plugins::builtins() {
                manager.register_builtin(plugin);
            }
            for plugin in builtins(weak.clone(), Arc::clone(&settings), Arc::clone(&vault)) {
                manager.register_host_builtin(plugin);
            }
            manager
        });
        (manager, vault)
    }

    fn runtime_without_initialized_vault(
        settings: Arc<Mutex<PluginSettings>>,
        config_dir: &Path,
    ) -> Arc<PluginManager> {
        let vault = Arc::new(VaultStore::at(config_dir, Arc::new(FixedKey)));
        Arc::new_cyclic(|weak| {
            let mut manager = PluginManager::new();
            for plugin in builtins(weak.clone(), Arc::clone(&settings), Arc::clone(&vault)) {
                manager.register_host_builtin(plugin);
            }
            manager
        })
    }

    #[test]
    fn host_commands_are_typed_but_not_registered_plugins() {
        let temp = tempfile::tempdir().unwrap();
        let settings = Arc::new(Mutex::new(
            PluginSettings::load_from_path(temp.path().join("plugins.json")).unwrap(),
        ));
        let manager = runtime(settings, temp.path());
        let command_ids = manager
            .list_enabled_commands()
            .unwrap()
            .into_iter()
            .map(|command| command.descriptor.id)
            .collect::<Vec<_>>();
        assert!(command_ids.contains(&"ai.info".to_owned()));
        assert!(command_ids.contains(&"plugins.list".to_owned()));
        assert!(command_ids.contains(&"secrets.list".to_owned()));
        assert!(command_ids.contains(&"ctx.pack".to_owned()));
        assert!(!command_ids.contains(&"mcp.serve".to_owned()));
        assert!(
            manager
                .list_registered_plugins()
                .iter()
                .all(|plugin| plugin.metadata.domain != "ai"
                    && plugin.metadata.domain != "plugins"
                    && plugin.metadata.domain != "secrets")
        );
    }

    #[test]
    fn ai_info_returns_valid_structured_manual() {
        let temp = tempfile::tempdir().unwrap();
        let settings = Arc::new(Mutex::new(
            PluginSettings::load_from_path(temp.path().join("plugins.json")).unwrap(),
        ));
        let manager = runtime(settings, temp.path());
        let response = manager
            .invoke_typed(&request("ai.info", json!({"domain": "ctx"})))
            .unwrap();
        assert!(response.success);
        let data = response.data.unwrap();
        assert_eq!(data["command"], "ai.info");
        assert_eq!(data["domain_filter"], "ctx");
        assert_eq!(data["plugin_count"], 1);
    }

    #[test]
    fn secrets_list_is_registered_as_a_read_only_typed_command() {
        let temp = tempfile::tempdir().unwrap();
        let settings = Arc::new(Mutex::new(
            PluginSettings::load_from_path(temp.path().join("plugins.json")).unwrap(),
        ));
        let (manager, vault) = runtime_with_vault(settings, temp.path());
        let secret_value = "typed-output-secret";
        vault
            .put(
                NewSecret::postgres("billing", "Billing", secret_value)
                    .with_description(Some("Production billing database".to_owned())),
            )
            .unwrap();

        let command = manager
            .list_enabled_commands()
            .unwrap()
            .into_iter()
            .find(|command| command.descriptor.id == "secrets.list")
            .expect("secrets.list should be registered");
        assert!(command.descriptor.effects.read_only);
        assert_eq!(
            command.descriptor.effects.risk,
            ah_plugin_api::RiskLevel::Low
        );
        assert_eq!(
            command.descriptor.examples[0].arguments,
            json!({"kind": "postgres"})
        );

        let response = manager
            .invoke_typed(&request("secrets.list", json!({"kind": "postgres"})))
            .unwrap();
        assert!(response.success);
        let data = response.data.unwrap();
        assert_eq!(
            data,
            json!({"secrets": [{
                "id": "billing",
                "kind": "postgres",
                "label": "Billing",
                "description": "Production billing database"
            }]})
        );
        assert!(!serde_json::to_string(&data).unwrap().contains(secret_value));
    }

    #[test]
    fn secrets_list_returns_empty_without_initialized_vault() {
        let temp = tempfile::tempdir().unwrap();
        let settings = Arc::new(Mutex::new(
            PluginSettings::load_from_path(temp.path().join("plugins.json")).unwrap(),
        ));
        let manager = runtime_without_initialized_vault(settings, temp.path());

        let response = manager
            .invoke_typed(&request("secrets.list", json!({})))
            .unwrap();

        assert!(response.success);
        assert_eq!(response.data, Some(json!({"secrets": []})));
    }

    #[test]
    fn plugin_mutation_updates_live_catalog_and_persists_settings() {
        let temp = tempfile::tempdir().unwrap();
        let settings_path = temp.path().join("plugins.json");
        let settings = Arc::new(Mutex::new(
            PluginSettings::load_from_path(settings_path.clone()).unwrap(),
        ));
        let manager = runtime(settings, temp.path());

        let disabled = manager
            .invoke_typed(&request("plugins.disable", json!({"domain": "ctx"})))
            .unwrap();
        assert!(disabled.success);
        assert!(manager.is_domain_disabled("ctx"));
        assert!(
            manager
                .list_enabled_commands()
                .unwrap()
                .iter()
                .all(|command| !command.descriptor.id.starts_with("ctx."))
        );
        assert!(settings_path.is_file());

        let enabled = manager
            .invoke_typed(&request("plugins.enable", json!({"domain": "ctx"})))
            .unwrap();
        assert!(enabled.success);
        assert!(!manager.is_domain_disabled("ctx"));
        assert!(
            manager
                .list_enabled_commands()
                .unwrap()
                .iter()
                .any(|command| command.descriptor.id == "ctx.pack")
        );
    }

    #[test]
    fn failed_plugin_persistence_does_not_change_live_or_retained_state() {
        let temp = tempfile::tempdir().unwrap();
        let blocked_parent = temp.path().join("blocked");
        std::fs::write(&blocked_parent, "not a directory").unwrap();
        let settings = Arc::new(Mutex::new(
            PluginSettings::load_from_path(blocked_parent.join("plugins.json")).unwrap(),
        ));
        let manager = runtime(Arc::clone(&settings), temp.path());

        let response = manager
            .invoke_typed(&request("plugins.disable", json!({"domain": "ctx"})))
            .unwrap();

        assert!(!response.success);
        assert!(!manager.is_domain_disabled("ctx"));
        assert!(!settings.lock().unwrap().is_disabled("ctx"));
    }

    #[test]
    fn plugin_list_reports_mcp_exposure_and_omission_reason() {
        let temp = tempfile::tempdir().unwrap();
        let settings = Arc::new(Mutex::new(
            PluginSettings::load_from_path(temp.path().join("plugins.json")).unwrap(),
        ));
        let manager = runtime(settings, temp.path());
        let response = manager
            .invoke_typed(&request("plugins.list", json!({})))
            .unwrap();
        let data = response.data.unwrap();
        let plugins = data["plugins"].as_array().unwrap();
        let ctx = plugins
            .iter()
            .find(|plugin| plugin["domain"] == "ctx")
            .unwrap();
        assert_eq!(ctx["mcp_exposed"], true);
        assert!(ctx.get("mcp_omission_reason").is_none());
        let file = plugins
            .iter()
            .find(|plugin| plugin["domain"] == "file")
            .unwrap();
        assert_eq!(file["mcp_exposed"], true);
        assert!(file.get("mcp_omission_reason").is_none());
        let search = plugins
            .iter()
            .find(|plugin| plugin["domain"] == "search")
            .unwrap();
        assert_eq!(search["mcp_exposed"], true);
        assert!(search.get("mcp_omission_reason").is_none());
        let task = plugins
            .iter()
            .find(|plugin| plugin["domain"] == "task")
            .unwrap();
        assert_eq!(task["mcp_exposed"], true);
        assert!(task.get("mcp_omission_reason").is_none());
    }
}
