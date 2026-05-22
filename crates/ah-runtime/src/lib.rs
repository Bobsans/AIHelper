use std::{
    collections::{HashMap, HashSet},
    ffi::{CString, c_char},
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};

use ah_plugin_api::{
    AH_PLUGIN_ABI_VERSION, AH_PLUGIN_API_MAJOR_VERSION, AH_PLUGIN_API_MINOR_VERSION,
    AH_PLUGIN_ENTRY_V1_SYMBOL, AH_PLUGIN_MANUAL_JSON_V1_SYMBOL,
    AH_PLUGIN_METADATA_JSON_V1_SYMBOL, AhPluginEntryV1, AhPluginManualJsonV1,
    AhPluginMetadataJsonV1, GlobalOptionsWire, InvocationRequest, InvocationResponse,
    PluginManual, PluginMetadata, RequiredTool, c_ptr_to_string, plugin_capabilities,
};
use libloading::Library;
use thiserror::Error;

pub mod core;

#[derive(Debug, Error)]
pub enum RuntimeError {
    #[error("plugin not found for domain '{0}'")]
    DomainNotFound(String),
    #[error("failed to load plugin library at {path:?}: {source}")]
    LibraryLoad {
        path: PathBuf,
        source: libloading::Error,
    },
    #[error("failed to load plugin entrypoint from {path:?}: {source}")]
    SymbolLoad {
        path: PathBuf,
        source: libloading::Error,
    },
    #[error("plugin at {path:?} has incompatible abi version {found}, expected {expected}")]
    AbiVersionMismatch {
        path: PathBuf,
        found: u32,
        expected: u32,
    },
    #[error(
        "plugin at {path:?} requires unsupported plugin api version {found_major}.{found_minor}, host supports {supported_major}.{supported_minor}"
    )]
    ApiVersionMismatch {
        path: PathBuf,
        found_major: u16,
        found_minor: u16,
        supported_major: u16,
        supported_minor: u16,
    },
    #[error("plugin at {path:?} returned invalid metadata: {reason}")]
    InvalidMetadata { path: PathBuf, reason: String },
    #[error("plugin invocation failed: {0}")]
    Invocation(String),
    #[error("plugin response parse failed: {0}")]
    ResponseParse(String),
    #[error("plugin domain '{0}' is disabled")]
    DomainDisabled(String),
    #[error("required external tool '{tool}' is not available for domain '{domain}'")]
    DependencyMissing {
        domain: String,
        operation: Option<String>,
        tool: String,
        reason: String,
    },
}

pub trait BuiltinPlugin: Send + Sync {
    fn metadata(&self) -> PluginMetadata;
    fn manual(&self) -> PluginManual;
    fn required_tools(&self, _request: &InvocationRequest) -> Vec<RequiredTool> {
        self.metadata().required_tools
    }
    fn invoke(&self, request: &InvocationRequest) -> InvocationResponse;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PluginSource {
    Builtin,
    Dynamic,
}

#[derive(Debug, Clone)]
pub struct PluginLoadWarning {
    pub path: PathBuf,
    pub error: String,
}

#[derive(Debug, Clone)]
pub struct PluginLoadConflict {
    pub domain: String,
    pub winner: PluginMetadata,
    pub loser: PluginMetadata,
    pub winner_source: PluginSource,
    pub loser_source: PluginSource,
    pub reason: String,
}

#[derive(Debug, Default, Clone)]
pub struct PluginLoadReport {
    pub loaded: usize,
    pub skipped: usize,
    pub warnings: Vec<PluginLoadWarning>,
    pub conflicts: Vec<PluginLoadConflict>,
}

impl PluginLoadReport {
    fn push_warning(&mut self, path: PathBuf, error: impl Into<String>) {
        self.skipped += 1;
        self.warnings.push(PluginLoadWarning {
            path,
            error: error.into(),
        });
    }

    fn push_conflict(
        &mut self,
        winner: PluginMetadata,
        loser: PluginMetadata,
        winner_source: PluginSource,
        loser_source: PluginSource,
        reason: impl Into<String>,
    ) {
        let domain = winner.domain.clone();
        self.conflicts.push(PluginLoadConflict {
            domain,
            winner,
            loser,
            winner_source,
            loser_source,
            reason: reason.into(),
        });
    }
}

pub struct PluginManager {
    dynamic_plugins: HashMap<String, DynamicPlugin>,
    builtin_plugins: HashMap<String, Arc<dyn BuiltinPlugin>>,
    disabled_domains: HashSet<String>,
}

#[derive(Debug, Clone)]
pub struct RegisteredPlugin {
    pub metadata: PluginMetadata,
    pub source: PluginSource,
    pub enabled: bool,
}

impl PluginManager {
    pub fn new() -> Self {
        Self {
            dynamic_plugins: HashMap::new(),
            builtin_plugins: HashMap::new(),
            disabled_domains: HashSet::new(),
        }
    }

    pub fn set_disabled_domains<I>(&mut self, domains: I)
    where
        I: IntoIterator<Item = String>,
    {
        self.disabled_domains = domains
            .into_iter()
            .map(|domain| domain_key(&domain))
            .collect();
    }

    pub fn is_domain_disabled(&self, domain: &str) -> bool {
        self.disabled_domains.contains(&domain_key(domain))
    }

    pub fn register_builtin(&mut self, plugin: Arc<dyn BuiltinPlugin>) {
        let key = domain_key(&plugin.metadata().domain);
        self.builtin_plugins
            .insert(key, plugin);
    }

    pub fn load_dynamic_plugins_from_dir(&mut self, dir: &Path) -> PluginLoadReport {
        let mut report = PluginLoadReport::default();
        if !dir.exists() {
            return report;
        }
        let entries = match fs::read_dir(dir) {
            Ok(value) => value,
            Err(error) => {
                report.push_warning(
                    dir.to_path_buf(),
                    format!("failed to read plugin directory: {error}"),
                );
                return report;
            }
        };

        let mut plugin_paths = Vec::new();
        for entry in entries {
            let entry = match entry {
                Ok(value) => value,
                Err(error) => {
                    report.push_warning(
                        dir.to_path_buf(),
                        format!("failed to read plugin directory entry: {error}"),
                    );
                    continue;
                }
            };
            let path = entry.path();
            if !path.is_file() || !is_dynamic_lib_file(&path) {
                continue;
            }
            plugin_paths.push(path);
        }

        plugin_paths.sort_unstable_by(|left, right| left.file_name().cmp(&right.file_name()));
        for path in plugin_paths {
            match DynamicPlugin::load(path.clone()) {
                Ok(plugin) => {
                    let domain_key = domain_key(&plugin.metadata.domain);
                    push_dynamic_plugin_conflicts(
                        &mut report,
                        &domain_key,
                        &plugin.metadata,
                        self.dynamic_plugins
                            .get(&domain_key)
                            .map(|existing| &existing.metadata),
                        self.builtin_plugins
                            .get(&domain_key)
                            .map(|existing| existing.metadata()),
                    );

                    self.dynamic_plugins.insert(domain_key, plugin);
                    report.loaded += 1;
                }
                Err(error) => {
                    report.push_warning(path, error.to_string());
                }
            }
        }

        report
    }

    pub fn invoke(
        &self,
        domain: &str,
        argv: Vec<String>,
        globals: GlobalOptionsWire,
    ) -> Result<InvocationResponse, RuntimeError> {
        let domain = domain_key(domain);
        let request = InvocationRequest {
            domain: domain.clone(),
            argv,
            globals,
        };
        if let Some(plugin) = self.dynamic_plugins.get(&domain) {
            if self.is_domain_disabled(&domain) {
                return Err(RuntimeError::DomainDisabled(domain.clone()));
            }
            preflight_required_tools(&request, &plugin.metadata.required_tools)?;
            return plugin.invoke(&request);
        }
        if let Some(plugin) = self.builtin_plugins.get(&domain) {
            if self.is_domain_disabled(&domain) {
                return Err(RuntimeError::DomainDisabled(domain.clone()));
            }
            let required_tools = plugin.required_tools(&request);
            preflight_required_tools(&request, &required_tools)?;
            return Ok(plugin.invoke(&request));
        }

        Err(RuntimeError::DomainNotFound(domain))
    }

    pub fn list_plugins(&self) -> Vec<PluginMetadata> {
        self.list_registered_plugins()
            .into_iter()
            .map(|plugin| plugin.metadata)
            .collect()
    }

    pub fn list_registered_plugins(&self) -> Vec<RegisteredPlugin> {
        let mut plugins_by_domain = HashMap::new();
        for plugin in self.builtin_plugins.values() {
            let metadata = plugin.metadata();
            let domain = domain_key(&metadata.domain);
            plugins_by_domain.entry(domain).or_insert(RegisteredPlugin {
                enabled: !self.is_domain_disabled(&metadata.domain),
                metadata,
                source: PluginSource::Builtin,
            });
        }
        for plugin in self.dynamic_plugins.values() {
            let metadata = plugin.metadata.clone();
            let domain = domain_key(&metadata.domain);
            plugins_by_domain.insert(
                domain,
                RegisteredPlugin {
                    enabled: !self.is_domain_disabled(&metadata.domain),
                    metadata,
                    source: PluginSource::Dynamic,
                },
            );
        }
        let mut plugins = Vec::from_iter(plugins_by_domain.into_values());
        plugins.sort_by(|left, right| {
            left.metadata
                .domain
                .cmp(&right.metadata.domain)
                .then_with(|| left.metadata.plugin_name.cmp(&right.metadata.plugin_name))
        });
        plugins
    }

    pub fn collect_plugin_manuals(&self) -> Vec<PluginManual> {
        let mut manuals = Vec::new();
        for plugin in self.list_registered_plugins() {
            if !plugin.enabled {
                continue;
            }
            let domain = domain_key(&plugin.metadata.domain);
            match plugin.source {
                PluginSource::Builtin => {
                    let plugin = self
                        .builtin_plugins
                        .get(&domain)
                        .expect("builtin plugin should exist for listed domain");
                    manuals.push(plugin.manual());
                }
                PluginSource::Dynamic => {
                    let plugin = self
                        .dynamic_plugins
                        .get(&domain)
                        .expect("dynamic plugin should exist for listed domain");
                    match plugin.manual() {
                        Ok(Some(manual)) => manuals.push(manual),
                        Ok(None) => manuals.push(fallback_manual(
                            &plugin.metadata,
                            "manual is not provided by this dynamic plugin".to_owned(),
                        )),
                        Err(error) => manuals.push(fallback_manual(
                            &plugin.metadata,
                            format!("failed to load plugin manual: {error}"),
                        )),
                    }
                }
            }
        }
        manuals.sort_by(|left, right| left.domain.cmp(&right.domain));
        manuals
    }

    pub fn list_enabled_plugins(&self) -> Vec<PluginMetadata> {
        self.list_registered_plugins()
            .into_iter()
            .filter(|plugin| plugin.enabled)
            .map(|plugin| plugin.metadata)
            .collect()
    }
}

impl Default for PluginManager {
    fn default() -> Self {
        Self::new()
    }
}

fn push_dynamic_plugin_conflicts(
    report: &mut PluginLoadReport,
    domain_key: &str,
    new_metadata: &PluginMetadata,
    existing_dynamic: Option<&PluginMetadata>,
    existing_builtin: Option<PluginMetadata>,
) {
    if let Some(existing) = existing_dynamic {
        if !is_same_plugin_identity(new_metadata, existing) {
            report.push_conflict(
                new_metadata.clone(),
                existing.clone(),
                PluginSource::Dynamic,
                PluginSource::Dynamic,
                format!(
                    "multiple dynamic plugins for domain '{domain_key}', last loaded takes precedence",
                ),
            );
        }
    }
    if let Some(existing) = existing_builtin {
        report.push_conflict(
            new_metadata.clone(),
            existing,
            PluginSource::Dynamic,
            PluginSource::Builtin,
            format!("dynamic plugin shadows builtin plugin for domain '{domain_key}'"),
        );
    }
}

fn is_same_plugin_identity(left: &PluginMetadata, right: &PluginMetadata) -> bool {
    left.plugin_name == right.plugin_name
        && left.domain.eq_ignore_ascii_case(&right.domain)
        && left.description == right.description
        && left.abi_version == right.abi_version
        && left.compatibility == right.compatibility
}

struct DynamicPlugin {
    _library: Library,
    metadata: PluginMetadata,
    invoke_json: unsafe extern "C" fn(*const c_char) -> *mut c_char,
    manual_json: Option<unsafe extern "C" fn() -> *mut c_char>,
    free_c_string: unsafe extern "C" fn(*mut c_char),
}

impl DynamicPlugin {
    fn load(path: PathBuf) -> Result<Self, RuntimeError> {
        let library =
            unsafe { Library::new(&path) }.map_err(|source| RuntimeError::LibraryLoad {
                path: path.clone(),
                source,
            })?;
        let entry = unsafe { library.get::<AhPluginEntryV1>(AH_PLUGIN_ENTRY_V1_SYMBOL) }.map_err(
            |source| RuntimeError::SymbolLoad {
                path: path.clone(),
                source,
            },
        )?;
        let api_ptr = unsafe { entry() };
        if api_ptr.is_null() {
            return Err(RuntimeError::InvalidMetadata {
                path,
                reason: "null plugin api pointer".to_owned(),
            });
        }
        let api = unsafe { &*api_ptr };
        if api.abi_version != AH_PLUGIN_ABI_VERSION {
            return Err(RuntimeError::AbiVersionMismatch {
                path,
                found: api.abi_version,
                expected: AH_PLUGIN_ABI_VERSION,
            });
        }

        let plugin_name = unsafe { c_ptr_to_string(api.plugin_name) }.map_err(|reason| {
            RuntimeError::InvalidMetadata {
                path: path.clone(),
                reason,
            }
        })?;
        let domain = unsafe { c_ptr_to_string(api.domain) }.map_err(|reason| {
            RuntimeError::InvalidMetadata {
                path: path.clone(),
                reason,
            }
        })?;
        let description = unsafe { c_ptr_to_string(api.description) }.map_err(|reason| {
            RuntimeError::InvalidMetadata {
                path: path.clone(),
                reason,
            }
        })?;
        let manual_json =
            unsafe { library.get::<AhPluginManualJsonV1>(AH_PLUGIN_MANUAL_JSON_V1_SYMBOL) }
                .ok()
                .map(|symbol| *symbol);
        let metadata_json =
            unsafe { library.get::<AhPluginMetadataJsonV1>(AH_PLUGIN_METADATA_JSON_V1_SYMBOL) }
                .ok()
                .map(|symbol| *symbol);
        let metadata = if let Some(metadata_json) = metadata_json {
            let metadata_ptr = unsafe { metadata_json() };
            if metadata_ptr.is_null() {
                return Err(RuntimeError::InvalidMetadata {
                    path,
                    reason: "metadata JSON symbol returned null".to_owned(),
                });
            }
            let metadata_raw = unsafe { c_ptr_to_string(metadata_ptr.cast_const()) };
            unsafe { (api.free_c_string)(metadata_ptr) };
            let metadata_raw = metadata_raw.map_err(|reason| RuntimeError::InvalidMetadata {
                path: path.clone(),
                reason,
            })?;
            let metadata =
                serde_json::from_str::<PluginMetadata>(&metadata_raw).map_err(|error| {
                    RuntimeError::InvalidMetadata {
                        path: path.clone(),
                        reason: format!("metadata JSON parse failed: {error}"),
                    }
                })?;
            validate_plugin_metadata_contract(
                &path,
                api.abi_version,
                &plugin_name,
                &domain,
                &description,
                &metadata,
                manual_json.is_some(),
            )?;
            metadata
        } else {
            PluginMetadata {
                plugin_name,
                domain,
                description,
                abi_version: api.abi_version,
                required_tools: Vec::new(),
                compatibility: Default::default(),
            }
        };
        validate_plugin_api_contract(&path, &metadata, manual_json.is_some())?;

        Ok(Self {
            _library: library,
            metadata,
            invoke_json: api.invoke_json,
            manual_json,
            free_c_string: api.free_c_string,
        })
    }

    fn invoke(&self, request: &InvocationRequest) -> Result<InvocationResponse, RuntimeError> {
        let request_json = serde_json::to_string(request).map_err(|error| {
            RuntimeError::Invocation(format!("request serialization failed: {error}"))
        })?;
        let c_request = CString::new(request_json).map_err(|error| {
            RuntimeError::Invocation(format!("invalid request cstring: {error}"))
        })?;

        let response_ptr = unsafe { (self.invoke_json)(c_request.as_ptr()) };
        if response_ptr.is_null() {
            return Err(RuntimeError::Invocation(
                "plugin returned null response".to_owned(),
            ));
        }
        let response_raw = unsafe {
            let decoded = c_ptr_to_string(response_ptr);
            (self.free_c_string)(response_ptr);
            decoded
        }
        .map_err(RuntimeError::ResponseParse)?;

        serde_json::from_str::<InvocationResponse>(&response_raw)
            .map_err(|error| RuntimeError::ResponseParse(error.to_string()))
    }

    fn manual(&self) -> Result<Option<PluginManual>, RuntimeError> {
        let Some(manual_json) = self.manual_json else {
            return Ok(None);
        };

        let response_ptr = unsafe { manual_json() };
        if response_ptr.is_null() {
            return Ok(None);
        }
        let response_raw = unsafe {
            let decoded = c_ptr_to_string(response_ptr);
            (self.free_c_string)(response_ptr);
            decoded
        }
        .map_err(RuntimeError::ResponseParse)?;
        let manual = serde_json::from_str::<PluginManual>(&response_raw)
            .map_err(|error| RuntimeError::ResponseParse(error.to_string()))?;
        Ok(Some(manual))
    }
}

fn validate_plugin_metadata_contract(
    path: &Path,
    abi_version: u32,
    plugin_name: &str,
    domain: &str,
    description: &str,
    metadata: &PluginMetadata,
    manual_json_available: bool,
) -> Result<(), RuntimeError> {
    if metadata.plugin_name != plugin_name {
        return Err(RuntimeError::InvalidMetadata {
            path: path.to_path_buf(),
            reason: format!(
                "metadata plugin_name '{}' does not match ABI plugin_name '{}'",
                metadata.plugin_name, plugin_name
            ),
        });
    }
    if metadata.domain != domain {
        return Err(RuntimeError::InvalidMetadata {
            path: path.to_path_buf(),
            reason: format!(
                "metadata domain '{}' does not match ABI domain '{}'",
                metadata.domain, domain
            ),
        });
    }
    if metadata.description != description {
        return Err(RuntimeError::InvalidMetadata {
            path: path.to_path_buf(),
            reason: "metadata description does not match ABI description".to_owned(),
        });
    }
    if metadata.abi_version != abi_version {
        return Err(RuntimeError::InvalidMetadata {
            path: path.to_path_buf(),
            reason: format!(
                "metadata abi_version {} does not match ABI version {}",
                metadata.abi_version, abi_version
            ),
        });
    }
    validate_plugin_api_contract(path, metadata, manual_json_available)
}

fn validate_plugin_api_contract(
    path: &Path,
    metadata: &PluginMetadata,
    manual_json_available: bool,
) -> Result<(), RuntimeError> {
    if !metadata.is_api_compatible_with_host() {
        return Err(RuntimeError::ApiVersionMismatch {
            path: path.to_path_buf(),
            found_major: metadata.compatibility.api_version.major,
            found_minor: metadata.compatibility.api_version.minor,
            supported_major: AH_PLUGIN_API_MAJOR_VERSION,
            supported_minor: AH_PLUGIN_API_MINOR_VERSION,
        });
    }
    if metadata.supports_capability(plugin_capabilities::MANUAL_JSON) && !manual_json_available {
        return Err(RuntimeError::InvalidMetadata {
            path: path.to_path_buf(),
            reason: format!(
                "metadata declares '{}' capability but '{}' symbol is missing",
                plugin_capabilities::MANUAL_JSON,
                String::from_utf8_lossy(AH_PLUGIN_MANUAL_JSON_V1_SYMBOL).trim_end_matches('\0')
            ),
        });
    }
    Ok(())
}

fn is_dynamic_lib_file(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|ext| ext.to_str()),
        Some("dll") | Some("so") | Some("dylib")
    )
}

fn fallback_manual(metadata: &PluginMetadata, note: String) -> PluginManual {
    PluginManual {
        plugin_name: metadata.plugin_name.clone(),
        domain: metadata.domain.clone(),
        description: metadata.description.clone(),
        commands: Vec::new(),
        notes: vec![note],
    }
}

fn preflight_required_tools(
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

fn infer_operation(domain: &str, argv: &[String]) -> Option<String> {
    argv.first().map(|command| format!("{domain}.{command}"))
}

fn domain_key(domain: &str) -> String {
    domain.trim().to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::Path,
        sync::Arc,
        time::{SystemTime, UNIX_EPOCH},
    };

    use ah_plugin_api::GlobalOptionsWire;

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

    impl BuiltinPlugin for EchoBuiltinPlugin {
        fn metadata(&self) -> PluginMetadata {
            PluginMetadata {
                plugin_name: "builtin-echo".to_owned(),
                domain: "echo".to_owned(),
                description: "echo test plugin".to_owned(),
                abi_version: AH_PLUGIN_ABI_VERSION,
                required_tools: Vec::new(),
                compatibility: Default::default(),
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
                },
            )
            .expect("builtin plugin should still be invokable");
        assert!(response.success);
        assert_eq!(response.message.as_deref(), Some("ok"));

        fs::remove_dir_all(&dir).expect("temp plugin dir should be removed");
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

        let error = validate_plugin_api_contract(Path::new("plugin.dll"), &metadata, false)
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
        )
        .expect_err("ABI mismatch should fail");
        assert!(abi_error.to_string().contains("abi_version"));
    }

    #[test]
    fn plugin_api_contract_rejects_missing_manual_symbol_when_capability_declared() {
        let mut metadata = test_metadata("external-echo", "echo");
        metadata.compatibility = ah_plugin_api::PluginCompatibility::current()
            .with_capability(plugin_capabilities::MANUAL_JSON);

        let error = validate_plugin_api_contract(Path::new("plugin.dll"), &metadata, false)
            .expect_err("declared manual_json capability without symbol should fail");
        assert!(error.to_string().contains(plugin_capabilities::MANUAL_JSON));
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
}
