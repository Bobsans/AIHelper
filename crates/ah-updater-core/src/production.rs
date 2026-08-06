use crate::{ReleaseTrust, ReleaseTrustAnchor, UpdaterError};

const PRODUCTION_TRUST_ANCHORS: &[ReleaseTrustAnchor] = &[ReleaseTrustAnchor {
    key_id: "ed25519-sha256-5ba0dbdadcbf6973891194448d6496c5e70a4e1d419fc325f445a8ad5d2a180f",
    public_key: [
        0x0a, 0xbd, 0xd6, 0x2c, 0xf1, 0xac, 0x4b, 0x93, 0xd8, 0xaa, 0x2c, 0xa0, 0x95, 0xd5, 0x1a,
        0x13, 0xab, 0xf2, 0x43, 0x7b, 0xc6, 0xcb, 0xf4, 0x32, 0xf2, 0x4f, 0x30, 0xd3, 0x43, 0x2a,
        0x6e, 0xe8,
    ],
}];

pub fn production_release_trust() -> Result<ReleaseTrust, UpdaterError> {
    ReleaseTrust::from_anchors(PRODUCTION_TRUST_ANCHORS)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn production_trust_contains_activated_key() {
        let trust = production_release_trust().unwrap();
        assert_eq!(trust.key_count(), 1);
    }
}
