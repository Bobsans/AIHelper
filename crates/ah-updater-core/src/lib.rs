//! Network- and filesystem-independent policy core for AIHelper self-update.

pub use ah_release_manifest::DETACHED_SIGNATURE_BYTES;

mod error;
mod release;
mod trust;

pub use error::{UpdaterError, UpdaterErrorCode};
pub use release::{
    CheckStatus, DiscoveredReleaseV1, GitHubAssetDtoV1, GitHubReleaseDtoV1, ReleaseAssetV1,
    ReleaseAssetsV1, StableReleaseVersion, UpdateOperation, UpdateTarget, WINDOWS_X64_TARGET,
    select_highest_stable_release,
};
pub use trust::ReleaseTrust;
