//! Network- and filesystem-independent policy core for AIHelper self-update.

pub use ah_release_manifest::{
    DETACHED_SIGNATURE_BYTES, FilePurpose, MAX_MANIFEST_BYTES, ManagedFile, ReleaseManifest,
};

mod check;
mod error;
mod helper;
mod installation;
mod plan;
mod production;
mod release;
mod trust;

pub use check::{
    UPDATE_RESULT_SCHEMA_VERSION, UpdateSource, UpgradeCheckResultV1, VerifiedReleaseV1,
    verify_discovered_release, verify_discovered_release_for_check,
};
pub use error::{UpdaterError, UpdaterErrorCode};
pub use helper::{
    UPDATE_HELPER_PROTOCOL_VERSION, UPDATE_HELPER_SELF_CHECK_SCHEMA_VERSION,
    UpdateHelperSelfCheckV1,
};
pub use installation::{
    INSTALLATION_IDENTITY_SCHEMA_VERSION, InstallationIdentityV1, lifecycle_mutex_name,
};
pub use plan::{
    ManagedFileOperationV1, TRANSACTION_JOURNAL_SCHEMA_VERSION, TRANSACTION_PLAN_SCHEMA_VERSION,
    TransactionJournalV1, TransactionPlanV1, TransactionStateV1, encode_digest,
};
pub use production::production_release_trust;
pub use release::{
    CheckStatus, DiscoveredReleaseV1, GitHubAssetDtoV1, GitHubReleaseDtoV1, ReleaseAssetV1,
    ReleaseAssetsV1, StableReleaseVersion, UpdateOperation, UpdateTarget, WINDOWS_X64_TARGET,
    select_highest_stable_release, select_stable_release_by_version,
};
pub use trust::{ReleaseTrust, ReleaseTrustAnchor};
