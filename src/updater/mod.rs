mod activate;
pub(crate) mod candidate;
pub(crate) mod check;
pub(crate) mod command;
pub(crate) mod github;
#[cfg(windows)]
mod handoff;
mod installation;
pub(crate) mod recovery;
pub(crate) mod service;
mod smoke;
mod trust;

pub(crate) use check::execute;

use ah_updater_core::UpdaterError;

use crate::error::AppError;

/// The updater's errors, reported under the code the core assigned them.
///
/// Two identical copies of this lived in `activate` and `check`. `recovery` has
/// a third that is deliberately *not* this one: everything that fails during
/// recovery reports as `UPDATER_RECOVERY` with the original code in the detail,
/// because a user whose invocation was consumed by recovery needs to see which
/// phase consumed it, not which primitive failed inside it.
fn map_updater_error(error: UpdaterError) -> AppError {
    AppError::external(
        format!("UPDATER_{}", error.code().as_str().to_ascii_uppercase()),
        error.detail(),
    )
}
