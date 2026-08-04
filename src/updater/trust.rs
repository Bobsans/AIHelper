use ah_updater_core::{ReleaseTrust, UpdaterError};

pub(super) fn production_release_trust() -> Result<ReleaseTrust, UpdaterError> {
    ah_updater_core::production_release_trust()
}
