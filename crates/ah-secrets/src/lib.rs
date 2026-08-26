//! The secret vault: where credentials are kept, and how a command that needs
//! one gets it.
//!
//! Encrypted at rest with a key this process never writes down: it comes from
//! the system keyring, or from an explicit master key captured once at startup.
//! A resolved secret is a value with a kind, never a string, so a command
//! cannot be handed the wrong credential by accident.

mod key_provider;
mod kinds;
mod setup;
mod store;

pub use key_provider::{
    ExplicitMasterKey, KeyProvider, SystemKeyring, capture_startup_master_key, resolve_key_provider,
};
pub use kinds::{NewSecret, ResolvedSecret, SecretKind, SecretMetadata};
pub use setup::{SecretSetupCapabilities, SecretSetupError, SecretSetupTarget, VaultSetupService};
pub use store::{VaultError, VaultStore};
