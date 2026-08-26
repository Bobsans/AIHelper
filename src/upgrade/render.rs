//! What `ah upgrade` prints.
//!
//! The mechanism returns an outcome and this renders it. It used to call
//! `Emitter` from inside the activation, which is why `src/updater/` knew about
//! `GlobalOptions` at all - and a mechanism that renders cannot be tested
//! without deciding what its output should look like.

use ah_updater_core::{CheckStatus, UpdateOperation, UpdateSource, UpgradeCheckResultV1};

use crate::{
    cli::GlobalOptions,
    error::AppError,
    output::Emitter,
    updater::{activate::UpgradeLaunchResult, check::UpgradeOutcome},
};

/// # Errors
///
/// [`AppError`] when the output cannot be written.
pub(crate) fn outcome(outcome: &UpgradeOutcome, options: GlobalOptions) -> Result<(), AppError> {
    match outcome {
        UpgradeOutcome::Checked(result) => check(result, options),
        UpgradeOutcome::Launched(result) => launch(result, options),
    }
}

fn check(result: &UpgradeCheckResultV1, options: GlobalOptions) -> Result<(), AppError> {
    Emitter::stdio(&options).value(result, |_| {
        format!(
            "status={} current_version={} selected_version={} target={} source={}",
            check_status(result.status),
            result.current_version,
            result.selected_version.as_deref().unwrap_or("none"),
            result.target.as_deref().unwrap_or("none"),
            update_source(result.source),
        )
    })
}

fn launch(result: &UpgradeLaunchResult, options: GlobalOptions) -> Result<(), AppError> {
    Emitter::stdio(&options).value(result, |_| {
        format!(
            "operation={} status={} current_version={} selected_version={} target={} source={} activation={} managed_mcp_restoration={} rollback={}",
            operation_name(result.operation),
            result.status,
            result.current_version,
            result.selected_version,
            result.target,
            result.source,
            result.activation,
            result.managed_mcp_restoration,
            result.rollback,
        )
    })
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

fn operation_name(operation: UpdateOperation) -> &'static str {
    match operation {
        UpdateOperation::Upgrade => "upgrade",
        UpdateOperation::Version => "version",
        UpdateOperation::Rollback => "rollback",
        UpdateOperation::Check | UpdateOperation::Recovery => {
            unreachable!("activation result contains only update operations")
        }
    }
}
