use ah_release_manifest::ReleaseManifest;
use semver::Version;
use serde::{Deserialize, Serialize};

use crate::{
    CheckStatus, DiscoveredReleaseV1, ReleaseTrust, UpdateOperation, UpdaterError, UpdaterErrorCode,
};

pub const UPDATE_RESULT_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UpdateSource {
    GitHubRelease,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpgradeCheckResultV1 {
    pub schema_version: u32,
    pub operation: UpdateOperation,
    pub status: CheckStatus,
    pub current_version: String,
    pub selected_version: Option<String>,
    pub target: Option<String>,
    pub source: Option<UpdateSource>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedReleaseV1 {
    discovered: DiscoveredReleaseV1,
    manifest: ReleaseManifest,
    manifest_bytes: Vec<u8>,
    signature_bytes: Vec<u8>,
    check_result: UpgradeCheckResultV1,
}

impl VerifiedReleaseV1 {
    pub fn discovered(&self) -> &DiscoveredReleaseV1 {
        &self.discovered
    }

    pub fn manifest(&self) -> &ReleaseManifest {
        &self.manifest
    }

    pub fn manifest_bytes(&self) -> &[u8] {
        &self.manifest_bytes
    }

    pub fn signature_bytes(&self) -> &[u8] {
        &self.signature_bytes
    }

    pub fn check_result(&self) -> &UpgradeCheckResultV1 {
        &self.check_result
    }

    pub fn into_check_result(self) -> UpgradeCheckResultV1 {
        self.check_result
    }
}

pub fn verify_discovered_release(
    discovered: DiscoveredReleaseV1,
    manifest_bytes: &[u8],
    signature_bytes: &[u8],
    trust: &ReleaseTrust,
    current_version: &Version,
) -> Result<VerifiedReleaseV1, UpdaterError> {
    require_download_size(
        manifest_bytes.len(),
        discovered.assets.manifest.size,
        "downloaded manifest size differs from the GitHub asset declaration",
    )?;
    require_download_size(
        signature_bytes.len(),
        discovered.assets.signature.size,
        "downloaded signature size differs from the GitHub asset declaration",
    )?;

    let verified = trust.verify(manifest_bytes, signature_bytes)?;
    let manifest = verified.into_manifest();
    validate_manifest_identity(&discovered, &manifest, current_version)?;

    let selected_version = discovered.version.version();
    let status = match current_version.cmp(selected_version) {
        std::cmp::Ordering::Less => CheckStatus::UpdateAvailable,
        std::cmp::Ordering::Equal => CheckStatus::UpToDate,
        std::cmp::Ordering::Greater => CheckStatus::CurrentNewer,
    };
    let check_result = UpgradeCheckResultV1 {
        schema_version: UPDATE_RESULT_SCHEMA_VERSION,
        operation: UpdateOperation::Check,
        status,
        current_version: current_version.to_string(),
        selected_version: Some(selected_version.to_string()),
        target: Some(discovered.target.rust_target.to_owned()),
        source: Some(UpdateSource::GitHubRelease),
    };
    Ok(VerifiedReleaseV1 {
        discovered,
        manifest,
        manifest_bytes: manifest_bytes.to_vec(),
        signature_bytes: signature_bytes.to_vec(),
        check_result,
    })
}

pub fn verify_discovered_release_for_check(
    discovered: &DiscoveredReleaseV1,
    manifest_bytes: &[u8],
    signature_bytes: &[u8],
    trust: &ReleaseTrust,
    current_version: &Version,
) -> Result<UpgradeCheckResultV1, UpdaterError> {
    verify_discovered_release(
        discovered.clone(),
        manifest_bytes,
        signature_bytes,
        trust,
        current_version,
    )
    .map(VerifiedReleaseV1::into_check_result)
}

fn validate_manifest_identity(
    discovered: &DiscoveredReleaseV1,
    manifest: &ReleaseManifest,
    current_version: &Version,
) -> Result<(), UpdaterError> {
    if manifest.release.version != discovered.version.version().to_string()
        || discovered.tag != format!("v{}", manifest.release.version)
    {
        return Err(release_contract(
            "signed manifest version does not match the selected GitHub release",
        ));
    }
    discovered
        .target
        .require_manifest_identity(&manifest.release.target, &manifest.release.architecture)?;
    if manifest.archive.url != discovered.assets.archive.browser_download_url
        || manifest.archive.size != discovered.assets.archive.size
    {
        return Err(release_contract(
            "signed manifest archive does not match the selected GitHub release asset",
        ));
    }
    let minimum_updater = Version::parse(&manifest.minimum_updater_version)
        .map_err(|_| release_contract("signed manifest minimum updater version is invalid"))?;
    if current_version < &minimum_updater {
        return Err(UpdaterError::new(
            UpdaterErrorCode::Compatibility,
            "current updater is older than the signed release compatibility floor",
        ));
    }
    Ok(())
}

fn require_download_size(
    actual: usize,
    expected: u64,
    detail: &'static str,
) -> Result<(), UpdaterError> {
    if u64::try_from(actual).ok() == Some(expected) {
        Ok(())
    } else {
        Err(release_contract(detail))
    }
}

fn release_contract(detail: &'static str) -> UpdaterError {
    UpdaterError::new(UpdaterErrorCode::ReleaseContract, detail)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn optional_fields_serialize_as_explicit_nulls() {
        let result = UpgradeCheckResultV1 {
            schema_version: UPDATE_RESULT_SCHEMA_VERSION,
            operation: UpdateOperation::Check,
            status: CheckStatus::UpToDate,
            current_version: "1.1.0".to_owned(),
            selected_version: None,
            target: None,
            source: None,
        };

        let json = serde_json::to_value(result).unwrap();
        assert!(json["selected_version"].is_null());
        assert!(json["target"].is_null());
        assert!(json["source"].is_null());
    }
}
