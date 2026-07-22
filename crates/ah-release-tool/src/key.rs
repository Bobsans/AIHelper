use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use ed25519_dalek::{Signer as _, SigningKey};
use zeroize::Zeroizing;

use ah_release_manifest::{
    SIGNING_ALGORITHM, SIGNING_DOMAIN, TrustedKey, TrustedKeyRegistry, key_id_for_public_key,
};

use crate::ReleaseToolError;

const ENCODED_KEY_BYTES: usize = 43;

pub struct SigningMaterial {
    signing_key: SigningKey,
    public_key: [u8; 32],
    key_id: String,
}

impl SigningMaterial {
    pub fn parse(
        encoded_seed: &str,
        encoded_expected_public_key: &str,
    ) -> Result<Self, ReleaseToolError> {
        let seed = decode_secret_seed(encoded_seed)?;
        let expected_public_key = decode_public_key(encoded_expected_public_key)?;
        let signing_key = SigningKey::from_bytes(&seed);
        let public_key = signing_key.verifying_key().to_bytes();
        if public_key != expected_public_key {
            return Err(ReleaseToolError::signing(
                "signing seed does not match the configured public key",
            ));
        }
        let key_id = key_id_for_public_key(&public_key);
        Ok(Self {
            signing_key,
            public_key,
            key_id,
        })
    }

    pub fn public_key(&self) -> [u8; 32] {
        self.public_key
    }

    pub fn key_id(&self) -> &str {
        &self.key_id
    }

    pub fn sign_manifest(&self, canonical_manifest: &[u8]) -> String {
        let mut preimage = Vec::with_capacity(SIGNING_DOMAIN.len() + canonical_manifest.len());
        preimage.extend_from_slice(SIGNING_DOMAIN);
        preimage.extend_from_slice(canonical_manifest);
        let signature = self.signing_key.sign(&preimage);
        URL_SAFE_NO_PAD.encode(signature.to_bytes())
    }

    pub fn trusted_registry(&self) -> Result<TrustedKeyRegistry, ReleaseToolError> {
        TrustedKeyRegistry::new(vec![TrustedKey {
            key_id: self.key_id.clone(),
            algorithm: SIGNING_ALGORITHM.to_owned(),
            public_key: self.public_key,
        }])
        .map_err(|error| ReleaseToolError::manifest(error.to_string()))
    }
}

fn decode_secret_seed(encoded: &str) -> Result<Zeroizing<[u8; 32]>, ReleaseToolError> {
    let decoded = decode_key(encoded, "signing seed")?;
    Ok(Zeroizing::new(decoded))
}

fn decode_public_key(encoded: &str) -> Result<[u8; 32], ReleaseToolError> {
    decode_key(encoded, "public key")
}

fn decode_key(encoded: &str, label: &'static str) -> Result<[u8; 32], ReleaseToolError> {
    if encoded.len() != ENCODED_KEY_BYTES
        || !encoded.is_ascii()
        || !encoded
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
    {
        return Err(ReleaseToolError::signing(format!(
            "{label} must be exactly 43 unpadded Base64URL characters"
        )));
    }
    let mut decoded = [0_u8; 32];
    let decoded_len = URL_SAFE_NO_PAD
        .decode_slice(encoded, &mut decoded)
        .map_err(|_| {
            ReleaseToolError::signing(format!("{label} is not valid unpadded Base64URL"))
        })?;
    if decoded_len != decoded.len() {
        return Err(ReleaseToolError::signing(format!(
            "{label} must decode to exactly 32 bytes"
        )));
    }
    Ok(decoded)
}
