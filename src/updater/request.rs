//! What one `ah upgrade` invocation asks for.
//!
//! The mechanism's input. Reading it off the command line is the CLI's job and
//! lives in `upgrade::route`.

use semver::Version;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpgradeRequest {
    Check,
    Upgrade,
    Version(Version),
    Rollback,
}
