//! AIHelper's self-update: check, download, verify, activate, roll back and
//! recover.
//!
//! Everything here is mechanism. It renders nothing and reads no command line -
//! `ah`'s `upgrade` module does both, and supplies the two ports [`Host`]
//! names.
//!
//! The order of the steps is the safety argument: nothing is trusted until it
//! is verified, nothing is replaced until a backup exists, and every state a
//! crash can leave is one the recovery path can name.
//!
//! Off Windows an upgrade refuses with `UnsupportedPlatform` before it reaches
//! any of that, so activation, installation and recovery are unreachable there
//! by construction. The two lints that report it are relaxed for that build
//! only; on Windows, where the code is live, both still apply.

#![cfg_attr(not(windows), allow(dead_code, unused_imports))]

pub mod activate;
pub mod candidate;
pub mod check;
pub mod github;
#[cfg(windows)]
mod handoff;
mod installation;
pub mod recovery;
pub mod request;
pub mod service;
pub mod smoke;
mod trust;

pub use check::execute;

use ah_updater_core::UpdaterError;

use ah_error::AppError;

/// What the updater needs from the process it runs inside.
///
/// Two ports, because the mechanism must not reach for either itself: it may
/// have to hold a managed service still while it swaps the binary under it, and
/// it has to run the candidate binary before trusting it. The implementations
/// are the CLI's - `mcp_service::lifecycle::guard` and `upgrade`'s bounded
/// runner - which is exactly what an extracted crate must not know.
pub struct Host<'a> {
    pub service: &'a dyn service::ServiceGuard,
    pub smoke: &'a dyn smoke::SmokeRunner,
}

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
