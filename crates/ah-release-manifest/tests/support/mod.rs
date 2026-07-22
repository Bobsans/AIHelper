use ah_release_manifest::{
    SIGNING_ALGORITHM, SIGNING_DOMAIN, TrustedKey, TrustedKeyRegistry, key_id_for_public_key,
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use ed25519_dalek::{Signer, SigningKey};

// Deterministic non-production seed used only by release-manifest tests.
const TEST_ONLY_SIGNING_SEED: [u8; 32] = [0x42; 32];

pub fn signing_key() -> SigningKey {
    SigningKey::from_bytes(&TEST_ONLY_SIGNING_SEED)
}

fn secondary_signing_key() -> SigningKey {
    let mut seed = TEST_ONLY_SIGNING_SEED;
    seed[0] ^= 0xff;
    SigningKey::from_bytes(&seed)
}

pub fn trusted_key(signing_key: &SigningKey) -> TrustedKey {
    let public_key = signing_key.verifying_key().to_bytes();
    TrustedKey {
        key_id: key_id_for_public_key(&public_key),
        algorithm: SIGNING_ALGORITHM.to_owned(),
        public_key,
    }
}

pub fn registry() -> TrustedKeyRegistry {
    TrustedKeyRegistry::new(vec![trusted_key(&signing_key())]).unwrap()
}

pub fn primary_trusted_key() -> TrustedKey {
    trusted_key(&signing_key())
}

pub fn secondary_trusted_key() -> TrustedKey {
    trusted_key(&secondary_signing_key())
}

pub fn two_key_registry() -> TrustedKeyRegistry {
    TrustedKeyRegistry::new(vec![primary_trusted_key(), secondary_trusted_key()]).unwrap()
}

pub fn sign_manifest(manifest_bytes: &[u8], signing_key: &SigningKey) -> Vec<u8> {
    let mut message = Vec::with_capacity(SIGNING_DOMAIN.len() + manifest_bytes.len());
    message.extend_from_slice(SIGNING_DOMAIN);
    message.extend_from_slice(manifest_bytes);
    URL_SAFE_NO_PAD
        .encode(signing_key.sign(&message).to_bytes())
        .into_bytes()
}

pub fn sign_with_primary(manifest_bytes: &[u8]) -> Vec<u8> {
    sign_manifest(manifest_bytes, &signing_key())
}

pub fn sign_with_secondary(manifest_bytes: &[u8]) -> Vec<u8> {
    sign_manifest(manifest_bytes, &secondary_signing_key())
}
