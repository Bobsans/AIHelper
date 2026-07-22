//! Release-only archive validation and signing support for AIHelper CI.
//!
//! This package is intentionally not a dependency of the shipped `ah` binary.

mod archive;
mod error;
mod key;
mod profile;
mod release;

pub use archive::{ArchiveFile, ArchiveInventory, validate_archive, validate_archive_set};
pub use error::ReleaseToolError;
pub use key::SigningMaterial;
pub use profile::{PLUGIN_DOMAINS, RELEASE_PROFILES, ReleaseProfile};
pub use release::{ReleaseRequest, sign_release_set};
