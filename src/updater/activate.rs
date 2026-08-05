use std::{
    ffi::OsStr,
    fmt::Write as _,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    time::Duration,
};

use ah_update_helper::transaction::{
    TransactionPaths, prepare_transaction, remove_completed_transaction,
};
use ah_updater_core::{
    CheckStatus, FilePurpose, TransactionPlanV1, UpdateOperation, UpdateSource, UpdaterError,
    UpdaterErrorCode, verify_discovered_release,
};
use semver::Version;
use serde::Serialize;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::{
    cli::GlobalOptions,
    error::AppError,
    mcp_service::{
        lifecycle::{
            capture_for_update_while_locked, restore_for_update_while_locked,
            stop_for_update_while_locked,
        },
        lock::FileLease,
        paths::ServicePaths,
    },
    output::OutputMode,
};

use super::{
    candidate::prepare_candidate, command::UpgradeRequest, github::GitHubReleaseClient,
    installation::resolve_current_managed_installation, smoke::run_offline_smoke,
    trust::production_release_trust,
};

const LIFECYCLE_LOCK_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Serialize)]
struct UpgradeLaunchResult<'a> {
    schema_version: u32,
    operation: UpdateOperation,
    status: &'a str,
    current_version: &'a str,
    selected_version: &'a str,
    target: &'a str,
    source: UpdateSource,
    activation: &'a str,
    managed_mcp_restoration: &'a str,
    rollback: &'a str,
}

pub(super) fn execute(request: UpgradeRequest, options: GlobalOptions) -> Result<(), AppError> {
    #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
    {
        execute_windows(request, options)
    }
    #[cfg(not(all(target_os = "windows", target_arch = "x86_64")))]
    {
        let _ = (request, options);
        Err(map_updater_error(UpdaterError::new(
            UpdaterErrorCode::UnsupportedPlatform,
            "self-update requires Windows x86_64",
        )))
    }
}

#[cfg(all(target_os = "windows", target_arch = "x86_64"))]
fn execute_windows(request: UpgradeRequest, options: GlobalOptions) -> Result<(), AppError> {
    let operation = match request {
        UpgradeRequest::Upgrade => UpdateOperation::Upgrade,
        UpgradeRequest::Version(_) => UpdateOperation::Version,
        UpgradeRequest::Check => unreachable!("check requests use the read-only path"),
    };
    let current_version = Version::parse(env!("CARGO_PKG_VERSION"))
        .map_err(|_| map_updater_error(contract("running version is not canonical SemVer")))?;
    let trust = production_release_trust().map_err(map_updater_error)?;
    let source = GitHubReleaseClient::new().map_err(map_updater_error)?;
    let installation = resolve_current_managed_installation().map_err(map_updater_error)?;
    let discovered = match &request {
        UpgradeRequest::Upgrade => source.discover(),
        UpgradeRequest::Version(version) => source.discover_version(version),
        UpgradeRequest::Check => unreachable!("check requests use the read-only path"),
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
        return render(
            &UpgradeLaunchResult {
                schema_version: 1,
                operation,
                status: match status {
                    CheckStatus::UpToDate => "up_to_date",
                    CheckStatus::CurrentNewer => "current_newer",
                    CheckStatus::UpdateAvailable => unreachable!(),
                },
                current_version: env!("CARGO_PKG_VERSION"),
                selected_version: &selected_version_text,
                target: verified.discovered().target.rust_target,
                source: UpdateSource::GitHubRelease,
                activation: "not_required",
                managed_mcp_restoration: "not_required",
                rollback: "not_required",
            },
            options,
        );
    }

    let prepared = prepare_candidate(&source, verified, installation.state_root())
        .and_then(run_offline_smoke)
        .map_err(map_updater_error)?;
    let transaction_id = Uuid::new_v4();
    let transactions = installation.state_root().join("transactions");
    ensure_directory(&transactions).map_err(map_updater_error)?;
    let paths = TransactionPaths::new(
        installation.portable().root(),
        installation.state_root(),
        transactions.join(transaction_id.to_string()),
    );

    let service_paths = ServicePaths::discover()?;
    let lease = FileLease::acquire(&service_paths.lifecycle_lock, LIFECYCLE_LOCK_TIMEOUT)?;
    let mcp_state = capture_for_update_while_locked()?;
    let plan = TransactionPlanV1::build(
        transaction_id,
        installation.identity().installation_id,
        installation.manifest(),
        prepared.verified_release().manifest(),
    )
    .and_then(|plan| {
        plan.with_managed_mcp_state(mcp_state.was_running, mcp_state.previous_instance_id)
    })
    .map_err(map_updater_error)?;
    prepare_transaction(
        &paths,
        &plan,
        prepared.root(),
        prepared.verified_release().manifest_bytes(),
        prepared.verified_release().signature_bytes(),
        &trust,
    )
    .map_err(map_updater_error)?;
    let helper = match copy_activation_helper(&prepared, installation.state_root(), transaction_id)
    {
        Ok(helper) => helper,
        Err(error) => {
            let _ = remove_completed_transaction(&paths, &trust);
            return Err(map_updater_error(error));
        }
    };
    let stopped = match stop_for_update_while_locked() {
        Ok(stopped) => stopped,
        Err(error) => {
            let _ = restore_for_update_while_locked(mcp_state);
            let _ = remove_completed_transaction(&paths, &trust);
            let _ = fs::remove_file(&helper);
            return Err(error);
        }
    };
    if stopped != mcp_state.was_running {
        let _ = restore_for_update_while_locked(mcp_state);
        let _ = remove_completed_transaction(&paths, &trust);
        let _ = fs::remove_file(&helper);
        return Err(AppError::external(
            "UPDATER_ACTIVATION",
            "managed MCP state changed while the update was being prepared",
        ));
    }

    if let Err(error) = super::handoff::launch_activation(
        &helper,
        &[
            OsStr::new("activate"),
            OsStr::new("--installation-root"),
            paths.installation_root().as_os_str(),
            OsStr::new("--installation-state-root"),
            paths.installation_state_root().as_os_str(),
            OsStr::new("--transaction-root"),
            paths.transaction_root().as_os_str(),
        ],
        &lease,
    ) {
        let restore = restore_for_update_while_locked(mcp_state);
        let _ = remove_completed_transaction(&paths, &trust);
        let _ = fs::remove_file(&helper);
        restore?;
        return Err(error);
    }

    render(
        &UpgradeLaunchResult {
            schema_version: 1,
            operation,
            status: "activation_launched",
            current_version: env!("CARGO_PKG_VERSION"),
            selected_version: &selected_version_text,
            target: prepared.verified_release().discovered().target.rust_target,
            source: UpdateSource::GitHubRelease,
            activation: "launched",
            managed_mcp_restoration: if mcp_state.was_running {
                "pending"
            } else {
                "not_required"
            },
            rollback: "not_required",
        },
        options,
    )
}

#[cfg(all(target_os = "windows", target_arch = "x86_64"))]
fn copy_activation_helper(
    candidate: &super::smoke::SmokeCheckedCandidate,
    state_root: &Path,
    transaction_id: Uuid,
) -> Result<PathBuf, UpdaterError> {
    let helpers = candidate
        .verified_release()
        .manifest()
        .files
        .iter()
        .filter(|file| file.purpose == FilePurpose::UpdateHelper)
        .collect::<Vec<_>>();
    let [expected] = helpers.as_slice() else {
        return Err(candidate_error(
            "signed candidate must contain exactly one update helper",
        ));
    };
    let source = candidate
        .root()
        .join(expected.path.split('/').collect::<PathBuf>());
    // ponytail: successful helper copies remain until Task 11 adds bounded staging cleanup.
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

fn render(result: &UpgradeLaunchResult<'_>, options: GlobalOptions) -> Result<(), AppError> {
    if options.quiet {
        return Ok(());
    }
    match options.output {
        OutputMode::Json => println!("{}", serde_json::to_string_pretty(result)?),
        OutputMode::Text => println!(
            "operation={} status={} current_version={} selected_version={} target={} source=github_release activation={} managed_mcp_restoration={} rollback={}",
            operation_name(result.operation),
            result.status,
            result.current_version,
            result.selected_version,
            result.target,
            result.activation,
            result.managed_mcp_restoration,
            result.rollback,
        ),
    }
    Ok(())
}

fn operation_name(operation: UpdateOperation) -> &'static str {
    match operation {
        UpdateOperation::Upgrade => "upgrade",
        UpdateOperation::Version => "version",
        UpdateOperation::Check | UpdateOperation::Rollback | UpdateOperation::Recovery => {
            unreachable!("activation result contains only update operations")
        }
    }
}

fn contract(detail: &'static str) -> UpdaterError {
    UpdaterError::new(UpdaterErrorCode::ReleaseContract, detail)
}

fn candidate_error(detail: &'static str) -> UpdaterError {
    UpdaterError::new(UpdaterErrorCode::Candidate, detail)
}

fn map_updater_error(error: UpdaterError) -> AppError {
    AppError::external(
        format!("UPDATER_{}", error.code().as_str().to_ascii_uppercase()),
        error.detail(),
    )
}
