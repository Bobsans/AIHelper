use crate::{ReleaseTrust, ReleaseTrustAnchor, UpdaterError};

const PRODUCTION_TRUST_ANCHORS: &[ReleaseTrustAnchor] = &[];

pub fn production_release_trust() -> Result<ReleaseTrust, UpdaterError> {
    ReleaseTrust::from_anchors(PRODUCTION_TRUST_ANCHORS)
}

#[cfg(test)]
mod tests {
    use crate::UpdaterErrorCode;

    use super::*;

    #[test]
    fn production_trust_fails_closed_until_external_key_activation() {
        let error = production_release_trust().unwrap_err();
        assert_eq!(error.code(), UpdaterErrorCode::Trust);
        assert!(error.detail().contains("at least one public key"));
    }
}
