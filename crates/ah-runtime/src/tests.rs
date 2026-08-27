use std::{
    collections::BTreeMap,
    ffi::CStr,
    fs,
    path::Path,
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicUsize, Ordering as AtomicOrdering},
    },
    time::{SystemTime, UNIX_EPOCH},
};

use ah_plugin_api::{GlobalOptionsWire, ResolvedSecret, SecretSlot};

use super::*;

fn test_metadata(plugin_name: &str, domain: &str) -> PluginMetadata {
    PluginMetadata {
        plugin_name: plugin_name.to_owned(),
        domain: domain.to_owned(),
        description: format!("{domain} test plugin"),
        abi_version: AH_PLUGIN_ABI_VERSION,
        required_tools: Vec::new(),
        compatibility: Default::default(),
    }
}

struct EchoBuiltinPlugin;

struct FixedSecretResolver {
    calls: AtomicUsize,
}

impl SecretResolver for FixedSecretResolver {
    fn resolve(&self, id: &str) -> Result<ResolvedSecret, SecretResolverError> {
        self.calls.fetch_add(1, AtomicOrdering::Relaxed);
        Ok(ResolvedSecret {
            id: id.to_owned(),
            kind: "postgres".to_owned(),
            values: BTreeMap::from([("password".to_owned(), "private-password".to_owned())]),
        })
    }
}

struct ErrorSecretResolver(SecretResolverError);

impl SecretResolver for ErrorSecretResolver {
    fn resolve(&self, _id: &str) -> Result<ResolvedSecret, SecretResolverError> {
        Err(self.0)
    }
}

struct WrongKindSecretResolver;

impl SecretResolver for WrongKindSecretResolver {
    fn resolve(&self, id: &str) -> Result<ResolvedSecret, SecretResolverError> {
        Ok(ResolvedSecret {
            id: id.to_owned(),
            kind: "http-basic".to_owned(),
            values: BTreeMap::from([("password".to_owned(), "redaction-sentinel".to_owned())]),
        })
    }
}

struct SecretProbePlugin {
    received: Arc<Mutex<Option<TypedInvocationRequest>>>,
    required: bool,
    include_slot: bool,
}

fn probe_descriptor(required: bool, include_slot: bool) -> CommandDescriptor {
    let mut slot = SecretSlot::optional("database", ["postgres"], "Database credential");
    slot.required = required;
    let descriptor = CommandDescriptor::new(
        "probe.run",
        "Probe",
        "Capture one typed request for runtime tests.",
        serde_json::json!({
            "type": "object",
            "properties": {},
            "additionalProperties": false
        }),
        serde_json::json!({
            "type": "object",
            "properties": {},
            "additionalProperties": false
        }),
        ah_plugin_api::CommandEffects::new(
            true,
            false,
            true,
            false,
            vec![ah_plugin_api::CommandEffect::ConfigurationRead],
            ah_plugin_api::RiskLevel::Low,
            "Captures an in-memory test request only.",
            ah_plugin_api::Reversibility::Yes,
        ),
    );
    if include_slot {
        descriptor.with_secret_slot(slot)
    } else {
        descriptor
    }
}

impl BuiltinPlugin for SecretProbePlugin {
    fn metadata(&self) -> PluginMetadata {
        let mut metadata = test_metadata("builtin-probe", "probe");
        metadata.compatibility = ah_plugin_api::PluginCompatibility::current()
            .with_capability(plugin_capabilities::TYPED_COMMANDS_V1);
        metadata
    }

    fn manual(&self) -> PluginManual {
        PluginManual {
            plugin_name: "builtin-probe".to_owned(),
            domain: "probe".to_owned(),
            description: "probe test plugin".to_owned(),
            commands: Vec::new(),
            notes: Vec::new(),
        }
    }

    fn invoke(&self, _request: &InvocationRequest) -> InvocationResponse {
        InvocationResponse::ok(None)
    }

    fn command_catalog(&self) -> Option<CommandCatalog> {
        Some(CommandCatalog::new(
            "builtin-probe",
            "probe",
            vec![probe_descriptor(self.required, self.include_slot)],
        ))
    }

    fn invoke_typed(&self, request: &TypedInvocationRequest) -> TypedInvocationResponse {
        *self.received.lock().unwrap() = Some(request.clone());
        TypedInvocationResponse::success(serde_json::json!({}), None)
    }
}

fn probe_manager(
    required: bool,
    resolver: Option<Arc<dyn SecretResolver>>,
    host: bool,
) -> (PluginManager, Arc<Mutex<Option<TypedInvocationRequest>>>) {
    let received = Arc::new(Mutex::new(None));
    let mut manager = PluginManager::new();
    if let Some(resolver) = resolver {
        manager.set_secret_resolver(resolver);
    }
    let plugin = Arc::new(SecretProbePlugin {
        received: Arc::clone(&received),
        required,
        include_slot: true,
    });
    if host {
        manager.register_host_builtin(plugin);
    } else {
        manager.register_builtin(plugin);
    }
    (manager, received)
}

static DYNAMIC_PROBE_REQUEST: OnceLock<Mutex<Option<TypedInvocationRequest>>> = OnceLock::new();

unsafe extern "C" fn dynamic_probe_invoke(request: *const c_char) -> *mut c_char {
    let raw = unsafe { CStr::from_ptr(request) }.to_string_lossy();
    let request = serde_json::from_str::<TypedInvocationRequest>(&raw).unwrap();
    *DYNAMIC_PROBE_REQUEST
        .get_or_init(|| Mutex::new(None))
        .lock()
        .unwrap() = Some(request);
    let response = TypedInvocationResponse::success(serde_json::json!({}), None);
    CString::new(serde_json::to_string(&response).unwrap())
        .unwrap()
        .into_raw()
}

unsafe extern "C" fn dynamic_probe_free(value: *mut c_char) {
    if !value.is_null() {
        drop(unsafe { CString::from_raw(value) });
    }
}

unsafe extern "C" fn dynamic_probe_cancel(_request_id: *const c_char) -> i32 {
    0
}

#[cfg(unix)]
fn current_process_library() -> Library {
    libloading::os::unix::Library::this().into()
}

#[cfg(windows)]
fn current_process_library() -> Library {
    libloading::os::windows::Library::this().unwrap().into()
}

fn register_dynamic_probe(manager: &mut PluginManager) {
    let mut metadata = test_metadata("dynamic-probe", "probe");
    metadata.compatibility = ah_plugin_api::PluginCompatibility::current()
        .with_capability(plugin_capabilities::TYPED_COMMANDS_V1);
    manager.dynamic_plugins.insert(
        "probe".to_owned(),
        DynamicPlugin {
            _library: current_process_library(),
            metadata,
            invoke_json: dynamic_probe_invoke,
            manual_json: None,
            command_catalog: Some(CommandCatalog::new(
                "dynamic-probe",
                "probe",
                vec![probe_descriptor(false, true)],
            )),
            invoke_command_json: Some(dynamic_probe_invoke),
            cancel_command: Some(dynamic_probe_cancel),
            free_c_string: dynamic_probe_free,
        },
    );
    manager.invalidate_typed_registry();
}

#[test]
fn a_domain_without_a_secret_resolver_reports_the_vault_as_unavailable() {
    let mut manager = PluginManager::new();
    register_dynamic_probe(&mut manager);

    let error = manager
        .invoke_credentialed(
            "probe",
            vec!["run".to_owned()],
            GlobalOptionsWire {
                json: false,
                quiet: false,
                limit: None,
                cwd: None,
            },
            &BTreeMap::from([("database".to_owned(), "app-db".to_owned())]),
        )
        .expect_err("no resolver means no credential");

    assert!(matches!(error, RuntimeError::VaultKeyUnavailable { .. }));
}

impl BuiltinPlugin for EchoBuiltinPlugin {
    fn metadata(&self) -> PluginMetadata {
        PluginMetadata {
            plugin_name: "builtin-echo".to_owned(),
            domain: "echo".to_owned(),
            description: "echo test plugin".to_owned(),
            abi_version: AH_PLUGIN_ABI_VERSION,
            required_tools: Vec::new(),
            compatibility: ah_plugin_api::PluginCompatibility::current()
                .with_capability(plugin_capabilities::TYPED_COMMANDS_V1),
        }
    }

    fn manual(&self) -> PluginManual {
        PluginManual {
            plugin_name: "builtin-echo".to_owned(),
            domain: "echo".to_owned(),
            description: "echo test plugin".to_owned(),
            commands: Vec::new(),
            notes: vec!["test manual".to_owned()],
        }
    }

    fn invoke(&self, _request: &InvocationRequest) -> InvocationResponse {
        InvocationResponse::ok(Some("ok".to_owned()))
    }

    fn invoke_observed_into(
        &self,
        request: &InvocationRequest,
        _sink: &OutputSink,
    ) -> InvocationObservation {
        InvocationObservation::new(
            self.invoke(request),
            InvocationOutcome::RunCheck(RunCheckOutcome {
                success: false,
                timed_out: false,
                exit_code: Some(7),
            }),
        )
    }

    fn command_catalog(&self) -> Option<CommandCatalog> {
        Some(CommandCatalog::new(
            "builtin-echo",
            "echo",
            vec![CommandDescriptor::new(
                "echo.value",
                "Echo value",
                "Echo one value. Impact: reads the supplied value only.",
                serde_json::json!({
                    "type": "object",
                    "properties": {
                        "value": { "type": "string" }
                    },
                    "required": ["value"],
                    "additionalProperties": false
                }),
                serde_json::json!({
                    "type": "object",
                    "properties": {
                        "value": { "type": "string" }
                    },
                    "required": ["value"],
                    "additionalProperties": false
                }),
                ah_plugin_api::CommandEffects::new(
                    true,
                    false,
                    true,
                    false,
                    vec![ah_plugin_api::CommandEffect::ConfigurationRead],
                    ah_plugin_api::RiskLevel::Low,
                    "Reads the supplied value only.",
                    ah_plugin_api::Reversibility::Yes,
                ),
            )],
        ))
    }

    fn invoke_typed(&self, request: &TypedInvocationRequest) -> TypedInvocationResponse {
        TypedInvocationResponse::success(request.arguments.clone(), None)
    }

    fn cancel_typed(&self, request_id: &str) -> bool {
        request_id == "cancel-me"
    }
}

#[test]
fn load_dynamic_plugins_missing_dir_returns_empty_report() {
    let mut manager = PluginManager::new();
    let missing = unique_temp_path("missing");
    let report = manager.load_dynamic_plugins_from_dir(&missing);

    assert_eq!(report.loaded, 0);
    assert_eq!(report.skipped, 0);
    assert!(report.warnings.is_empty());
}

#[test]
fn reserved_dynamic_domains_are_normalized() {
    let mut manager = PluginManager::new();
    manager.reserve_dynamic_domains(["AI", " plugins ", "mcp"]);

    assert!(manager.reserved_dynamic_domains.contains("ai"));
    assert!(manager.reserved_dynamic_domains.contains("plugins"));
    assert!(manager.reserved_dynamic_domains.contains("mcp"));
}

#[test]
fn load_dynamic_plugins_skips_invalid_library_file() {
    let mut manager = PluginManager::new();
    let dir = unique_temp_path("invalid-plugin");
    fs::create_dir_all(&dir).expect("temp plugin dir should be created");
    let lib_path = dir.join(format!("broken.{}", dynamic_lib_extension()));
    fs::write(&lib_path, "not a dynamic library").expect("test plugin file should be written");

    let report = manager.load_dynamic_plugins_from_dir(&dir);
    assert_eq!(report.loaded, 0);
    assert_eq!(report.skipped, 1);
    assert_eq!(report.warnings.len(), 1);
    assert_eq!(report.warnings[0].path, lib_path);
    assert!(manager.list_plugins().is_empty());

    fs::remove_dir_all(&dir).expect("temp plugin dir should be removed");
}

#[test]
fn builtin_invocation_works_after_skipped_dynamic_plugin() {
    let mut manager = PluginManager::new();
    manager.register_builtin(Arc::new(EchoBuiltinPlugin));

    let dir = unique_temp_path("invalid-plugin-with-builtin");
    fs::create_dir_all(&dir).expect("temp plugin dir should be created");
    let lib_path = dir.join(format!("broken.{}", dynamic_lib_extension()));
    fs::write(&lib_path, "not a dynamic library").expect("test plugin file should be written");

    let report = manager.load_dynamic_plugins_from_dir(&dir);
    assert_eq!(report.loaded, 0);
    assert_eq!(report.skipped, 1);

    let response = manager
        .invoke(
            "echo",
            Vec::new(),
            GlobalOptionsWire {
                json: false,
                quiet: false,
                limit: None,
                cwd: None,
            },
        )
        .expect("builtin plugin should still be invokable");
    assert!(response.success);
    assert_eq!(response.message.as_deref(), Some("ok"));

    fs::remove_dir_all(&dir).expect("temp plugin dir should be removed");
}

#[test]
fn builtin_observation_is_available_without_changing_legacy_response() {
    let mut manager = PluginManager::new();
    manager.register_builtin(Arc::new(EchoBuiltinPlugin));
    let globals = GlobalOptionsWire {
        json: false,
        quiet: false,
        limit: None,
        cwd: None,
    };

    let observation = manager
        .invoke_observed("echo", Vec::new(), globals.clone())
        .expect("builtin observation should be returned");
    assert!(observation.response.success);
    assert_eq!(
        observation.outcome,
        Some(InvocationOutcome::RunCheck(RunCheckOutcome {
            success: false,
            timed_out: false,
            exit_code: Some(7),
        }))
    );

    let response = manager
        .invoke("echo", Vec::new(), globals)
        .expect("legacy invocation should remain available");
    assert!(response.success);
    assert_eq!(response.message.as_deref(), Some("ok"));
}

#[test]
fn builtin_typed_catalog_and_invocation_work() {
    let mut manager = PluginManager::new();
    manager.register_builtin(Arc::new(EchoBuiltinPlugin));

    let commands = manager
        .list_enabled_commands()
        .expect("typed catalog should load");
    assert_eq!(commands.len(), 1);
    assert_eq!(commands[0].descriptor.id, "echo.value");

    let request = TypedInvocationRequest::new(
        "echo.value",
        serde_json::json!({ "value": "hello" }),
        ah_plugin_api::ExecutionContextWire::new("request-1", ".", None, 1_000),
    );
    let response = manager
        .invoke_typed(&request)
        .expect("typed invocation should work");
    assert_eq!(response.data, Some(serde_json::json!({ "value": "hello" })));
    assert!(manager.cancel_typed("echo.value", "cancel-me"));
}

#[test]
fn resolver_keeps_public_id_and_injects_private_secret() {
    let resolver = Arc::new(FixedSecretResolver {
        calls: AtomicUsize::new(0),
    });
    let (manager, received) = probe_manager(false, Some(resolver.clone()), false);

    manager
        .invoke_typed(&TypedInvocationRequest::new(
            "probe.run",
            serde_json::json!({"credentials": {"database": "qa-lms"}}),
            ah_plugin_api::ExecutionContextWire::new("resolver-test", ".", None, 1_000),
        ))
        .expect("credential should resolve before builtin dispatch");

    let request = received.lock().unwrap().clone().unwrap();
    assert_eq!(request.arguments["credentials"]["database"], "qa-lms");
    assert_eq!(request.resolved_secrets["database"].id, "qa-lms");
    assert_eq!(request.resolved_secrets["database"].kind, "postgres");
    assert_eq!(
        request.resolved_secrets["database"].values["password"],
        "private-password"
    );
    assert_eq!(resolver.calls.load(AtomicOrdering::Relaxed), 1);
}

#[test]
fn resolver_injects_before_host_dispatch() {
    let resolver = Arc::new(FixedSecretResolver {
        calls: AtomicUsize::new(0),
    });
    let (manager, received) = probe_manager(false, Some(resolver), true);

    manager
        .invoke_typed(&TypedInvocationRequest::new(
            "probe.run",
            serde_json::json!({"credentials": {"database": "qa-lms"}}),
            ah_plugin_api::ExecutionContextWire::new("host-resolver-test", ".", None, 1_000),
        ))
        .unwrap();

    assert_eq!(
        received.lock().unwrap().as_ref().unwrap().resolved_secrets["database"].id,
        "qa-lms"
    );
}

#[test]
fn resolver_injects_before_dynamic_plugin_dispatch() {
    let resolver = Arc::new(FixedSecretResolver {
        calls: AtomicUsize::new(0),
    });
    let mut manager = PluginManager::new();
    manager.set_secret_resolver(resolver);
    register_dynamic_probe(&mut manager);
    *DYNAMIC_PROBE_REQUEST
        .get_or_init(|| Mutex::new(None))
        .lock()
        .unwrap() = None;

    manager
        .invoke_typed(&TypedInvocationRequest::new(
            "probe.run",
            serde_json::json!({"credentials": {"database": "qa-lms"}}),
            ah_plugin_api::ExecutionContextWire::new("dynamic-wire", ".", None, 1_000),
        ))
        .unwrap();
    let request = DYNAMIC_PROBE_REQUEST
        .get()
        .unwrap()
        .lock()
        .unwrap()
        .clone()
        .unwrap();

    assert_eq!(request.arguments["credentials"]["database"], "qa-lms");
    assert_eq!(request.resolved_secrets["database"].kind, "postgres");
}

#[test]
fn resolver_is_not_touched_for_commands_without_slots() {
    let resolver = Arc::new(FixedSecretResolver {
        calls: AtomicUsize::new(0),
    });
    let mut manager = PluginManager::new();
    manager.set_secret_resolver(resolver.clone());
    manager.register_builtin(Arc::new(EchoBuiltinPlugin));

    manager
        .invoke_typed(&TypedInvocationRequest::new(
            "echo.value",
            serde_json::json!({"value": "hello"}),
            ah_plugin_api::ExecutionContextWire::new("no-slots", ".", None, 1_000),
        ))
        .unwrap();

    assert_eq!(resolver.calls.load(AtomicOrdering::Relaxed), 0);
}

#[test]
fn caller_supplied_resolved_secrets_are_discarded_before_dispatch() {
    let injected = ResolvedSecret {
        id: "caller-controlled".to_owned(),
        kind: "postgres".to_owned(),
        values: BTreeMap::from([("password".to_owned(), "caller-private-value".to_owned())]),
    };

    let no_slot_received = Arc::new(Mutex::new(None));
    let mut no_slot_manager = PluginManager::new();
    no_slot_manager.register_builtin(Arc::new(SecretProbePlugin {
        received: Arc::clone(&no_slot_received),
        required: false,
        include_slot: false,
    }));
    no_slot_manager
        .invoke_typed(
            &TypedInvocationRequest::new(
                "probe.run",
                serde_json::json!({}),
                ah_plugin_api::ExecutionContextWire::new("no-slot-injection", ".", None, 1_000),
            )
            .with_resolved_secrets(BTreeMap::from([("database".to_owned(), injected.clone())])),
        )
        .unwrap();
    assert!(
        no_slot_received
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .resolved_secrets
            .is_empty()
    );

    let (empty_credentials_manager, empty_credentials_received) = probe_manager(false, None, false);
    empty_credentials_manager
        .invoke_typed(
            &TypedInvocationRequest::new(
                "probe.run",
                serde_json::json!({"credentials": {}}),
                ah_plugin_api::ExecutionContextWire::new(
                    "empty-credentials-injection",
                    ".",
                    None,
                    1_000,
                ),
            )
            .with_resolved_secrets(BTreeMap::from([("database".to_owned(), injected)])),
        )
        .unwrap();
    assert!(
        empty_credentials_received
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .resolved_secrets
            .is_empty()
    );
}

#[test]
fn caller_supplied_resolved_secrets_cannot_bypass_id_or_kind_validation() {
    let injected = BTreeMap::from([(
        "database".to_owned(),
        ResolvedSecret {
            id: "caller-controlled".to_owned(),
            kind: "postgres".to_owned(),
            values: BTreeMap::from([("password".to_owned(), "caller-private-value".to_owned())]),
        },
    )]);

    let missing_resolver: Arc<dyn SecretResolver> =
        Arc::new(ErrorSecretResolver(SecretResolverError::NotFound));
    let (missing_manager, missing_received) = probe_manager(false, Some(missing_resolver), false);
    let missing = missing_manager
        .invoke_typed(
            &TypedInvocationRequest::new(
                "probe.run",
                serde_json::json!({"credentials": {"database": "missing-id"}}),
                ah_plugin_api::ExecutionContextWire::new("injected-missing-id", ".", None, 1_000),
            )
            .with_resolved_secrets(injected.clone()),
        )
        .unwrap_err();
    assert!(matches!(missing, RuntimeError::SecretNotFound { .. }));
    assert!(missing_received.lock().unwrap().is_none());

    let wrong_kind_resolver: Arc<dyn SecretResolver> = Arc::new(WrongKindSecretResolver);
    let (wrong_kind_manager, wrong_kind_received) =
        probe_manager(false, Some(wrong_kind_resolver), false);
    let wrong_kind = wrong_kind_manager
        .invoke_typed(
            &TypedInvocationRequest::new(
                "probe.run",
                serde_json::json!({"credentials": {"database": "wrong-kind-id"}}),
                ah_plugin_api::ExecutionContextWire::new("injected-wrong-kind", ".", None, 1_000),
            )
            .with_resolved_secrets(injected),
        )
        .unwrap_err();
    assert!(matches!(
        wrong_kind,
        RuntimeError::SecretKindMismatch { .. }
    ));
    assert!(wrong_kind_received.lock().unwrap().is_none());
}

#[test]
fn resolver_returns_deterministic_redacted_errors() {
    let (required_manager, _) = probe_manager(true, None, false);
    let schema_error = required_manager
        .invoke_typed(&TypedInvocationRequest::new(
            "probe.run",
            serde_json::json!({"unexpected": true}),
            ah_plugin_api::ExecutionContextWire::new("schema-first", ".", None, 1_000),
        ))
        .unwrap_err();
    assert!(matches!(schema_error, RuntimeError::TypedInvocation(_)));

    let undeclared = required_manager
        .invoke_typed(&TypedInvocationRequest::new(
            "probe.run",
            serde_json::json!({"credentials": {"other": "qa-lms"}}),
            ah_plugin_api::ExecutionContextWire::new("undeclared", ".", None, 1_000),
        ))
        .unwrap_err();
    assert!(undeclared.to_string().contains("undeclared slot 'other'"));

    let required = required_manager
        .invoke_typed(&TypedInvocationRequest::new(
            "probe.run",
            serde_json::json!({}),
            ah_plugin_api::ExecutionContextWire::new("required", ".", None, 1_000),
        ))
        .unwrap_err();
    assert!(matches!(required, RuntimeError::SecretRequired { .. }));

    let missing_resolver: Arc<dyn SecretResolver> =
        Arc::new(ErrorSecretResolver(SecretResolverError::NotFound));
    let (missing_manager, _) = probe_manager(false, Some(missing_resolver), false);
    let missing = missing_manager
        .invoke_typed(&TypedInvocationRequest::new(
            "probe.run",
            serde_json::json!({"credentials": {"database": "missing-id"}}),
            ah_plugin_api::ExecutionContextWire::new("missing", ".", None, 1_000),
        ))
        .unwrap_err();
    assert!(matches!(missing, RuntimeError::SecretNotFound { .. }));

    let wrong_kind_resolver: Arc<dyn SecretResolver> = Arc::new(WrongKindSecretResolver);
    let (wrong_kind_manager, _) = probe_manager(false, Some(wrong_kind_resolver), false);
    let wrong_kind = wrong_kind_manager
        .invoke_typed(&TypedInvocationRequest::new(
            "probe.run",
            serde_json::json!({"credentials": {"database": "api-basic"}}),
            ah_plugin_api::ExecutionContextWire::new("wrong-kind", ".", None, 1_000),
        ))
        .unwrap_err();
    assert!(matches!(
        &wrong_kind,
        RuntimeError::SecretKindMismatch { .. }
    ));
    assert!(!wrong_kind.to_string().contains("redaction-sentinel"));

    let (unavailable_manager, _) = probe_manager(false, None, false);
    let unavailable = unavailable_manager
        .invoke_typed(&TypedInvocationRequest::new(
            "probe.run",
            serde_json::json!({"credentials": {"database": "qa-lms"}}),
            ah_plugin_api::ExecutionContextWire::new("unavailable", ".", None, 1_000),
        ))
        .unwrap_err();
    assert!(matches!(
        unavailable,
        RuntimeError::VaultKeyUnavailable { .. }
    ));

    let locked_resolver: Arc<dyn SecretResolver> =
        Arc::new(ErrorSecretResolver(SecretResolverError::VaultLocked));
    let (locked_manager, _) = probe_manager(false, Some(locked_resolver), false);
    let locked = locked_manager
        .invoke_typed(&TypedInvocationRequest::new(
            "probe.run",
            serde_json::json!({"credentials": {"database": "qa-lms"}}),
            ah_plugin_api::ExecutionContextWire::new("locked", ".", None, 1_000),
        ))
        .unwrap_err();
    assert!(matches!(locked, RuntimeError::VaultLocked { .. }));
}

#[test]
fn typed_registry_is_built_once_per_definition_revision() {
    let mut manager = PluginManager::new();
    manager.register_builtin(Arc::new(EchoBuiltinPlugin));
    assert_eq!(manager.registry_build_count(), 0);

    manager.list_enabled_commands().unwrap();
    assert_eq!(manager.registry_build_count(), 1);
    let request = TypedInvocationRequest::new(
        "echo.value",
        serde_json::json!({"value": "hello"}),
        ah_plugin_api::ExecutionContextWire::new("request-1", ".", None, 1_000),
    );
    manager.invoke_typed(&request).unwrap();
    assert!(manager.cancel_typed("echo.value", "cancel-me"));
    assert_eq!(manager.registry_build_count(), 1);

    manager.register_builtin(Arc::new(EchoBuiltinPlugin));
    manager.list_enabled_commands().unwrap();
    assert_eq!(manager.registry_build_count(), 2);
}

#[test]
fn catalog_revision_changes_only_for_real_enabled_state_mutations() {
    let mut manager = PluginManager::new();
    manager.register_builtin(Arc::new(EchoBuiltinPlugin));
    let initial_revision = manager.catalog_revision();

    manager.set_disabled_domains(Vec::new());
    assert_eq!(manager.catalog_revision(), initial_revision);

    manager.set_disabled_domains(vec!["ECHO".to_owned()]);
    let disabled_revision = manager.catalog_revision();
    assert!(disabled_revision > initial_revision);
    assert!(manager.list_enabled_commands().unwrap().is_empty());
    assert_eq!(manager.registry_build_count(), 1);

    manager.set_disabled_domains(vec!["echo".to_owned()]);
    assert_eq!(manager.catalog_revision(), disabled_revision);
    manager.set_disabled_domains(Vec::new());
    assert!(manager.catalog_revision() > disabled_revision);
    assert_eq!(manager.list_enabled_commands().unwrap().len(), 1);
    assert_eq!(manager.registry_build_count(), 1);
}

#[test]
fn builtin_typed_invocation_rejects_invalid_arguments() {
    let mut manager = PluginManager::new();
    manager.register_builtin(Arc::new(EchoBuiltinPlugin));
    let request = TypedInvocationRequest::new(
        "echo.value",
        serde_json::json!({ "unexpected": true }),
        ah_plugin_api::ExecutionContextWire::new("request-1", ".", None, 1_000),
    );

    let error = manager
        .invoke_typed(&request)
        .expect_err("invalid arguments should fail");
    assert!(matches!(error, RuntimeError::TypedInvocation(_)));
}

#[test]
fn collect_plugin_manuals_includes_builtin_plugins() {
    let mut manager = PluginManager::new();
    manager.register_builtin(Arc::new(EchoBuiltinPlugin));

    let manuals = manager.collect_plugin_manuals();
    assert_eq!(manuals.len(), 1);
    assert_eq!(manuals[0].domain, "echo");
    assert_eq!(manuals[0].plugin_name, "builtin-echo");
}

#[test]
fn disabled_domain_blocks_invocation_and_manuals() {
    let mut manager = PluginManager::new();
    manager.register_builtin(Arc::new(EchoBuiltinPlugin));
    manager.set_disabled_domains(vec!["echo".to_owned()]);

    let invoke = manager.invoke(
        "echo",
        Vec::new(),
        GlobalOptionsWire {
            json: false,
            quiet: false,
            limit: None,
            cwd: None,
        },
    );
    let Err(RuntimeError::DomainDisabled(domain)) = invoke else {
        panic!("expected domain disabled error");
    };
    assert_eq!(domain, "echo");

    assert!(manager.collect_plugin_manuals().is_empty());
    assert!(manager.list_enabled_plugins().is_empty());
    assert_eq!(manager.list_plugins().len(), 1);
}

#[test]
fn dynamic_domain_conflicts_record_sources_and_winner() {
    let mut report = PluginLoadReport::default();
    let new_dynamic = test_metadata("external-echo-v2", "echo");
    let existing_dynamic = test_metadata("external-echo-v1", "echo");
    let existing_builtin = test_metadata("builtin-echo", "echo");

    push_dynamic_plugin_conflicts(
        &mut report,
        "echo",
        &new_dynamic,
        Some(&existing_dynamic),
        Some(existing_builtin),
    );

    assert_eq!(report.conflicts.len(), 2);
    assert_eq!(report.conflicts[0].domain, "echo");
    assert_eq!(report.conflicts[0].winner.plugin_name, "external-echo-v2");
    assert_eq!(report.conflicts[0].loser.plugin_name, "external-echo-v1");
    assert_eq!(report.conflicts[0].winner_source, PluginSource::Dynamic);
    assert_eq!(report.conflicts[0].loser_source, PluginSource::Dynamic);
    assert!(report.conflicts[0].reason.contains("last loaded"));

    assert_eq!(report.conflicts[1].winner.plugin_name, "external-echo-v2");
    assert_eq!(report.conflicts[1].loser.plugin_name, "builtin-echo");
    assert_eq!(report.conflicts[1].winner_source, PluginSource::Dynamic);
    assert_eq!(report.conflicts[1].loser_source, PluginSource::Builtin);
    assert!(report.conflicts[1].reason.contains("shadows builtin"));
}

#[test]
fn plugin_api_contract_rejects_unsupported_major_version() {
    let mut metadata = test_metadata("external-echo", "echo");
    metadata.compatibility.api_version = ah_plugin_api::PluginApiVersion { major: 2, minor: 0 };

    let error = validate_plugin_api_contract(
        Path::new("plugin.dll"),
        &metadata,
        false,
        TypedSymbolAvailability::default(),
    )
    .expect_err("unsupported major version should fail");
    let RuntimeError::ApiVersionMismatch {
        found_major,
        found_minor,
        supported_major,
        supported_minor,
        ..
    } = error
    else {
        panic!("expected API version mismatch");
    };
    assert_eq!(found_major, 2);
    assert_eq!(found_minor, 0);
    assert_eq!(supported_major, AH_PLUGIN_API_MAJOR_VERSION);
    assert_eq!(supported_minor, AH_PLUGIN_API_MINOR_VERSION);
}

#[test]
fn plugin_metadata_contract_rejects_name_domain_and_abi_mismatch() {
    let metadata = test_metadata("external-echo", "echo");

    let name_error = validate_plugin_metadata_contract(
        Path::new("plugin.dll"),
        AH_PLUGIN_ABI_VERSION,
        "external-other",
        "echo",
        "echo test plugin",
        &metadata,
        false,
        TypedSymbolAvailability::default(),
    )
    .expect_err("name mismatch should fail");
    assert!(name_error.to_string().contains("plugin_name"));

    let domain_error = validate_plugin_metadata_contract(
        Path::new("plugin.dll"),
        AH_PLUGIN_ABI_VERSION,
        "external-echo",
        "other",
        "echo test plugin",
        &metadata,
        false,
        TypedSymbolAvailability::default(),
    )
    .expect_err("domain mismatch should fail");
    assert!(domain_error.to_string().contains("domain"));

    let abi_error = validate_plugin_metadata_contract(
        Path::new("plugin.dll"),
        AH_PLUGIN_ABI_VERSION + 1,
        "external-echo",
        "echo",
        "echo test plugin",
        &metadata,
        false,
        TypedSymbolAvailability::default(),
    )
    .expect_err("ABI mismatch should fail");
    assert!(abi_error.to_string().contains("abi_version"));
}

#[test]
fn plugin_api_contract_rejects_missing_manual_symbol_when_capability_declared() {
    let mut metadata = test_metadata("external-echo", "echo");
    metadata.compatibility = ah_plugin_api::PluginCompatibility::current()
        .with_capability(plugin_capabilities::MANUAL_JSON);

    let error = validate_plugin_api_contract(
        Path::new("plugin.dll"),
        &metadata,
        false,
        TypedSymbolAvailability::default(),
    )
    .expect_err("declared manual_json capability without symbol should fail");
    assert!(error.to_string().contains(plugin_capabilities::MANUAL_JSON));
}

#[test]
fn plugin_api_contract_requires_complete_typed_symbols() {
    let mut metadata = test_metadata("external-echo", "echo");
    metadata.compatibility = ah_plugin_api::PluginCompatibility::current()
        .with_capability(plugin_capabilities::TYPED_COMMANDS_V1);

    let error = validate_plugin_api_contract(
        Path::new("plugin.dll"),
        &metadata,
        false,
        TypedSymbolAvailability {
            catalog: true,
            invoke: false,
            cancel: false,
        },
    )
    .expect_err("partial typed symbols should fail");
    assert!(
        error
            .to_string()
            .contains("ah_plugin_invoke_command_json_v1")
    );
    assert!(error.to_string().contains("ah_plugin_cancel_command_v1"));
}

#[test]
fn plugin_api_contract_rejects_unadvertised_typed_symbols() {
    let metadata = test_metadata("external-echo", "echo");
    let error = validate_plugin_api_contract(
        Path::new("plugin.dll"),
        &metadata,
        false,
        TypedSymbolAvailability {
            catalog: true,
            invoke: true,
            cancel: true,
        },
    )
    .expect_err("unadvertised typed symbols should fail");
    assert!(
        error
            .to_string()
            .contains(plugin_capabilities::TYPED_COMMANDS_V1)
    );
}

fn unique_temp_path(suffix: &str) -> PathBuf {
    let ticks = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time should be valid")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "ah-runtime-{suffix}-{ticks}-{}",
        std::process::id()
    ))
}

fn dynamic_lib_extension() -> &'static str {
    if cfg!(windows) {
        "dll"
    } else if cfg!(target_os = "macos") {
        "dylib"
    } else {
        "so"
    }
}
/// Credential ids, credential kinds and accepted-kind lists appear in
/// `Display` but must never reach a diagnostic — neither the host-facing
/// one nor the MCP-facing one, which is serialized into the tool response.
#[test]
fn secret_diagnostics_redact_credential_details_on_every_surface() {
    let id = "runtime-private-credential-id";
    let kind = "runtime-unexpected-private-kind";
    let accepted = "runtime-accepted-private-kind";
    let errors = [
        RuntimeError::SecretNotFound {
            command: "http.get".to_owned(),
            slot: "basic".to_owned(),
            id: id.to_owned(),
        },
        RuntimeError::SecretKindMismatch {
            command: "http.get".to_owned(),
            slot: "basic".to_owned(),
            id: id.to_owned(),
            kind: kind.to_owned(),
            accepted_kinds: vec![accepted.to_owned()],
        },
        RuntimeError::VaultLocked {
            command: "http.get".to_owned(),
            slot: "basic".to_owned(),
            id: id.to_owned(),
        },
        RuntimeError::VaultKeyUnavailable {
            command: "http.get".to_owned(),
            slot: "basic".to_owned(),
            id: id.to_owned(),
        },
    ];

    for error in errors {
        assert!(
            error.to_string().contains(id),
            "the test only proves something if Display still leaks: {error}"
        );
        let diagnostic = error.diagnostic();
        let command_error = error.command_error();
        for secret in [id, kind, accepted] {
            for field in [
                &diagnostic.message,
                &diagnostic.cause,
                &command_error.message,
                &command_error.cause,
            ] {
                assert!(!field.contains(secret), "'{secret}' leaked into '{field}'");
            }
        }
    }
}

/// The MCP wire contract froze code strings that differ from the host's,
/// and a retryability flag the host has no concept of.
#[test]
fn command_errors_keep_the_frozen_mcp_contract() {
    let cases = [
        (
            RuntimeError::ResponseParse("bad json".to_owned()),
            "PLUGIN_RESPONSE_PARSE_FAILED",
            "PLUGIN_RESPONSE_INVALID",
            false,
        ),
        (
            RuntimeError::TypedCommandNotFound("probe.run".to_owned()),
            "TYPED_COMMAND_NOT_FOUND",
            "COMMAND_NOT_FOUND",
            true,
        ),
        (
            RuntimeError::ExecutionCancelled {
                request_id: "req-1".to_owned(),
            },
            "EXECUTION_CANCELLED",
            "CANCELLED",
            false,
        ),
        (
            RuntimeError::ExecutionTimeout {
                request_id: "req-1".to_owned(),
            },
            "EXECUTION_TIMEOUT",
            "TIMEOUT",
            true,
        ),
        (
            RuntimeError::ExecutionPanic {
                request_id: "req-1".to_owned(),
            },
            "EXECUTION_HANDLER_PANIC",
            "HANDLER_PANIC",
            false,
        ),
        (
            RuntimeError::ExecutionCapacityFull { capacity: 4 },
            "EXECUTION_CAPACITY_FULL",
            "EXECUTION_CAPACITY_FULL",
            true,
        ),
        (
            RuntimeError::DomainDisabled("git".to_owned()),
            "DOMAIN_DISABLED",
            "DOMAIN_DISABLED",
            false,
        ),
    ];

    for (error, host_code, agent_code, retryable) in cases {
        let diagnostic = error.diagnostic();
        // Exercise the `From` impls the call sites actually use.
        let command_error = CommandError::from(error);
        assert_eq!(diagnostic.code, host_code);
        assert_eq!(command_error.code, agent_code);
        assert_eq!(command_error.retryable, retryable, "for {agent_code}");
    }
}

/// Not-found and disabled keep the exit hint an MCP client sees, while the
/// host diagnostic keeps the hint that matches `AppError::exit_code`.
#[test]
fn exit_code_hints_stay_per_surface() {
    let error = RuntimeError::DomainDisabled("git".to_owned());
    assert_eq!(error.diagnostic().exit_code_hint, 1);
    assert_eq!(error.command_error().exit_code_hint, 2);

    let error = RuntimeError::ExecutionWorker("worker gone".to_owned());
    assert_eq!(error.diagnostic().exit_code_hint, 1);
    assert_eq!(error.command_error().exit_code_hint, 1);
}
