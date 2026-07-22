use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use ed25519_dalek::Signature;
use serde::Deserialize;

use crate::{
    DETACHED_SIGNATURE_BYTES, MAX_MANIFEST_BYTES, ManifestError, ReleaseManifest, SCHEMA_VERSION,
    SIGNING_ALGORITHM, SIGNING_DOMAIN, TrustedKeyRegistry, canonical::decode_canonical_untrusted,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedManifest {
    manifest: ReleaseManifest,
}

impl VerifiedManifest {
    pub fn manifest(&self) -> &ReleaseManifest {
        &self.manifest
    }

    pub fn into_manifest(self) -> ReleaseManifest {
        self.manifest
    }
}

#[derive(Deserialize)]
struct UntrustedSelectors {
    schema_version: u32,
    signing: UntrustedSigningSelectors,
}

#[derive(Deserialize)]
struct UntrustedSigningSelectors {
    key_id: String,
    algorithm: String,
}

pub fn verify_manifest(
    manifest_bytes: &[u8],
    signature_bytes: &[u8],
    registry: &TrustedKeyRegistry,
) -> Result<VerifiedManifest, ManifestError> {
    bound_inputs(manifest_bytes, signature_bytes)?;

    let selectors =
        serde_json::from_slice::<UntrustedSelectors>(manifest_bytes).map_err(|error| {
            ManifestError::MalformedJson {
                detail: error.to_string(),
            }
        })?;
    if selectors.schema_version != SCHEMA_VERSION {
        return Err(ManifestError::UnsupportedSchema {
            found: selectors.schema_version,
        });
    }
    if selectors.signing.algorithm != SIGNING_ALGORITHM {
        return Err(ManifestError::UnsupportedSignatureAlgorithm {
            algorithm: selectors.signing.algorithm,
        });
    }

    let manifest = decode_canonical_untrusted(manifest_bytes)?;
    let verifying_key =
        registry.resolve(&selectors.signing.key_id, &selectors.signing.algorithm)?;
    let signature = parse_signature(signature_bytes)?;

    let mut message = Vec::with_capacity(SIGNING_DOMAIN.len() + manifest_bytes.len());
    message.extend_from_slice(SIGNING_DOMAIN);
    message.extend_from_slice(manifest_bytes);
    verifying_key
        .verify_strict(&message, &signature)
        .map_err(|_| ManifestError::InvalidSignature)?;

    manifest.validate()?;
    Ok(VerifiedManifest { manifest })
}

fn bound_inputs(manifest_bytes: &[u8], signature_bytes: &[u8]) -> Result<(), ManifestError> {
    if manifest_bytes.len() > MAX_MANIFEST_BYTES {
        return Err(ManifestError::InputTooLarge {
            actual: manifest_bytes.len(),
            maximum: MAX_MANIFEST_BYTES,
        });
    }
    if signature_bytes.len() != DETACHED_SIGNATURE_BYTES {
        return Err(ManifestError::MalformedSignature {
            detail: format!(
                "must contain exactly {DETACHED_SIGNATURE_BYTES} ASCII bytes; found {}",
                signature_bytes.len()
            ),
        });
    }
    Ok(())
}

fn parse_signature(raw: &[u8]) -> Result<Signature, ManifestError> {
    if !raw
        .iter()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(ManifestError::MalformedSignature {
            detail: "must use the unpadded Base64URL alphabet".to_owned(),
        });
    }

    let decoded = URL_SAFE_NO_PAD
        .decode(raw)
        .map_err(|_| ManifestError::MalformedSignature {
            detail: "is not valid unpadded Base64URL".to_owned(),
        })?;
    let bytes: [u8; 64] =
        decoded
            .try_into()
            .map_err(|decoded: Vec<u8>| ManifestError::MalformedSignature {
                detail: format!("must decode to 64 bytes; found {}", decoded.len()),
            })?;
    Ok(Signature::from_bytes(&bytes))
}
