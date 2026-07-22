use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use ah_release_manifest::{
    ArchiveMetadata, ManagedFile, ReleaseManifest, ReleaseMetadata, RequiredFiles, SCHEMA_VERSION,
    SignatureAlgorithm, SigningMetadata, verify_manifest,
};
use semver::Version;

use crate::{
    ArchiveInventory, RELEASE_PROFILES, ReleaseToolError, SigningMaterial, validate_archive,
    validate_archive_set,
};

const MANIFEST_SUFFIX: &str = ".manifest.json";
const SIGNATURE_SUFFIX: &str = ".manifest.sig";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseRequest {
    pub assets_dir: PathBuf,
    pub output_dir: PathBuf,
    pub repository: String,
    pub tag: String,
    pub minimum_updater_version: String,
}

pub fn sign_release_set(
    request: &ReleaseRequest,
    signing: &SigningMaterial,
) -> Result<Vec<PathBuf>, ReleaseToolError> {
    let (version, minimum_updater_version) = validate_release_metadata(
        &request.repository,
        &request.tag,
        &request.minimum_updater_version,
    )?;
    if request.output_dir.exists() {
        return Err(ReleaseToolError::metadata(format!(
            "output path '{}' already exists",
            request.output_dir.display()
        )));
    }
    validate_archive_set(&request.assets_dir)?;

    let parent = usable_parent(&request.output_dir);
    fs::create_dir_all(parent)
        .map_err(|source| ReleaseToolError::io("create output parent", parent, source))?;
    let staging = tempfile::Builder::new()
        .prefix(".ah-release-assets-")
        .tempdir_in(parent)
        .map_err(|source| {
            ReleaseToolError::io("create release staging directory", parent, source)
        })?;

    for profile in RELEASE_PROFILES {
        let source = request.assets_dir.join(profile.asset_name);
        let destination = staging.path().join(profile.asset_name);
        fs::copy(&source, &destination).map_err(|source_error| {
            ReleaseToolError::io("copy release archive", source, source_error)
        })?;
    }
    let inventories = validate_archive_set(staging.path())?;
    let registry = signing.trusted_registry()?;

    for inventory in &inventories {
        let manifest = build_manifest(
            inventory,
            &request.repository,
            &request.tag,
            &version,
            &minimum_updater_version,
            signing.key_id(),
        );
        let manifest_bytes = manifest
            .to_canonical_bytes()
            .map_err(|error| ReleaseToolError::manifest(error.to_string()))?;
        let signature = signing.sign_manifest(&manifest_bytes);
        let manifest_path = staging
            .path()
            .join(manifest_name(inventory.profile.asset_name));
        let signature_path = staging
            .path()
            .join(signature_name(inventory.profile.asset_name));
        fs::write(&manifest_path, &manifest_bytes).map_err(|source| {
            ReleaseToolError::io("write release manifest", manifest_path.clone(), source)
        })?;
        fs::write(&signature_path, signature.as_bytes()).map_err(|source| {
            ReleaseToolError::io("write release signature", signature_path.clone(), source)
        })?;

        let verified = verify_manifest(&manifest_bytes, signature.as_bytes(), &registry)
            .map_err(|error| ReleaseToolError::manifest(error.to_string()))?;
        if verified.manifest() != &manifest {
            return Err(ReleaseToolError::manifest(
                "verified manifest differs from the generated manifest",
            ));
        }
    }

    verify_staged_set(
        staging.path(),
        &inventories,
        &request.repository,
        &request.tag,
        &version,
        &minimum_updater_version,
        signing,
    )?;
    let output_paths = expected_output_paths(&request.output_dir);
    fs::rename(staging.path(), &request.output_dir).map_err(|source| {
        ReleaseToolError::io(
            "activate verified release asset set",
            request.output_dir.clone(),
            source,
        )
    })?;
    Ok(output_paths)
}

fn build_manifest(
    inventory: &ArchiveInventory,
    repository: &str,
    tag: &str,
    version: &Version,
    minimum_updater_version: &Version,
    key_id: &str,
) -> ReleaseManifest {
    let files = inventory
        .files
        .iter()
        .map(|file| ManagedFile {
            path: file.path.clone(),
            size: file.size,
            sha256: file.sha256.clone(),
            purpose: file.purpose,
        })
        .collect::<Vec<_>>();
    let executables = files
        .iter()
        .filter(|file| file.purpose == ah_release_manifest::FilePurpose::Executable)
        .map(|file| file.path.clone())
        .collect();
    let plugins = files
        .iter()
        .filter(|file| file.purpose == ah_release_manifest::FilePurpose::Plugin)
        .map(|file| file.path.clone())
        .collect();
    ReleaseManifest {
        schema_version: SCHEMA_VERSION,
        release: ReleaseMetadata {
            version: version.to_string(),
            target: inventory.profile.target.to_owned(),
            architecture: inventory.profile.architecture.to_owned(),
        },
        archive: ArchiveMetadata {
            url: format!(
                "https://github.com/{repository}/releases/download/{tag}/{}",
                inventory.profile.asset_name
            ),
            size: inventory.archive_size,
            sha256: inventory.archive_sha256.clone(),
        },
        minimum_updater_version: minimum_updater_version.to_string(),
        signing: SigningMetadata {
            key_id: key_id.to_owned(),
            algorithm: SignatureAlgorithm::Ed25519,
        },
        files,
        required: RequiredFiles {
            executables,
            plugins,
        },
    }
}

fn verify_staged_set(
    staging: &Path,
    inventories: &[ArchiveInventory],
    repository: &str,
    tag: &str,
    version: &Version,
    minimum_updater_version: &Version,
    signing: &SigningMaterial,
) -> Result<(), ReleaseToolError> {
    let registry = signing.trusted_registry()?;
    for prior in inventories {
        let current = validate_archive(&staging.join(prior.profile.asset_name), prior.profile)?;
        if &current != prior {
            return Err(ReleaseToolError::archive(
                prior.profile.asset_name,
                "archive changed while the release set was prepared",
            ));
        }
        let expected = build_manifest(
            &current,
            repository,
            tag,
            version,
            minimum_updater_version,
            signing.key_id(),
        );
        let manifest_path = staging.join(manifest_name(prior.profile.asset_name));
        let signature_path = staging.join(signature_name(prior.profile.asset_name));
        let manifest_bytes = fs::read(&manifest_path).map_err(|source| {
            ReleaseToolError::io("read generated manifest", manifest_path, source)
        })?;
        let signature_bytes = fs::read(&signature_path).map_err(|source| {
            ReleaseToolError::io("read generated signature", signature_path, source)
        })?;
        let verified = verify_manifest(&manifest_bytes, &signature_bytes, &registry)
            .map_err(|error| ReleaseToolError::manifest(error.to_string()))?;
        if verified.manifest() != &expected {
            return Err(ReleaseToolError::manifest(format!(
                "manifest for '{}' does not match its archive",
                prior.profile.asset_name
            )));
        }
    }

    let observed = fs::read_dir(staging)
        .map_err(|source| ReleaseToolError::io("read staged release set", staging, source))?
        .map(|entry| {
            entry
                .map_err(|source| ReleaseToolError::io("read staged release set", staging, source))?
                .file_name()
                .into_string()
                .map_err(|_| ReleaseToolError::metadata("output filenames must be valid UTF-8"))
        })
        .collect::<Result<BTreeSet<_>, _>>()?;
    let expected = expected_output_names();
    if observed != expected {
        return Err(ReleaseToolError::metadata(format!(
            "expected output [{}], found [{}]",
            expected.iter().cloned().collect::<Vec<_>>().join(", "),
            observed.iter().cloned().collect::<Vec<_>>().join(", ")
        )));
    }
    Ok(())
}

fn validate_release_metadata(
    repository: &str,
    tag: &str,
    minimum_updater_version: &str,
) -> Result<(Version, Version), ReleaseToolError> {
    let mut repository_parts = repository.split('/');
    let owner = repository_parts.next().unwrap_or_default();
    let name = repository_parts.next().unwrap_or_default();
    if owner.is_empty()
        || name.is_empty()
        || repository_parts.next().is_some()
        || !owner.chars().all(repository_character)
        || !name.chars().all(repository_character)
    {
        return Err(ReleaseToolError::metadata(
            "repository must be one safe 'owner/name' value",
        ));
    }
    let raw_version = tag
        .strip_prefix('v')
        .ok_or_else(|| ReleaseToolError::metadata("release tag must be 'v<SemVer>'"))?;
    let version = Version::parse(raw_version)
        .map_err(|_| ReleaseToolError::metadata("release tag must contain canonical SemVer"))?;
    if version.to_string() != raw_version {
        return Err(ReleaseToolError::metadata(
            "release tag must contain canonical SemVer",
        ));
    }
    if raw_version != env!("CARGO_PKG_VERSION") {
        return Err(ReleaseToolError::metadata(format!(
            "release tag version '{raw_version}' does not match workspace version '{}'",
            env!("CARGO_PKG_VERSION")
        )));
    }
    let parsed_minimum_updater_version = Version::parse(minimum_updater_version).map_err(|_| {
        ReleaseToolError::metadata("minimum updater version must contain canonical SemVer")
    })?;
    if parsed_minimum_updater_version.to_string() != minimum_updater_version {
        return Err(ReleaseToolError::metadata(
            "minimum updater version must contain canonical SemVer",
        ));
    }
    if parsed_minimum_updater_version > version {
        return Err(ReleaseToolError::metadata(
            "minimum updater version must not be newer than the release version",
        ));
    }
    Ok((version, parsed_minimum_updater_version))
}

fn repository_character(character: char) -> bool {
    character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.')
}

fn manifest_name(asset_name: &str) -> String {
    format!("{}{MANIFEST_SUFFIX}", asset_name.trim_end_matches(".zip"))
}

fn signature_name(asset_name: &str) -> String {
    format!("{}{SIGNATURE_SUFFIX}", asset_name.trim_end_matches(".zip"))
}

fn expected_output_names() -> BTreeSet<String> {
    RELEASE_PROFILES
        .iter()
        .flat_map(|profile| {
            [
                profile.asset_name.to_owned(),
                manifest_name(profile.asset_name),
                signature_name(profile.asset_name),
            ]
        })
        .collect()
}

fn expected_output_paths(output_dir: &Path) -> Vec<PathBuf> {
    expected_output_names()
        .into_iter()
        .map(|name| output_dir.join(name))
        .collect()
}

fn usable_parent(path: &Path) -> &Path {
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
}
