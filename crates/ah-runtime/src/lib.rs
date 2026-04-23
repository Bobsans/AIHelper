use std::{
    collections::HashMap,
    ffi::{CString, c_char},
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};

use ah_plugin_api::{
    AH_PLUGIN_ABI_VERSION, AH_PLUGIN_ENTRY_V1_SYMBOL, AhPluginEntryV1, GlobalOptionsWire,
    InvocationRequest, InvocationResponse, PluginMetadata, c_ptr_to_string,
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
    fn invoke(&self, request: &InvocationRequest) -> InvocationResponse;
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

    pub fn load_dynamic_plugins_from_dir(&mut self, dir: &Path) -> Result<usize, RuntimeError> {
        if !dir.exists() {
            return Ok(0);
        }
        let mut loaded = 0usize;

        for entry in
            fs::read_dir(dir).map_err(|error| RuntimeError::Invocation(error.to_string()))?
        {
            let entry = entry.map_err(|error| RuntimeError::Invocation(error.to_string()))?;
            let path = entry.path();
            if !path.is_file() || !is_dynamic_lib_file(&path) {
                continue;
            }
            let plugin = DynamicPlugin::load(path.clone())?;
            self.dynamic_plugins
                .insert(plugin.metadata.domain.clone(), plugin);
            loaded += 1;
        }

        Ok(loaded)
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

        Ok(Self {
            _library: library,
            metadata: PluginMetadata {
                plugin_name,
                domain,
                description,
                abi_version: api.abi_version,
            },
            invoke_json: api.invoke_json,
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
        let response_raw =
            unsafe { c_ptr_to_string(response_ptr) }.map_err(RuntimeError::ResponseParse)?;
        unsafe { (self.free_c_string)(response_ptr) };

        serde_json::from_str::<InvocationResponse>(&response_raw)
            .map_err(|error| RuntimeError::ResponseParse(error.to_string()))
    }
}

fn is_dynamic_lib_file(path: &Path) -> bool {
    match path.extension().and_then(|ext| ext.to_str()) {
        Some("dll") | Some("so") | Some("dylib") => true,
        _ => false,
    }
}
