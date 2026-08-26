use std::{
    ffi::OsStr,
    fmt::Write as _,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    time::Duration,
};

use ah_update_helper::apply::{
    TransactionPaths, load_permanent_backup, prepare_transaction, remove_completed_transaction,
};
use ah_updater_core::{
    CheckStatus, FilePurpose, ReleaseManifest, TransactionPlanV1, UpdateOperation, UpdaterError,
    UpdaterErrorCode, verify_discovered_release,
};
use semver::Version;
use serde::Serialize;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use ah_error::AppError;

use crate::Host;

use super::{
    candidate::prepare_candidate,
    github::GitHubReleaseClient,
    installation::{load_current_managed_installation, resolve_current_managed_installation},
    map_updater_error,
    request::UpgradeRequest,
    smoke::run_offline_smoke,
    trust::production_release_trust,
};

const LIFECYCLE_LOCK_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_ACTIVATION_HELPER_COPIES: usize = 32;

/// What an upgrade attempt did, in the shape it is reported in.
///
/// Owned rather than borrowed because it crosses out of here: the mechanism
/// returns it and the CLI renders it. That is one allocation per upgrade.
#[derive(Debug, Serialize)]
pub struct UpgradeLaunchResult {
    pub schema_version: u32,
    pub operation: UpdateOperation,
    pub status: &'static str,
    pub current_version: String,
    pub selected_version: String,
    pub target: String,
    pub source: &'static str,
    pub activation: &'static str,
    pub managed_mcp_restoration: &'static str,
    pub rollback: &'static str,
}

pub(crate) fn execute(
    request: UpgradeRequest,
    host: &Host<'_>,
) -> Result<UpgradeLaunchResult, AppError> {
    #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
    {
        execute_windows(request, host)
    }
    #[cfg(not(all(target_os = "windows", target_arch = "x86_64")))]
    {
        let _ = (request, host);
        Err(map_updater_error(UpdaterError::new(
            UpdaterErrorCode::UnsupportedPlatform,
            "self-update requires Windows x86_64",
        )))
    }
}

#[cfg(all(target_os = "windows", target_arch = "x86_64"))]
fn execute_windows(
    request: UpgradeRequest,
    host: &Host<'_>,
) -> Result<UpgradeLaunchResult, AppError> {
    if request == UpgradeRequest::Rollback {
        return execute_rollback(host);
    }
    let operation = match request {
        UpgradeRequest::Upgrade => UpdateOperation::Upgrade,
        UpgradeRequest::Version(_) => UpdateOperation::Version,
        UpgradeRequest::Check | UpgradeRequest::Rollback => {
            unreachable!("request uses another updater path")
        }
    };
    let current_version = Version::parse(env!("CARGO_PKG_VERSION"))
        .map_err(|_| map_updater_error(contract("running version is not canonical SemVer")))?;
    let trust = production_release_trust().map_err(map_updater_error)?;
    let source = GitHubReleaseClient::new().map_err(map_updater_error)?;
    let installation = resolve_current_managed_installation().map_err(map_updater_error)?;
    let discovered = match &request {
        UpgradeRequest::Upgrade => source.discover(),
        UpgradeRequest::Version(version) => source.discover_version(version),
        UpgradeRequest::Check | UpgradeRequest::Rollback => {
            unreachable!("request uses another updater path")
        }
    }
    .map_err(map_updater_error)?;
    let manifest_bytes = source
        .download_asset(&discovered.assets.manifest)
        .map_err(map_updater_error)?;
    let signature_bytes = source
        .download_asset(&discovered.assets.signature)
        .map_err(map_updater_error)?;
    let verified = verify_discovered_release(
        discovered,
        &manifest_bytes,
        &signature_bytes,
        &trust,
        &current_version,
    )
    .map_err(map_updater_error)?;
    let selected_version = verified.discovered().version.version().clone();
    let selected_version_text = selected_version.to_string();
    let status = verified.check_result().status;
    if status != CheckStatus::UpdateAvailable {
        if matches!(request, UpgradeRequest::Version(_)) && status == CheckStatus::CurrentNewer {
            return Err(map_updater_error(UpdaterError::new(
                UpdaterErrorCode::Compatibility,
                "requested release would downgrade the installed version",
            )));
        }
        return Ok(UpgradeLaunchResult {
            schema_version: 1,
            operation,
            status: match status {
                CheckStatus::UpToDate => "up_to_date",
                CheckStatus::CurrentNewer => "current_newer",
                CheckStatus::UpdateAvailable => unreachable!(),
            },
            current_version: env!("CARGO_PKG_VERSION").to_owned(),
            selected_version: selected_version_text,
            target: verified.discovered().target.rust_target.to_owned(),
            source: "github_release",
            activation: "not_required",
            managed_mcp_restoration: "not_required",
            rollback: "not_required",
        });
    }

    let prepared = prepare_candidate(&source, verified, installation.state_root())
        .and_then(|prepared| run_offline_smoke(prepared, host.smoke))
        .map_err(map_updater_error)?;
    launch_transaction(
        operation,
        &installation,
        prepared.root(),
        prepared.verified_release().manifest(),
        prepared.verified_release().manifest_bytes(),
        prepared.verified_release().signature_bytes(),
        &selected_version_text,
        "github_release",
        &trust,
        host,
    )
}

#[cfg(all(target_os = "windows", target_arch = "x86_64"))]
fn execute_rollback(host: &Host<'_>) -> Result<UpgradeLaunchResult, AppError> {
    let trust = production_release_trust().map_err(map_updater_error)?;
    let installation = load_current_managed_installation(&trust).map_err(map_updater_error)?;
    let backup = load_permanent_backup(
        installation.portable().root(),
        installation.state_root(),
        installation.identity().installation_id,
        &trust,
    )
    .map_err(|_| {
        map_updater_error(UpdaterError::new(
            UpdaterErrorCode::Rollback,
            "no verified permanent backup is available",
        ))
    })?;
    let selected_version = backup.manifest().release.version.clone();
    launch_transaction(
        UpdateOperation::Rollback,
        &installation,
        &backup.files_root(),
        backup.manifest(),
        backup.manifest_bytes(),
        backup.signature_bytes(),
        &selected_version,
        "permanent_backup",
        &trust,
        host,
    )
}

#[cfg(all(target_os = "windows", target_arch = "x86_64"))]
#[allow(clippy::too_many_arguments)]
fn launch_transaction(
    operation: UpdateOperation,
    installation: &super::installation::ManagedInstallation,
    candidate_root: &Path,
    candidate_manifest: &ReleaseManifest,
    candidate_manifest_bytes: &[u8],
    candidate_signature_bytes: &[u8],
    selected_version: &str,
    source: &'static str,
    trust: &ah_updater_core::ReleaseTrust,
    host: &Host<'_>,
) -> Result<UpgradeLaunchResult, AppError> {
    let transaction_id = Uuid::new_v4();
    let transactions = installation.state_root().join("transactions");
    ensure_directory(&transactions).map_err(map_updater_error)?;
    let paths = TransactionPaths::new(
        installation.portable().root(),
        installation.state_root(),
        transactions.join(transaction_id.to_string()),
    );

    let hold = host.service.hold(LIFECYCLE_LOCK_TIMEOUT)?;
    cleanup_activation_helpers(installation.state_root()).map_err(map_updater_error)?;
    let mcp_state = host.service.capture(&hold)?;
    let plan = TransactionPlanV1::build_for_operation(
        operation,
        transaction_id,
        installation.identity().installation_id,
        installation.manifest(),
        candidate_manifest,
    )
    .and_then(|plan| {
        plan.with_managed_mcp_state(mcp_state.was_running, mcp_state.previous_instance_id)
    })
    .map_err(map_updater_error)?;
    prepare_transaction(
        &paths,
        &plan,
        candidate_root,
        candidate_manifest_bytes,
        candidate_signature_bytes,
        trust,
    )
    .map_err(map_updater_error)?;
    let (helper_root, helper_manifest) = if operation == UpdateOperation::Rollback {
        (installation.portable().root(), installation.manifest())
    } else {
        (candidate_root, candidate_manifest)
    };
    let helper = match copy_activation_helper(
        helper_root,
        helper_manifest,
        installation.state_root(),
        transaction_id,
    ) {
        Ok(helper) => helper,
        Err(error) => {
            let _ = remove_completed_transaction(&paths, trust);
            return Err(map_updater_error(error));
        }
    };
    let stopped = match host.service.stop(&hold) {
        Ok(stopped) => stopped,
        Err(error) => {
            let _ = host.service.restore(&hold, mcp_state);
            let _ = remove_completed_transaction(&paths, trust);
            let _ = fs::remove_file(&helper);
            return Err(error);
        }
    };
    if stopped != mcp_state.was_running {
        let _ = host.service.restore(&hold, mcp_state);
        let _ = remove_completed_transaction(&paths, trust);
        let _ = fs::remove_file(&helper);
        return Err(AppError::external(
            "UPDATER_ACTIVATION",
            "managed MCP state changed while the update was being prepared",
        ));
    }

    let helper_operation = if operation == UpdateOperation::Rollback {
        "rollback"
    } else {
        "activate"
    };
    let arguments = [
        OsStr::new(helper_operation),
        OsStr::new("--installation-root"),
        paths.installation_root().as_os_str(),
        OsStr::new("--installation-state-root"),
        paths.installation_state_root().as_os_str(),
        OsStr::new("--transaction-root"),
        paths.transaction_root().as_os_str(),
    ];
    let launch = if operation == UpdateOperation::Rollback {
        super::handoff::launch_rollback(&helper, &arguments, &hold)
    } else {
        super::handoff::launch_activation(&helper, &arguments, &hold)
    };
    if let Err(failure) = launch {
        if failure.cleanup_safe() {
            let restore = host.service.restore(&hold, mcp_state);
            let _ = remove_completed_transaction(&paths, trust);
            let _ = fs::remove_file(&helper);
            restore?;
        }
        return Err(failure.into_error());
    }

    Ok(UpgradeLaunchResult {
        schema_version: 1,
        operation,
        status: "activation_launched",
        current_version: installation.manifest().release.version.clone(),
        selected_version: selected_version.to_owned(),
        target: candidate_manifest.release.target.clone(),
        source,
        activation: "launched",
        managed_mcp_restoration: if mcp_state.was_running {
            "pending"
        } else {
            "not_required"
        },
        rollback: if operation == UpdateOperation::Rollback {
            "launched"
        } else {
            "not_required"
        },
    })
}

#[cfg(all(target_os = "windows", target_arch = "x86_64"))]
pub fn copy_activation_helper(
    candidate_root: &Path,
    candidate_manifest: &ReleaseManifest,
    state_root: &Path,
    transaction_id: Uuid,
) -> Result<PathBuf, UpdaterError> {
    let helpers = candidate_manifest
        .files
        .iter()
        .filter(|file| file.purpose == FilePurpose::UpdateHelper)
        .collect::<Vec<_>>();
    let [expected] = helpers.as_slice() else {
        return Err(candidate_error(
            "signed candidate must contain exactly one update helper",
        ));
    };
    let source = candidate_root.join(expected.path.split('/').collect::<PathBuf>());
    let destination = state_root.join(format!("activation-helper-{transaction_id}.exe"));
    let mut input = File::open(source)
        .map_err(|_| candidate_error("failed to open verified activation helper"))?;
    let mut output = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&destination)
        .map_err(|_| candidate_error("failed to create private activation helper copy"))?;
    let mut hasher = Sha256::new();
    let mut size = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = input
            .read(&mut buffer)
            .map_err(|_| candidate_error("failed to read verified activation helper"))?;
        if read == 0 {
            break;
        }
        output
            .write_all(&buffer[..read])
            .map_err(|_| candidate_error("failed to copy verified activation helper"))?;
        hasher.update(&buffer[..read]);
        size += read as u64;
    }
    output
        .sync_all()
        .map_err(|_| candidate_error("failed to sync verified activation helper"))?;
    let mut sha256 = String::with_capacity(64);
    for byte in hasher.finalize() {
        write!(sha256, "{byte:02x}").expect("writing to String cannot fail");
    }
    if size != expected.size || sha256 != expected.sha256 {
        let _ = fs::remove_file(&destination);
        return Err(candidate_error(
            "activation helper copy failed verification",
        ));
    }
    Ok(destination)
}

#[cfg(all(target_os = "windows", target_arch = "x86_64"))]
pub fn cleanup_activation_helpers(state_root: &Path) -> Result<(), UpdaterError> {
    let entries = fs::read_dir(state_root)
        .map_err(|_| candidate_error("failed to enumerate activation helper copies"))?;
    let mut count = 0_usize;
    for entry in entries {
        let entry =
            entry.map_err(|_| candidate_error("failed to enumerate activation helper copies"))?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        let Some(id) = name
            .strip_prefix("activation-helper-")
            .and_then(|name| name.strip_suffix(".exe"))
        else {
            continue;
        };
        if Uuid::parse_str(id).is_err() {
            continue;
        }
        count += 1;
        if count > MAX_ACTIVATION_HELPER_COPIES {
            return Err(candidate_error(
                "activation helper copy count exceeds the limit",
            ));
        }
        let metadata = fs::symlink_metadata(entry.path())
            .map_err(|_| candidate_error("failed to inspect activation helper copy"))?;
        use std::os::windows::fs::MetadataExt as _;
        if !metadata.is_file()
            || metadata.file_type().is_symlink()
            || metadata.file_attributes() & 0x400 != 0
        {
            return Err(candidate_error(
                "activation helper copy is not a direct file",
            ));
        }
        fs::remove_file(entry.path())
            .map_err(|_| candidate_error("failed to remove stale activation helper copy"))?;
    }
    Ok(())
}

#[cfg(all(target_os = "windows", target_arch = "x86_64"))]
fn ensure_directory(path: &Path) -> Result<(), UpdaterError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => Ok(()),
        Ok(_) => Err(UpdaterError::new(
            UpdaterErrorCode::Installation,
            "update transaction parent is not a direct directory",
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir(path).map_err(|_| {
                UpdaterError::new(
                    UpdaterErrorCode::Installation,
                    "failed to create update transaction parent",
                )
            })
        }
        Err(_) => Err(UpdaterError::new(
            UpdaterErrorCode::Installation,
            "failed to inspect update transaction parent",
        )),
    }
}

fn contract(detail: &'static str) -> UpdaterError {
    UpdaterError::new(UpdaterErrorCode::ReleaseContract, detail)
}

fn candidate_error(detail: &'static str) -> UpdaterError {
    UpdaterError::new(UpdaterErrorCode::Candidate, detail)
}
