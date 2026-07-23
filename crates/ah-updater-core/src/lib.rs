//! Network- and filesystem-independent policy core for AIHelper self-update.

pub use ah_release_manifest::{
    DETACHED_SIGNATURE_BYTES, FilePurpose, MAX_MANIFEST_BYTES, ManagedFile, ReleaseManifest,
};

mod check;
mod error;
mod installation;
mod release;
mod trust;

pub use check::{
    UPDATE_RESULT_SCHEMA_VERSION, UpdateSource, UpgradeCheckResultV1, VerifiedReleaseV1,
    verify_discovered_release, verify_discovered_release_for_check,
};
pub use error::{UpdaterError, UpdaterErrorCode};
pub use installation::{INSTALLATION_IDENTITY_SCHEMA_VERSION, InstallationIdentityV1};
pub use release::{
    CheckStatus, DiscoveredReleaseV1, GitHubAssetDtoV1, GitHubReleaseDtoV1, ReleaseAssetV1,
    ReleaseAssetsV1, StableReleaseVersion, UpdateOperation, UpdateTarget, WINDOWS_X64_TARGET,
    select_highest_stable_release, select_stable_release_by_version,
};
pub use trust::{ReleaseTrust, ReleaseTrustAnchor};
