//! Strict schema and verification contract for AIHelper release manifests.
//!
//! ADR: Canonical Ed25519 release manifest v1.
//! See: `../../docs/decisions/2026-07-22-adopt-canonical-ed25519-release-manifest-v1.md`.

mod canonical;
mod error;
mod model;
mod validation;

pub use error::ManifestError;
pub use model::{
    ArchiveMetadata, FilePurpose, ManagedFile, ReleaseManifest, ReleaseMetadata, RequiredFiles,
    SignatureAlgorithm, SigningMetadata,
};

pub const SCHEMA_VERSION: u32 = 1;
pub const MAX_MANIFEST_BYTES: usize = 1024 * 1024;
pub const MAX_MANAGED_FILES: usize = 4096;
pub const MAX_MANAGED_PATH_BYTES: usize = 512;
pub const SIGNING_ALGORITHM: &str = "ed25519";
pub const SUPPORTED_TARGETS: &[(&str, &str)] = &[
    ("x86_64-pc-windows-msvc", "x86_64"),
    ("x86_64-unknown-linux-gnu", "x86_64"),
    ("aarch64-apple-darwin", "aarch64"),
];
