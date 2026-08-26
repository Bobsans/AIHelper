//! The three release records - identity, manifest, signature - that say which
//! release an installation *is*.
//!
//! Swapping them is what makes an activation or a rollback visible to the next
//! process to start.

use std::path::{Component, Path};

use ah_updater_core::{
    InstallationIdentityV1, ReleaseManifest, ReleaseTrust, TransactionPlanV1, UpdaterError,
};

use super::fsverify::{
    atomic_replace, ensure_direct_directory, path_from_slashes, paths_equal, read_bounded,
    read_optional_bounded, remove_file_synced, write_new_synced,
};
use super::journal::mutate_with_checkpoint;
use super::layout::{
    IDENTITY_FILE, INSTALLED_MANIFEST_FILE, INSTALLED_SIGNATURE_FILE, MAX_IDENTITY_BYTES,
    TransactionPaths,
};
use super::managed::{FileMutationPhase, operation_temp_name, verify_manifest_files};
use super::{FailureInjector, LoadedTransaction, TransactionRunError, recovery, transaction};

pub(super) fn verify_active_old_release(
    transaction: &LoadedTransaction,
    trust: &ReleaseTrust,
) -> Result<(), UpdaterError> {
    let paths = &transaction.paths;
    let active = read_release_record(&paths.installation_state_root, trust)?;
    let backup = read_release_record(&paths.backup_root(), trust)?;
    if active.manifest_bytes != backup.manifest_bytes
        || active.signature_bytes != backup.signature_bytes
    {
        return Err(recovery(
            "active release record does not match the transaction backup",
        ));
    }
    let active_identity = read_bounded(
        &paths.installation_state_root.join(IDENTITY_FILE),
        MAX_IDENTITY_BYTES,
        "failed to read active installation identity",
    )?;
    let backup_identity = read_bounded(
        &paths.backup_root().join(IDENTITY_FILE),
        MAX_IDENTITY_BYTES,
        "failed to read transaction backup identity",
    )?;
    if active_identity != backup_identity {
        return Err(recovery(
            "active installation identity does not match the transaction backup",
        ));
    }
    verify_manifest_files(&paths.installation_root, &transaction.old_manifest)
}

pub(super) fn verify_active_new_release(
    transaction: &LoadedTransaction,
    trust: &ReleaseTrust,
) -> Result<(), UpdaterError> {
    let paths = &transaction.paths;
    let active = read_release_record(&paths.installation_state_root, trust)?;
    let candidate = read_release_record(&paths.candidate_root(), trust)?;
    if active.manifest_bytes != candidate.manifest_bytes
        || active.signature_bytes != candidate.signature_bytes
    {
        return Err(recovery(
            "active release record does not match the transaction candidate",
        ));
    }
    let active_identity = read_bounded(
        &paths.installation_state_root.join(IDENTITY_FILE),
        MAX_IDENTITY_BYTES,
        "failed to read active installation identity",
    )?;
    let backup_identity = read_bounded(
        &paths.backup_root().join(IDENTITY_FILE),
        MAX_IDENTITY_BYTES,
        "failed to read transaction backup identity",
    )?;
    if active_identity != backup_identity {
        return Err(recovery(
            "active installation identity does not match the transaction backup",
        ));
    }
    verify_manifest_files(&paths.installation_root, &transaction.new_manifest)
}

pub(super) fn publish_candidate_record(
    transaction: &LoadedTransaction,
    injector: &mut impl FailureInjector,
) -> Result<(), TransactionRunError> {
    for (index, relative) in [INSTALLED_MANIFEST_FILE, INSTALLED_SIGNATURE_FILE]
        .into_iter()
        .enumerate()
    {
        mutate_with_checkpoint(
            injector,
            FileMutationPhase::PublishInstalledRecord,
            index,
            relative,
            || {
                replace_release_record_file(
                    transaction,
                    &transaction.paths.candidate_root().join(relative),
                    &transaction.paths.backup_root().join(relative),
                    &transaction.paths.installation_state_root.join(relative),
                    FileMutationPhase::PublishInstalledRecord,
                    index,
                    false,
                )
            },
        )?;
    }
    Ok(())
}

pub(super) fn restore_backup_record(
    transaction: &LoadedTransaction,
    injector: &mut impl FailureInjector,
) -> Result<(), TransactionRunError> {
    for (index, relative) in [
        IDENTITY_FILE,
        INSTALLED_MANIFEST_FILE,
        INSTALLED_SIGNATURE_FILE,
    ]
    .into_iter()
    .enumerate()
    {
        mutate_with_checkpoint(
            injector,
            FileMutationPhase::RestoreInstalledRecord,
            index,
            relative,
            || {
                if relative != IDENTITY_FILE {
                    remove_record_temp(
                        transaction,
                        &transaction.paths.candidate_root().join(relative),
                        FileMutationPhase::PublishInstalledRecord,
                        index - 1,
                    )?;
                }
                replace_release_record_file(
                    transaction,
                    &transaction.paths.backup_root().join(relative),
                    &transaction.paths.candidate_root().join(relative),
                    &transaction.paths.installation_state_root.join(relative),
                    FileMutationPhase::RestoreInstalledRecord,
                    index,
                    true,
                )
            },
        )?;
    }
    Ok(())
}

pub(super) fn replace_release_record_file(
    loaded: &LoadedTransaction,
    desired_source: &Path,
    alternate_source: &Path,
    destination: &Path,
    phase: FileMutationPhase,
    index: usize,
    allow_missing: bool,
) -> Result<(), UpdaterError> {
    let maximum = record_limit(
        destination
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| transaction("release record destination name is invalid"))?,
    );
    let desired = read_bounded(
        desired_source,
        maximum,
        "failed to read durable release record source",
    )?;
    let alternate = if alternate_source.exists() {
        Some(read_bounded(
            alternate_source,
            maximum,
            "failed to read durable release record alternate",
        )?)
    } else {
        None
    };
    match read_optional_bounded(destination, maximum)? {
        Some(current) if current == desired => return Ok(()),
        Some(current) if alternate.as_ref().is_some_and(|bytes| *bytes == current) => {}
        Some(_) => {
            return Err(recovery(
                "active release record contains unknown durable content",
            ));
        }
        None if allow_missing => {}
        None => return Err(recovery("active release record is unexpectedly missing")),
    }
    let parent = destination
        .parent()
        .ok_or_else(|| transaction("release record destination has no parent"))?;
    ensure_direct_directory(parent)?;
    let temporary = parent.join(operation_temp_name(loaded, phase, index));
    prepare_record_temp(&temporary, &desired)?;
    atomic_replace(&temporary, destination)
        .map_err(|_| transaction("failed to atomically replace active release record"))
}

pub(super) fn remove_record_temp(
    transaction: &LoadedTransaction,
    expected_source: &Path,
    phase: FileMutationPhase,
    index: usize,
) -> Result<(), UpdaterError> {
    let parent = transaction.paths.installation_state_root();
    let temporary = parent.join(operation_temp_name(transaction, phase, index));
    let expected = read_bounded(
        expected_source,
        record_limit(
            expected_source
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or_else(|| recovery("release record source name is invalid"))?,
        ),
        "failed to read release record temporary expectation",
    )?;
    match read_optional_bounded(&temporary, expected.len())? {
        None => Ok(()),
        Some(bytes) if bytes == expected => remove_file_synced(&temporary)
            .map_err(|_| recovery("failed to remove release record temporary file")),
        Some(_) => Err(recovery(
            "release record temporary file contains unknown content",
        )),
    }
}

pub(super) fn prepare_record_temp(temporary: &Path, expected: &[u8]) -> Result<(), UpdaterError> {
    match read_optional_bounded(temporary, expected.len())? {
        None => write_new_synced(temporary, expected),
        Some(bytes) if bytes == expected => Ok(()),
        Some(_) => Err(recovery(
            "release record temporary file contains unknown content",
        )),
    }
}

pub(super) fn record_limit(file_name: &str) -> usize {
    match file_name {
        IDENTITY_FILE => MAX_IDENTITY_BYTES,
        INSTALLED_MANIFEST_FILE => ah_updater_core::MAX_MANIFEST_BYTES,
        INSTALLED_SIGNATURE_FILE => ah_updater_core::DETACHED_SIGNATURE_BYTES,
        _ => 0,
    }
}

#[derive(Debug)]
pub(super) struct ReleaseRecord {
    pub(super) manifest: ReleaseManifest,
    pub(super) manifest_bytes: Vec<u8>,
    pub(super) signature_bytes: Vec<u8>,
}

pub(super) fn read_release_record(
    root: &Path,
    trust: &ReleaseTrust,
) -> Result<ReleaseRecord, UpdaterError> {
    let manifest_bytes = read_bounded(
        &root.join(INSTALLED_MANIFEST_FILE),
        ah_updater_core::MAX_MANIFEST_BYTES,
        "failed to read durable release manifest",
    )?;
    let signature_bytes = read_bounded(
        &root.join(INSTALLED_SIGNATURE_FILE),
        ah_updater_core::DETACHED_SIGNATURE_BYTES,
        "failed to read durable release signature",
    )?;
    verify_release_record(&manifest_bytes, &signature_bytes, trust)
}

pub(super) fn verify_release_record(
    manifest_bytes: &[u8],
    signature_bytes: &[u8],
    trust: &ReleaseTrust,
) -> Result<ReleaseRecord, UpdaterError> {
    let manifest = trust
        .verify(manifest_bytes, signature_bytes)?
        .into_manifest();
    Ok(ReleaseRecord {
        manifest,
        manifest_bytes: manifest_bytes.to_vec(),
        signature_bytes: signature_bytes.to_vec(),
    })
}

pub(super) fn decode_identity(bytes: &[u8]) -> Result<InstallationIdentityV1, UpdaterError> {
    let identity: InstallationIdentityV1 = serde_json::from_slice(bytes)
        .map_err(|_| transaction("installation identity backup is malformed"))?;
    identity.validate()?;
    Ok(identity)
}

pub(super) fn validate_identity(
    paths: &TransactionPaths,
    plan: &TransactionPlanV1,
    identity: &InstallationIdentityV1,
    old_manifest: &ReleaseManifest,
) -> Result<(), UpdaterError> {
    if identity.installation_id != plan.installation_id {
        return Err(transaction(
            "installation identity does not match the transaction plan",
        ));
    }
    let executable = Path::new(&identity.executable_path);
    if !executable.is_absolute()
        || executable.to_str().is_none()
        || executable
            .components()
            .any(|component| matches!(component, Component::CurDir | Component::ParentDir))
    {
        return Err(transaction(
            "installation identity executable path is not absolute and normalized",
        ));
    }
    if !old_manifest.files.iter().any(|file| {
        paths_equal(
            executable,
            &paths.installation_root.join(path_from_slashes(&file.path)),
        )
    }) {
        return Err(transaction(
            "installation identity executable is absent from the old manifest",
        ));
    }
    Ok(())
}
