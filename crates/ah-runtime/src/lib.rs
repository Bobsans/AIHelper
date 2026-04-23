use std::{
    collections::HashMap,
    ffi::{CString, c_char},
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};

use ah_plugin_api::{
    AH_PLUGIN_ABI_VERSION, AH_PLUGIN_ENTRY_V1_SYMBOL, AH_PLUGIN_MANUAL_JSON_V1_SYMBOL,
    AhPluginEntryV1, AhPluginManualJsonV1, GlobalOptionsWire, InvocationRequest,
    InvocationResponse, PluginManual, PluginMetadata, c_ptr_to_string,
};
use libloading::Library;
use thiserror::Error;

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
    #[error("plugin at {path:?} returned invalid metadata: {reason}")]
    InvalidMetadata { path: PathBuf, reason: String },
    #[error("plugin invocation failed: {0}")]
    Invocation(String),
    #[error("plugin response parse failed: {0}")]
    ResponseParse(String),
}

pub trait BuiltinPlugin: Send + Sync {
    fn metadata(&self) -> PluginMetadata;
    fn manual(&self) -> PluginManual;
    fn invoke(&self, request: &InvocationRequest) -> InvocationResponse;
}

#[derive(Debug, Clone)]
pub struct PluginLoadWarning {
    pub path: PathBuf,
    pub error: String,
}

#[derive(Debug, Default, Clone)]
pub struct PluginLoadReport {
    pub loaded: usize,
    pub skipped: usize,
    pub warnings: Vec<PluginLoadWarning>,
}

impl PluginLoadReport {
    fn push_warning(&mut self, path: PathBuf, error: impl Into<String>) {
        self.skipped += 1;
        self.warnings.push(PluginLoadWarning {
            path,
            error: error.into(),
        });
    }
}

pub struct PluginManager {
    dynamic_plugins: HashMap<String, DynamicPlugin>,
    builtin_plugins: HashMap<String, Arc<dyn BuiltinPlugin>>,
}

impl PluginManager {
    pub fn new() -> Self {
        Self {
            dynamic_plugins: HashMap::new(),
            builtin_plugins: HashMap::new(),
        }
    }

    pub fn register_builtin(&mut self, plugin: Arc<dyn BuiltinPlugin>) {
        self.builtin_plugins
            .insert(plugin.metadata().domain.clone(), plugin);
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
            match DynamicPlugin::load(path.clone()) {
                Ok(plugin) => {
                    self.dynamic_plugins
                        .insert(plugin.metadata.domain.clone(), plugin);
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
        let request = InvocationRequest {
            domain: domain.to_owned(),
            argv,
            globals,
        };

        if let Some(plugin) = self.dynamic_plugins.get(domain) {
            return plugin.invoke(&request);
        }
        if let Some(plugin) = self.builtin_plugins.get(domain) {
            return Ok(plugin.invoke(&request));
        }

        Err(RuntimeError::DomainNotFound(domain.to_owned()))
    }

    pub fn list_plugins(&self) -> Vec<PluginMetadata> {
        let mut plugins = Vec::new();
        for plugin in self.builtin_plugins.values() {
            plugins.push(plugin.metadata());
        }
        for plugin in self.dynamic_plugins.values() {
            plugins.push(plugin.metadata.clone());
        }
        plugins.sort_by(|left, right| left.domain.cmp(&right.domain));
        plugins
    }

    pub fn collect_plugin_manuals(&self) -> Vec<PluginManual> {
        let mut manuals = Vec::new();
        for plugin in self.builtin_plugins.values() {
            manuals.push(plugin.manual());
        }
        for plugin in self.dynamic_plugins.values() {
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
        manuals.sort_by(|left, right| left.domain.cmp(&right.domain));
        manuals
    }
}

impl Default for PluginManager {
    fn default() -> Self {
        Self::new()
    }
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

        Ok(Self {
            _library: library,
            metadata: PluginMetadata {
                plugin_name,
                domain,
                description,
                abi_version: api.abi_version,
            },
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

fn is_dynamic_lib_file(path: &Path) -> bool {
    match path.extension().and_then(|ext| ext.to_str()) {
        Some("dll") | Some("so") | Some("dylib") => true,
        _ => false,
    }
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

#[cfg(test)]
mod tests {
    use std::{
        fs,
        sync::Arc,
        time::{SystemTime, UNIX_EPOCH},
    };

    use ah_plugin_api::GlobalOptionsWire;

    use super::*;

    struct EchoBuiltinPlugin;

    impl BuiltinPlugin for EchoBuiltinPlugin {
        fn metadata(&self) -> PluginMetadata {
            PluginMetadata {
                plugin_name: "builtin-echo".to_owned(),
                domain: "echo".to_owned(),
                description: "echo test plugin".to_owned(),
                abi_version: AH_PLUGIN_ABI_VERSION,
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
