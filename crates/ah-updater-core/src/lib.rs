//! Network- and filesystem-independent policy core for AIHelper self-update.

mod error;
mod release;
mod trust;

pub use error::{UpdaterError, UpdaterErrorCode};
pub use release::{
    CheckStatus, GitHubAssetDtoV1, GitHubReleaseDtoV1, StableReleaseVersion, UpdateOperation,
    UpdateTarget, WINDOWS_X64_TARGET,
};
pub use trust::ReleaseTrust;
