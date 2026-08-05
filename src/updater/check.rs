use ah_updater_core::{
    CheckStatus, DiscoveredReleaseV1, ReleaseAssetV1, ReleaseTrust, UpdateSource, UpdaterError,
    UpgradeCheckResultV1, verify_discovered_release_for_check,
};
use semver::Version;

use crate::{
    cli::GlobalOptions,
    error::AppError,
    output::OutputMode,
    updater::{
        command::UpgradeRequest, github::GitHubReleaseClient, trust::production_release_trust,
    },
};

pub(crate) trait ReleaseCheckSource {
    fn discover(&self) -> Result<DiscoveredReleaseV1, UpdaterError>;
    fn download(&self, asset: &ReleaseAssetV1) -> Result<Vec<u8>, UpdaterError>;
}

pub(crate) fn execute(request: UpgradeRequest, options: GlobalOptions) -> Result<(), AppError> {
    match request {
        UpgradeRequest::Check => execute_check(options),
        request @ (UpgradeRequest::Upgrade | UpgradeRequest::Version(_)) => {
            super::activate::execute(request, options)
        }
    }
}

fn execute_check(options: GlobalOptions) -> Result<(), AppError> {
    ah_updater_core::UpdateTarget::current().map_err(map_updater_error)?;
    let trust = production_release_trust().map_err(map_updater_error)?;
    let source = GitHubReleaseClient::new().map_err(map_updater_error)?;
    let current_version = Version::parse(env!("CARGO_PKG_VERSION")).map_err(|_| {
        AppError::external(
            "UPDATER_RELEASE_CONTRACT",
            "running AIHelper version is not canonical SemVer",
        )
    })?;
    let result = perform_check(&source, &trust, &current_version).map_err(map_updater_error)?;
    render_result(&result, options)
}

pub(crate) fn perform_check(
    source: &impl ReleaseCheckSource,
    trust: &ReleaseTrust,
    current_version: &Version,
) -> Result<UpgradeCheckResultV1, UpdaterError> {
    let discovered = source.discover()?;
    let manifest_bytes = source.download(&discovered.assets.manifest)?;
    let signature_bytes = source.download(&discovered.assets.signature)?;
    verify_discovered_release_for_check(
        &discovered,
        &manifest_bytes,
        &signature_bytes,
        trust,
        current_version,
    )
}

fn render_result(result: &UpgradeCheckResultV1, options: GlobalOptions) -> Result<(), AppError> {
    if options.quiet {
        return Ok(());
    }
    match options.output {
        OutputMode::Json => println!("{}", serde_json::to_string_pretty(result)?),
        OutputMode::Text => println!(
            "status={} current_version={} selected_version={} target={} source={}",
            check_status(result.status),
            result.current_version,
            result.selected_version.as_deref().unwrap_or("none"),
            result.target.as_deref().unwrap_or("none"),
            update_source(result.source),
        ),
    }
    Ok(())
}

fn check_status(status: CheckStatus) -> &'static str {
    match status {
        CheckStatus::UpToDate => "up_to_date",
        CheckStatus::UpdateAvailable => "update_available",
        CheckStatus::CurrentNewer => "current_newer",
    }
}

fn update_source(source: Option<UpdateSource>) -> &'static str {
    match source {
        Some(UpdateSource::GitHubRelease) => "github_release",
        None => "none",
    }
}

fn map_updater_error(error: UpdaterError) -> AppError {
    AppError::external(
        format!("UPDATER_{}", error.code().as_str().to_ascii_uppercase()),
        error.detail(),
    )
}

#[cfg(test)]
mod tests {
    use std::{cell::Cell, fs};

    use ah_release_manifest::{
        ArchiveMetadata, FilePurpose, ManagedFile, ReleaseManifest, ReleaseMetadata, RequiredFiles,
        SCHEMA_VERSION, SIGNING_ALGORITHM, SIGNING_DOMAIN, SignatureAlgorithm, SigningMetadata,
        TrustedKey, key_id_for_public_key,
    };
    use ah_updater_core::{
        DETACHED_SIGNATURE_BYTES, ReleaseAssetsV1, StableReleaseVersion, UpdaterErrorCode,
        WINDOWS_X64_TARGET,
    };
    use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
    use ed25519_dalek::{Signer as _, SigningKey};
    use tempfile::TempDir;

    use super::*;

    #[test]
    fn reports_all_check_statuses_without_filesystem_mutation() {
        let fixture = signed_fixture("1.2.0", "1.0.0", [7_u8; 32]);
        let sentinel = TempDir::new().unwrap();
        let sentinel_path = sentinel.path().join("user-owned.txt");
        fs::write(&sentinel_path, b"unchanged").unwrap();

        for (current, expected) in [
            ("1.1.0", CheckStatus::UpdateAvailable),
            ("1.2.0", CheckStatus::UpToDate),
            ("1.3.0", CheckStatus::CurrentNewer),
        ] {
            let source = FakeSource::new(&fixture);
            let result =
                perform_check(&source, &fixture.trust, &Version::parse(current).unwrap()).unwrap();
            assert_eq!(result.status, expected);
            assert_eq!(source.discoveries.get(), 1);
            assert_eq!(source.downloads.get(), 2);
            assert_eq!(fs::read(&sentinel_path).unwrap(), b"unchanged");
            assert_eq!(fs::read_dir(sentinel.path()).unwrap().count(), 1);
        }
    }

    #[test]
    fn rejects_incompatible_updater_after_signature_verification() {
        let fixture = signed_fixture("1.2.0", "1.2.0", [7_u8; 32]);
        let source = FakeSource::new(&fixture);

        let error =
            perform_check(&source, &fixture.trust, &Version::parse("1.1.0").unwrap()).unwrap_err();

        assert_eq!(error.code(), UpdaterErrorCode::Compatibility);
        assert_eq!(source.downloads.get(), 2);
    }

    #[test]
    fn rejects_signed_manifest_identity_mismatches() {
        let original = signed_fixture("1.2.0", "1.0.0", [7_u8; 32]);

        let mut wrong_tag = original.clone();
        wrong_tag.discovered.tag = "v1.2.1".to_owned();
        assert_check_error(&wrong_tag, UpdaterErrorCode::ReleaseContract);

        let mut wrong_target = original.clone();
        wrong_target.discovered.target = ah_updater_core::UpdateTarget {
            rust_target: "x86_64-unknown-linux-gnu",
            architecture: "x86_64",
            archive_name: "ah-linux-x64.zip",
            manifest_name: "ah-linux-x64.manifest.json",
            signature_name: "ah-linux-x64.manifest.sig",
        };
        assert_check_error(&wrong_target, UpdaterErrorCode::Compatibility);

        let mut wrong_archive_url = original.clone();
        wrong_archive_url
            .discovered
            .assets
            .archive
            .browser_download_url = "https://example.invalid/archive.zip".to_owned();
        assert_check_error(&wrong_archive_url, UpdaterErrorCode::ReleaseContract);

        let mut wrong_archive_size = original.clone();
        wrong_archive_size.discovered.assets.archive.size += 1;
        assert_check_error(&wrong_archive_size, UpdaterErrorCode::ReleaseContract);
    }

    #[derive(Clone)]
    struct SignedFixture {
        discovered: DiscoveredReleaseV1,
        manifest: Vec<u8>,
        signature: Vec<u8>,
        trust: ReleaseTrust,
    }

    struct FakeSource<'a> {
        fixture: &'a SignedFixture,
        discoveries: Cell<usize>,
        downloads: Cell<usize>,
    }

    impl<'a> FakeSource<'a> {
        fn new(fixture: &'a SignedFixture) -> Self {
            Self {
                fixture,
                discoveries: Cell::new(0),
                downloads: Cell::new(0),
            }
        }
    }

    impl ReleaseCheckSource for FakeSource<'_> {
        fn discover(&self) -> Result<DiscoveredReleaseV1, UpdaterError> {
            self.discoveries.set(self.discoveries.get() + 1);
            Ok(self.fixture.discovered.clone())
        }

        fn download(&self, asset: &ReleaseAssetV1) -> Result<Vec<u8>, UpdaterError> {
            self.downloads.set(self.downloads.get() + 1);
            if asset.name == WINDOWS_X64_TARGET.manifest_name {
                Ok(self.fixture.manifest.clone())
            } else if asset.name == WINDOWS_X64_TARGET.signature_name {
                Ok(self.fixture.signature.clone())
            } else {
                Err(UpdaterError::new(
                    UpdaterErrorCode::Network,
                    "unexpected test asset",
                ))
            }
        }
    }

    fn assert_check_error(fixture: &SignedFixture, expected: UpdaterErrorCode) {
        let error = perform_check(
            &FakeSource::new(fixture),
            &fixture.trust,
            &Version::parse("1.1.0").unwrap(),
        )
        .unwrap_err();
        assert_eq!(error.code(), expected);
    }

    fn signed_fixture(version: &str, minimum: &str, seed: [u8; 32]) -> SignedFixture {
        let signing = SigningKey::from_bytes(&seed);
        let public_key = signing.verifying_key().to_bytes();
        let key_id = key_id_for_public_key(&public_key);
        let tag = format!("v{version}");
        let archive_url = format!(
            "https://github.com/Bobsans/AIHelper/releases/download/{tag}/{}",
            WINDOWS_X64_TARGET.archive_name
        );
        let manifest = ReleaseManifest {
            schema_version: SCHEMA_VERSION,
            release: ReleaseMetadata {
                version: version.to_owned(),
                target: WINDOWS_X64_TARGET.rust_target.to_owned(),
                architecture: WINDOWS_X64_TARGET.architecture.to_owned(),
            },
            archive: ArchiveMetadata {
                url: archive_url.clone(),
                size: 100,
                sha256: "0".repeat(64),
            },
            minimum_updater_version: minimum.to_owned(),
            signing: SigningMetadata {
                key_id: key_id.clone(),
                algorithm: SignatureAlgorithm::Ed25519,
            },
            files: vec![ManagedFile {
                path: "ah.exe".to_owned(),
                size: 1,
                sha256: "1".repeat(64),
                purpose: FilePurpose::Executable,
            }],
            required: RequiredFiles {
                executables: vec!["ah.exe".to_owned()],
                plugins: Vec::new(),
            },
        }
        .to_canonical_bytes()
        .unwrap();
        let mut preimage = Vec::from(SIGNING_DOMAIN);
        preimage.extend_from_slice(&manifest);
        let signature = URL_SAFE_NO_PAD
            .encode(signing.sign(&preimage).to_bytes())
            .into_bytes();
        assert_eq!(signature.len(), DETACHED_SIGNATURE_BYTES);

        let archive = asset(1, WINDOWS_X64_TARGET.archive_name, 100, &tag);
        let manifest_asset = asset(
            2,
            WINDOWS_X64_TARGET.manifest_name,
            manifest.len() as u64,
            &tag,
        );
        let signature_asset = asset(
            3,
            WINDOWS_X64_TARGET.signature_name,
            signature.len() as u64,
            &tag,
        );
        let discovered = DiscoveredReleaseV1 {
            release_id: 1,
            tag: tag.clone(),
            version: StableReleaseVersion::parse_tag(&tag).unwrap(),
            target: WINDOWS_X64_TARGET,
            assets: ReleaseAssetsV1 {
                archive,
                manifest: manifest_asset,
                signature: signature_asset,
            },
        };
        let trust = ReleaseTrust::from_keys(vec![TrustedKey {
            key_id,
            algorithm: SIGNING_ALGORITHM.to_owned(),
            public_key,
        }])
        .unwrap();
        SignedFixture {
            discovered,
            manifest,
            signature,
            trust,
        }
    }

    fn asset(id: u64, name: &str, size: u64, tag: &str) -> ReleaseAssetV1 {
        ReleaseAssetV1 {
            id,
            name: name.to_owned(),
            size,
            api_url: format!("https://api.github.com/repos/Bobsans/AIHelper/releases/assets/{id}"),
            browser_download_url: format!(
                "https://github.com/Bobsans/AIHelper/releases/download/{tag}/{name}"
            ),
        }
    }
}
