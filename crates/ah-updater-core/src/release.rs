use std::collections::{BTreeMap, BTreeSet};

use ah_release_manifest::{DETACHED_SIGNATURE_BYTES, MAX_MANIFEST_BYTES};
use semver::Version;
use serde::{Deserialize, Serialize};
use url::Url;

use crate::{UpdaterError, UpdaterErrorCode};

pub const WINDOWS_X64_TARGET: UpdateTarget = UpdateTarget {
    rust_target: "x86_64-pc-windows-msvc",
    architecture: "x86_64",
    archive_name: "ah-windows-x64.zip",
    manifest_name: "ah-windows-x64.manifest.json",
    signature_name: "ah-windows-x64.manifest.sig",
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UpdateTarget {
    pub rust_target: &'static str,
    pub architecture: &'static str,
    pub archive_name: &'static str,
    pub manifest_name: &'static str,
    pub signature_name: &'static str,
}

impl UpdateTarget {
    pub fn current() -> Result<Self, UpdaterError> {
        #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
        {
            Ok(WINDOWS_X64_TARGET)
        }
        #[cfg(not(all(target_os = "windows", target_arch = "x86_64")))]
        {
            Err(UpdaterError::new(
                UpdaterErrorCode::UnsupportedPlatform,
                "self-update requires Windows x86_64",
            ))
        }
    }

    pub fn require_manifest_identity(
        self,
        rust_target: &str,
        architecture: &str,
    ) -> Result<(), UpdaterError> {
        if rust_target == self.rust_target && architecture == self.architecture {
            return Ok(());
        }
        Err(UpdaterError::new(
            UpdaterErrorCode::Compatibility,
            "release manifest target is incompatible with this updater",
        ))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct StableReleaseVersion(Version);

impl StableReleaseVersion {
    pub fn parse_tag(tag: &str) -> Result<Self, UpdaterError> {
        let raw = tag.strip_prefix('v').ok_or_else(|| {
            UpdaterError::new(
                UpdaterErrorCode::ReleaseContract,
                "release tag must be 'v<SemVer>'",
            )
        })?;
        let version = Version::parse(raw).map_err(|_| {
            UpdaterError::new(
                UpdaterErrorCode::ReleaseContract,
                "release tag must contain canonical SemVer",
            )
        })?;
        if version.to_string() != raw {
            return Err(UpdaterError::new(
                UpdaterErrorCode::ReleaseContract,
                "release tag must contain canonical SemVer",
            ));
        }
        if !version.pre.is_empty() {
            return Err(UpdaterError::new(
                UpdaterErrorCode::ReleaseContract,
                "release tag must identify a stable version",
            ));
        }
        Ok(Self(version))
    }

    pub fn version(&self) -> &Version {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct GitHubReleaseDtoV1 {
    pub id: u64,
    pub tag_name: String,
    #[serde(default)]
    pub draft: bool,
    #[serde(default)]
    pub prerelease: bool,
    #[serde(default)]
    pub assets: Vec<GitHubAssetDtoV1>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct GitHubAssetDtoV1 {
    pub id: u64,
    pub name: String,
    pub state: String,
    pub size: u64,
    pub url: String,
    pub browser_download_url: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseAssetV1 {
    pub id: u64,
    pub name: String,
    pub size: u64,
    pub api_url: String,
    pub browser_download_url: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseAssetsV1 {
    pub archive: ReleaseAssetV1,
    pub manifest: ReleaseAssetV1,
    pub signature: ReleaseAssetV1,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredReleaseV1 {
    pub release_id: u64,
    pub tag: String,
    pub version: StableReleaseVersion,
    pub target: UpdateTarget,
    pub assets: ReleaseAssetsV1,
}

pub fn select_highest_stable_release(
    releases: &[GitHubReleaseDtoV1],
    target: UpdateTarget,
) -> Result<DiscoveredReleaseV1, UpdaterError> {
    let mut release_ids = BTreeSet::new();
    let mut precedence = BTreeMap::new();
    let mut selected: Option<(&GitHubReleaseDtoV1, StableReleaseVersion)> = None;

    for release in releases {
        if release.id == 0 || !release_ids.insert(release.id) {
            return Err(release_contract(
                "GitHub release listing contains an invalid or duplicate release ID",
            ));
        }
        if release.draft || release.prerelease {
            continue;
        }

        let version = StableReleaseVersion::parse_tag(&release.tag_name)?;
        let key = (
            version.version().major,
            version.version().minor,
            version.version().patch,
        );
        if precedence.insert(key, release.id).is_some() {
            return Err(release_contract(
                "GitHub release listing contains duplicate stable SemVer precedence",
            ));
        }
        if selected
            .as_ref()
            .is_none_or(|(_, current)| key > stable_precedence(current))
        {
            selected = Some((release, version));
        }
    }

    let (release, version) = selected.ok_or_else(|| {
        release_contract("GitHub release listing contains no canonical stable release")
    })?;
    let assets = resolve_release_assets(release, target)?;
    Ok(DiscoveredReleaseV1 {
        release_id: release.id,
        tag: release.tag_name.clone(),
        version,
        target,
        assets,
    })
}

fn resolve_release_assets(
    release: &GitHubReleaseDtoV1,
    target: UpdateTarget,
) -> Result<ReleaseAssetsV1, UpdaterError> {
    let mut names = BTreeSet::new();
    for asset in &release.assets {
        if asset.id == 0 || !names.insert(asset.name.as_str()) {
            return Err(release_contract(
                "selected GitHub release contains an invalid or duplicate asset",
            ));
        }
    }

    let archive = require_asset(release, target.archive_name, AssetSizePolicy::Archive)?;
    let manifest = require_asset(release, target.manifest_name, AssetSizePolicy::Manifest)?;
    let signature = require_asset(release, target.signature_name, AssetSizePolicy::Signature)?;
    Ok(ReleaseAssetsV1 {
        archive,
        manifest,
        signature,
    })
}

fn require_asset(
    release: &GitHubReleaseDtoV1,
    name: &str,
    size_policy: AssetSizePolicy,
) -> Result<ReleaseAssetV1, UpdaterError> {
    let asset = release
        .assets
        .iter()
        .find(|asset| asset.name == name)
        .ok_or_else(|| release_contract("selected GitHub release is missing a required asset"))?;
    if asset.state != "uploaded" {
        return Err(release_contract(
            "selected GitHub release asset is not uploaded",
        ));
    }
    size_policy.validate(asset.size)?;
    validate_asset_api_url(&asset.url, asset.id)?;
    let expected_download_url = format!(
        "https://github.com/Bobsans/AIHelper/releases/download/{}/{}",
        release.tag_name, name
    );
    if asset.browser_download_url != expected_download_url {
        return Err(release_contract(
            "selected GitHub release asset has an unexpected download URL",
        ));
    }
    Ok(ReleaseAssetV1 {
        id: asset.id,
        name: asset.name.clone(),
        size: asset.size,
        api_url: asset.url.clone(),
        browser_download_url: asset.browser_download_url.clone(),
    })
}

#[derive(Debug, Clone, Copy)]
enum AssetSizePolicy {
    Archive,
    Manifest,
    Signature,
}

impl AssetSizePolicy {
    fn validate(self, size: u64) -> Result<(), UpdaterError> {
        const MAX_ARCHIVE_BYTES: u64 = 512 * 1024 * 1024;
        let valid = match self {
            Self::Archive => (1..=MAX_ARCHIVE_BYTES).contains(&size),
            Self::Manifest => (1..=MAX_MANIFEST_BYTES as u64).contains(&size),
            Self::Signature => size == DETACHED_SIGNATURE_BYTES as u64,
        };
        if valid {
            Ok(())
        } else {
            Err(release_contract(
                "selected GitHub release asset has an invalid declared size",
            ))
        }
    }
}

fn validate_asset_api_url(raw: &str, asset_id: u64) -> Result<(), UpdaterError> {
    let url = Url::parse(raw)
        .map_err(|_| release_contract("selected GitHub release asset API URL is invalid"))?;
    let expected_path = format!("/repos/Bobsans/AIHelper/releases/assets/{asset_id}");
    if url.scheme() != "https"
        || url.host_str() != Some("api.github.com")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path() != expected_path
    {
        return Err(release_contract(
            "selected GitHub release asset API URL is invalid",
        ));
    }
    Ok(())
}

fn stable_precedence(version: &StableReleaseVersion) -> (u64, u64, u64) {
    (
        version.version().major,
        version.version().minor,
        version.version().patch,
    )
}

fn release_contract(detail: &'static str) -> UpdaterError {
    UpdaterError::new(UpdaterErrorCode::ReleaseContract, detail)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UpdateOperation {
    Check,
    Upgrade,
    Version,
    Rollback,
    Recovery,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckStatus {
    UpToDate,
    UpdateAvailable,
    CurrentNewer,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_only_canonical_stable_release_tags() {
        assert_eq!(
            StableReleaseVersion::parse_tag("v1.2.3").unwrap().version(),
            &Version::new(1, 2, 3)
        );
        for invalid in ["1.2.3", "v1.02.3", "v1.2.3-beta.1", "vnot-semver"] {
            let error = StableReleaseVersion::parse_tag(invalid).unwrap_err();
            assert_eq!(error.code(), UpdaterErrorCode::ReleaseContract);
        }
    }

    #[test]
    fn github_dtos_ignore_untrusted_additive_fields() {
        let release = serde_json::from_str::<GitHubReleaseDtoV1>(
            r#"{
                "id": 42,
                "tag_name": "v1.2.3",
                "draft": false,
                "prerelease": false,
                "html_url": "https://example.invalid/release",
                "assets": [{
                    "id": 7,
                    "name": "ah-windows-x64.zip",
                    "state": "uploaded",
                    "size": 10,
                    "url": "https://api.github.com/assets/7",
                    "browser_download_url": "https://github.com/example/releases/download/v1.2.3/ah-windows-x64.zip",
                    "content_type": "application/zip"
                }]
            }"#,
        )
        .unwrap();

        assert_eq!(release.id, 42);
        assert_eq!(release.assets[0].id, 7);
    }

    #[test]
    fn windows_target_uses_exact_release_asset_names() {
        assert_eq!(WINDOWS_X64_TARGET.rust_target, "x86_64-pc-windows-msvc");
        assert_eq!(WINDOWS_X64_TARGET.architecture, "x86_64");
        assert_eq!(WINDOWS_X64_TARGET.archive_name, "ah-windows-x64.zip");
        assert_eq!(
            WINDOWS_X64_TARGET.manifest_name,
            "ah-windows-x64.manifest.json"
        );
        assert_eq!(
            WINDOWS_X64_TARGET.signature_name,
            "ah-windows-x64.manifest.sig"
        );
    }

    #[test]
    fn manifest_target_mismatch_is_a_compatibility_error() {
        let error = WINDOWS_X64_TARGET
            .require_manifest_identity("x86_64-unknown-linux-gnu", "x86_64")
            .unwrap_err();
        assert_eq!(error.code(), UpdaterErrorCode::Compatibility);
    }

    #[test]
    fn selects_highest_stable_release_independent_of_api_order() {
        let releases = vec![
            release(2, "v1.2.0", false, false),
            release(4, "v9.0.0", true, false),
            release(1, "v1.1.0", false, false),
            release(5, "v8.0.0-beta.1", false, true),
            release(3, "v1.3.0", false, false),
        ];

        let selected = select_highest_stable_release(&releases, WINDOWS_X64_TARGET).unwrap();

        assert_eq!(selected.release_id, 3);
        assert_eq!(selected.tag, "v1.3.0");
        assert_eq!(selected.assets.archive.name, "ah-windows-x64.zip");
        assert_eq!(
            selected.assets.manifest.name,
            "ah-windows-x64.manifest.json"
        );
        assert_eq!(
            selected.assets.signature.name,
            "ah-windows-x64.manifest.sig"
        );
    }

    #[test]
    fn never_falls_back_when_highest_release_is_incomplete() {
        let mut highest = release(2, "v1.2.0", false, false);
        highest
            .assets
            .retain(|asset| asset.name != WINDOWS_X64_TARGET.signature_name);
        let error = select_highest_stable_release(
            &[release(1, "v1.1.0", false, false), highest],
            WINDOWS_X64_TARGET,
        )
        .unwrap_err();

        assert_eq!(error.code(), UpdaterErrorCode::ReleaseContract);
        assert!(error.detail().contains("missing a required asset"));
    }

    #[test]
    fn rejects_malformed_stable_tags_and_duplicate_asset_names() {
        let malformed = select_highest_stable_release(
            &[
                release(1, "v1.1.0", false, false),
                release(2, "latest", false, false),
            ],
            WINDOWS_X64_TARGET,
        )
        .unwrap_err();
        assert_eq!(malformed.code(), UpdaterErrorCode::ReleaseContract);

        let mut duplicated = release(3, "v1.3.0", false, false);
        duplicated.assets.push(duplicated.assets[0].clone());
        let duplicate =
            select_highest_stable_release(&[duplicated], WINDOWS_X64_TARGET).unwrap_err();
        assert_eq!(duplicate.code(), UpdaterErrorCode::ReleaseContract);
        assert!(duplicate.detail().contains("duplicate asset"));
    }

    #[test]
    fn rejects_ambiguous_stable_precedence() {
        let error = select_highest_stable_release(
            &[
                release(1, "v1.2.0+one", false, false),
                release(2, "v1.2.0+two", false, false),
            ],
            WINDOWS_X64_TARGET,
        )
        .unwrap_err();
        assert_eq!(error.code(), UpdaterErrorCode::ReleaseContract);
        assert!(
            error
                .detail()
                .contains("duplicate stable SemVer precedence")
        );
    }

    #[test]
    fn rejects_unuploaded_or_misdirected_required_assets() {
        let mut unuploaded = release(1, "v1.1.0", false, false);
        unuploaded.assets[0].state = "open".to_owned();
        let state = select_highest_stable_release(&[unuploaded], WINDOWS_X64_TARGET).unwrap_err();
        assert_eq!(state.code(), UpdaterErrorCode::ReleaseContract);

        let mut redirected = release(2, "v1.2.0", false, false);
        redirected.assets[0].url = "https://example.invalid/assets/1".to_owned();
        let url = select_highest_stable_release(&[redirected], WINDOWS_X64_TARGET).unwrap_err();
        assert_eq!(url.code(), UpdaterErrorCode::ReleaseContract);
    }

    #[test]
    fn rejects_empty_release_listing() {
        let error = select_highest_stable_release(&[], WINDOWS_X64_TARGET).unwrap_err();
        assert_eq!(error.code(), UpdaterErrorCode::ReleaseContract);
        assert!(error.detail().contains("no canonical stable release"));
    }

    fn release(id: u64, tag: &str, draft: bool, prerelease: bool) -> GitHubReleaseDtoV1 {
        let names = [
            WINDOWS_X64_TARGET.archive_name,
            WINDOWS_X64_TARGET.manifest_name,
            WINDOWS_X64_TARGET.signature_name,
        ];
        let assets = names
            .into_iter()
            .enumerate()
            .map(|(index, name)| {
                let asset_id = id * 10 + index as u64 + 1;
                let size = if name == WINDOWS_X64_TARGET.signature_name {
                    DETACHED_SIGNATURE_BYTES as u64
                } else {
                    100
                };
                GitHubAssetDtoV1 {
                    id: asset_id,
                    name: name.to_owned(),
                    state: "uploaded".to_owned(),
                    size,
                    url: format!(
                        "https://api.github.com/repos/Bobsans/AIHelper/releases/assets/{asset_id}"
                    ),
                    browser_download_url: format!(
                        "https://github.com/Bobsans/AIHelper/releases/download/{tag}/{name}"
                    ),
                }
            })
            .collect();
        GitHubReleaseDtoV1 {
            id,
            tag_name: tag.to_owned(),
            draft,
            prerelease,
            assets,
        }
    }
}
