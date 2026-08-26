//! The plugin runtime: what exists, what may run, and what running it reported.
//!
//! This file is the crate's wiring. The 1826 production lines it used to hold
//! were one registry, one invocation path, one dynamic-library loader and one
//! error type stacked in a single file, with a 1173-line test module under
//! them:
//!
//! | Module     | Owns                                                       |
//! |------------|------------------------------------------------------------|
//! | `error`    | `RuntimeError` and the diagnostics it renders into          |
//! | `ports`    | what the runtime needs from its host                       |
//! | `outcome`  | what an invocation and a discovery pass report              |
//! | `manager`  | the registry: which plugins and commands exist              |
//! | `invoke`   | running one: routing, credentials, typed requests, cancel   |
//! | `dynamic`  | a shared-library plugin and the contract it must satisfy    |

use std::{
    borrow::Cow,
    collections::{BTreeMap, HashMap, HashSet},
    ffi::{CString, c_char},
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc, RwLock,
        atomic::{AtomicU64, Ordering},
    },
};

use ah_plugin_api::{
    AH_PLUGIN_ABI_VERSION, AH_PLUGIN_API_MAJOR_VERSION, AH_PLUGIN_API_MINOR_VERSION,
    AH_PLUGIN_CANCEL_COMMAND_V1_SYMBOL, AH_PLUGIN_COMMAND_CATALOG_JSON_V1_SYMBOL,
    AH_PLUGIN_ENTRY_V1_SYMBOL, AH_PLUGIN_INVOKE_COMMAND_JSON_V1_SYMBOL,
    AH_PLUGIN_MANUAL_JSON_V1_SYMBOL, AH_PLUGIN_METADATA_JSON_V1_SYMBOL, AhPluginCancelCommandV1,
    AhPluginCommandCatalogJsonV1, AhPluginEntryV1, AhPluginInvokeCommandJsonV1,
    AhPluginManualJsonV1, AhPluginMetadataJsonV1, CommandCatalog, CommandDescriptor, CommandError,
    ErrorDiagnostic, GlobalOptionsWire, InvocationRequest, InvocationResponse, PluginManual,
    PluginMetadata, RequiredTool, ResolvedSecret, SecretSlot, TypedInvocationRequest,
    TypedInvocationResponse, c_ptr_to_string, plugin_capabilities,
};
use libloading::Library;
use thiserror::Error;

mod dynamic;
mod error;
mod invoke;
mod manager;
mod outcome;
mod ports;
use dynamic::{DynamicPlugin, fallback_manual, is_dynamic_lib_file};
pub use error::RuntimeError;
use invoke::domain_key;
pub use manager::{PluginManager, RegisteredCommand, RegisteredPlugin};
use manager::{
    RegisteredTypedCommand, TypedCommandRoute, TypedRegistry, TypedSymbolAvailability,
    append_typed_catalog,
};
pub use outcome::{
    InvocationObservation, InvocationOutcome, PluginLoadConflict, PluginLoadReport,
    PluginLoadWarning, PluginSource, RunCheckOutcome,
};
pub use ports::{BuiltinPlugin, SecretResolver, SecretResolverError};
// Reached only from `tests`, which globs this module, not its children.
#[cfg(test)]
use dynamic::{validate_plugin_api_contract, validate_plugin_metadata_contract};
// Reached only from `tests`, which globs this module, not its children.
#[cfg(test)]
use manager::push_dynamic_plugin_conflicts;

pub mod core;

pub mod executor;

pub mod typed;

#[cfg(test)]
mod tests;
