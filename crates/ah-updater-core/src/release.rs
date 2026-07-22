use semver::Version;
use serde::{Deserialize, Serialize};

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
}
