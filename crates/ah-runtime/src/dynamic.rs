//! A plugin loaded from a shared library, and the contract it has to satisfy
//! before a single one of its symbols is called.

use super::*;

pub(super) struct DynamicPlugin {
    pub(super) _library: Library,
    pub(super) metadata: PluginMetadata,
    pub(super) invoke_json: unsafe extern "C" fn(*const c_char) -> *mut c_char,
    pub(super) manual_json: Option<unsafe extern "C" fn() -> *mut c_char>,
    pub(super) command_catalog: Option<CommandCatalog>,
    pub(super) invoke_command_json: Option<AhPluginInvokeCommandJsonV1>,
    pub(super) cancel_command: Option<AhPluginCancelCommandV1>,
    pub(super) free_c_string: unsafe extern "C" fn(*mut c_char),
}

impl DynamicPlugin {
    pub(super) fn load(path: PathBuf) -> Result<Self, RuntimeError> {
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
        let command_catalog_json = unsafe {
            library.get::<AhPluginCommandCatalogJsonV1>(AH_PLUGIN_COMMAND_CATALOG_JSON_V1_SYMBOL)
        }
        .ok()
        .map(|symbol| *symbol);
        let invoke_command_json = unsafe {
            library.get::<AhPluginInvokeCommandJsonV1>(AH_PLUGIN_INVOKE_COMMAND_JSON_V1_SYMBOL)
        }
        .ok()
        .map(|symbol| *symbol);
        let cancel_command =
            unsafe { library.get::<AhPluginCancelCommandV1>(AH_PLUGIN_CANCEL_COMMAND_V1_SYMBOL) }
                .ok()
                .map(|symbol| *symbol);
        let typed_symbols = TypedSymbolAvailability {
            catalog: command_catalog_json.is_some(),
            invoke: invoke_command_json.is_some(),
            cancel: cancel_command.is_some(),
        };
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
                typed_symbols,
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
        validate_plugin_api_contract(&path, &metadata, manual_json.is_some(), typed_symbols)?;
        let command_catalog =
            if metadata.supports_capability(plugin_capabilities::TYPED_COMMANDS_V1) {
                let command_catalog_json =
                    command_catalog_json.expect("typed catalog symbol should be validated");
                let response_ptr = unsafe { command_catalog_json() };
                if response_ptr.is_null() {
                    return Err(RuntimeError::InvalidMetadata {
                        path,
                        reason: "typed command catalog symbol returned null".to_owned(),
                    });
                }
                let response_raw = unsafe {
                    let decoded = c_ptr_to_string(response_ptr.cast_const());
                    (api.free_c_string)(response_ptr);
                    decoded
                }
                .map_err(|reason| RuntimeError::InvalidMetadata {
                    path: path.clone(),
                    reason,
                })?;
                let catalog =
                    serde_json::from_str::<CommandCatalog>(&response_raw).map_err(|error| {
                        RuntimeError::InvalidMetadata {
                            path: path.clone(),
                            reason: format!("typed command catalog parse failed: {error}"),
                        }
                    })?;
                typed::validate_catalog(&metadata, &catalog)?;
                Some(catalog)
            } else {
                None
            };

        Ok(Self {
            _library: library,
            metadata,
            invoke_json: api.invoke_json,
            manual_json,
            command_catalog,
            invoke_command_json,
            cancel_command,
            free_c_string: api.free_c_string,
        })
    }

    pub(super) fn invoke(
        &self,
        request: &InvocationRequest,
    ) -> Result<InvocationResponse, RuntimeError> {
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

    pub(super) fn invoke_typed(
        &self,
        request: &TypedInvocationRequest,
    ) -> Result<TypedInvocationResponse, RuntimeError> {
        let invoke_command_json = self.invoke_command_json.ok_or_else(|| {
            RuntimeError::TypedInvocation(format!(
                "plugin '{}' does not expose typed invocation",
                self.metadata.plugin_name
            ))
        })?;
        let request_json = serde_json::to_string(request).map_err(|error| {
            RuntimeError::TypedInvocation(format!("request serialization failed: {error}"))
        })?;
        let c_request = CString::new(request_json).map_err(|error| {
            RuntimeError::TypedInvocation(format!("invalid request cstring: {error}"))
        })?;
        let response_ptr = unsafe { invoke_command_json(c_request.as_ptr()) };
        if response_ptr.is_null() {
            return Err(RuntimeError::TypedInvocation(
                "plugin returned null typed response".to_owned(),
            ));
        }
        let response_raw = unsafe {
            let decoded = c_ptr_to_string(response_ptr);
            (self.free_c_string)(response_ptr);
            decoded
        }
        .map_err(RuntimeError::ResponseParse)?;
        serde_json::from_str::<TypedInvocationResponse>(&response_raw)
            .map_err(|error| RuntimeError::ResponseParse(error.to_string()))
    }

    pub(super) fn cancel_typed(&self, request_id: &str) -> bool {
        let Some(cancel_command) = self.cancel_command else {
            return false;
        };
        let Ok(request_id) = CString::new(request_id) else {
            return false;
        };
        unsafe { cancel_command(request_id.as_ptr()) != 0 }
    }

    pub(super) fn manual(&self) -> Result<Option<PluginManual>, RuntimeError> {
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

#[allow(clippy::too_many_arguments)]
pub(super) fn validate_plugin_metadata_contract(
    path: &Path,
    abi_version: u32,
    plugin_name: &str,
    domain: &str,
    description: &str,
    metadata: &PluginMetadata,
    manual_json_available: bool,
    typed_symbols: TypedSymbolAvailability,
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
    validate_plugin_api_contract(path, metadata, manual_json_available, typed_symbols)
}

pub(super) fn validate_plugin_api_contract(
    path: &Path,
    metadata: &PluginMetadata,
    manual_json_available: bool,
    typed_symbols: TypedSymbolAvailability,
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
    let declares_typed = metadata.supports_capability(plugin_capabilities::TYPED_COMMANDS_V1);
    if declares_typed && !typed_symbols.is_complete() {
        let mut missing = Vec::new();
        if !typed_symbols.catalog {
            missing.push(
                String::from_utf8_lossy(AH_PLUGIN_COMMAND_CATALOG_JSON_V1_SYMBOL)
                    .trim_end_matches('\0')
                    .to_owned(),
            );
        }
        if !typed_symbols.invoke {
            missing.push(
                String::from_utf8_lossy(AH_PLUGIN_INVOKE_COMMAND_JSON_V1_SYMBOL)
                    .trim_end_matches('\0')
                    .to_owned(),
            );
        }
        if !typed_symbols.cancel {
            missing.push(
                String::from_utf8_lossy(AH_PLUGIN_CANCEL_COMMAND_V1_SYMBOL)
                    .trim_end_matches('\0')
                    .to_owned(),
            );
        }
        return Err(RuntimeError::InvalidMetadata {
            path: path.to_path_buf(),
            reason: format!(
                "metadata declares '{}' capability but required symbol(s) are missing: {}",
                plugin_capabilities::TYPED_COMMANDS_V1,
                missing.join(", ")
            ),
        });
    }
    if !declares_typed && typed_symbols.any() {
        return Err(RuntimeError::InvalidMetadata {
            path: path.to_path_buf(),
            reason: format!(
                "typed command symbols require '{}' capability",
                plugin_capabilities::TYPED_COMMANDS_V1
            ),
        });
    }
    Ok(())
}

pub(super) fn is_dynamic_lib_file(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|ext| ext.to_str()),
        Some("dll") | Some("so") | Some("dylib")
    )
}

pub(super) fn fallback_manual(metadata: &PluginMetadata, note: String) -> PluginManual {
    PluginManual {
        plugin_name: metadata.plugin_name.clone(),
        domain: metadata.domain.clone(),
        description: metadata.description.clone(),
        commands: Vec::new(),
        notes: vec![note],
    }
}
