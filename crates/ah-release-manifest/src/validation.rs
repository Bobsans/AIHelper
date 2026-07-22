use std::collections::{BTreeMap, BTreeSet};

use semver::Version;
use url::Url;

use crate::{
    FilePurpose, MAX_MANAGED_FILES, MAX_MANAGED_PATH_BYTES, ManifestError, ReleaseManifest,
    SCHEMA_VERSION, SUPPORTED_TARGETS,
};

const KEY_ID_PREFIX: &str = "ed25519-sha256-";

impl ReleaseManifest {
    pub fn validate(&self) -> Result<(), ManifestError> {
        if self.schema_version != SCHEMA_VERSION {
            return Err(ManifestError::UnsupportedSchema {
                found: self.schema_version,
            });
        }

        validate_semver("release.version", &self.release.version)?;
        validate_target(&self.release.target, &self.release.architecture)?;
        validate_archive_url(&self.archive.url)?;
        if self.archive.size == 0 {
            return invalid("archive.size", "must be greater than zero");
        }
        validate_digest("archive.sha256", &self.archive.sha256)?;
        validate_semver("minimum_updater_version", &self.minimum_updater_version)?;
        validate_key_id(&self.signing.key_id)?;
        validate_inventory(self)
    }
}

fn validate_semver(field: &'static str, raw: &str) -> Result<(), ManifestError> {
    let parsed = Version::parse(raw).map_err(|_| ManifestError::InvalidField {
        field,
        detail: "must be a canonical semantic version".to_owned(),
    })?;
    if parsed.to_string() != raw {
        return invalid(field, "must use canonical semantic-version spelling");
    }
    Ok(())
}

fn validate_target(target: &str, architecture: &str) -> Result<(), ManifestError> {
    if SUPPORTED_TARGETS
        .iter()
        .any(|candidate| *candidate == (target, architecture))
    {
        return Ok(());
    }

    invalid(
        "release.target",
        "target and architecture pair is unsupported",
    )
}

fn validate_archive_url(raw: &str) -> Result<(), ManifestError> {
    let url = Url::parse(raw).map_err(|_| ManifestError::InvalidField {
        field: "archive.url",
        detail: "must be an absolute HTTPS URL".to_owned(),
    })?;

    if url.scheme() != "https" || url.host_str().is_none() {
        return invalid("archive.url", "must be an absolute HTTPS URL with a host");
    }
    if !url.username().is_empty() || url.password().is_some() {
        return invalid("archive.url", "must not contain credentials");
    }
    if url.fragment().is_some() {
        return invalid("archive.url", "must not contain a fragment");
    }
    Ok(())
}

fn validate_digest(field: &'static str, digest: &str) -> Result<(), ManifestError> {
    if digest.len() != 64
        || !digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return invalid(field, "must be 64 lowercase hexadecimal characters");
    }
    Ok(())
}

fn validate_key_id(key_id: &str) -> Result<(), ManifestError> {
    let Some(fingerprint) = key_id.strip_prefix(KEY_ID_PREFIX) else {
        return invalid(
            "signing.key_id",
            "must use the ed25519-sha256 fingerprint form",
        );
    };
    validate_digest("signing.key_id", fingerprint)
}

fn validate_inventory(manifest: &ReleaseManifest) -> Result<(), ManifestError> {
    if manifest.files.is_empty() {
        return invalid("files", "must contain at least one managed file");
    }
    if manifest.files.len() > MAX_MANAGED_FILES {
        return invalid("files", "exceeds the managed-file count limit");
    }

    let mut inventory = BTreeMap::new();
    let mut folded_paths = BTreeSet::new();
    let mut previous_path: Option<&str> = None;
    for file in &manifest.files {
        validate_path("files[].path", &file.path)?;
        validate_digest("files[].sha256", &file.sha256)?;

        if let Some(previous) = previous_path
            && previous >= file.path.as_str()
        {
            return invalid("files[].path", "must be strictly sorted and unique");
        }
        previous_path = Some(&file.path);

        if !folded_paths.insert(file.path.to_ascii_lowercase()) {
            return invalid("files[].path", "must be unique after ASCII case folding");
        }
        inventory.insert(file.path.as_str(), file.purpose);
    }

    validate_required_list("required.executables", &manifest.required.executables)?;
    validate_required_list("required.plugins", &manifest.required.plugins)?;
    if manifest.required.executables.is_empty() {
        return invalid(
            "required.executables",
            "must contain at least one executable",
        );
    }

    for path in &manifest.required.executables {
        match inventory.get(path.as_str()) {
            Some(FilePurpose::Executable | FilePurpose::UpdateHelper) => {}
            Some(_) => {
                return invalid(
                    "required.executables",
                    "referenced file has an incompatible purpose",
                );
            }
            None => {
                return invalid(
                    "required.executables",
                    "referenced file is absent from the inventory",
                );
            }
        }
    }

    for path in &manifest.required.plugins {
        match inventory.get(path.as_str()) {
            Some(FilePurpose::Plugin) => {}
            Some(_) => {
                return invalid(
                    "required.plugins",
                    "referenced file has an incompatible purpose",
                );
            }
            None => {
                return invalid(
                    "required.plugins",
                    "referenced file is absent from the inventory",
                );
            }
        }
    }

    Ok(())
}

fn validate_required_list(field: &'static str, paths: &[String]) -> Result<(), ManifestError> {
    let mut folded_paths = BTreeSet::new();
    let mut previous_path: Option<&str> = None;
    for path in paths {
        validate_path(field, path)?;
        if let Some(previous) = previous_path
            && previous >= path.as_str()
        {
            return invalid(field, "must be strictly sorted and unique");
        }
        previous_path = Some(path);
        if !folded_paths.insert(path.to_ascii_lowercase()) {
            return invalid(field, "must be unique after ASCII case folding");
        }
    }
    Ok(())
}

fn validate_path(field: &'static str, path: &str) -> Result<(), ManifestError> {
    if path.is_empty() || path.len() > MAX_MANAGED_PATH_BYTES {
        return invalid(field, "must contain from 1 through 512 ASCII bytes");
    }
    if !path.is_ascii() {
        return invalid(field, "must contain ASCII characters only");
    }
    if !path
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'/'))
    {
        return invalid(field, "contains a character outside the path alphabet");
    }
    if path.starts_with('/') || path.ends_with('/') {
        return invalid(field, "must be a relative path without boundary slashes");
    }
    if path
        .split('/')
        .any(|segment| segment.is_empty() || matches!(segment, "." | ".."))
    {
        return invalid(field, "must not contain empty, dot, or parent segments");
    }
    Ok(())
}

fn invalid<T>(field: &'static str, detail: &str) -> Result<T, ManifestError> {
    Err(ManifestError::InvalidField {
        field,
        detail: detail.to_owned(),
    })
}
