use std::collections::BTreeSet;
use std::fmt::Write;

use ed25519_dalek::VerifyingKey;
use sha2::{Digest, Sha256};

use crate::{ManifestError, SIGNING_ALGORITHM};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrustedKey {
    pub key_id: String,
    pub algorithm: String,
    pub public_key: [u8; 32],
}

#[derive(Debug, Clone)]
struct RegisteredKey {
    key_id: String,
    algorithm: String,
    verifying_key: VerifyingKey,
}

#[derive(Debug, Clone)]
pub struct TrustedKeyRegistry {
    keys: Vec<RegisteredKey>,
}

impl TrustedKeyRegistry {
    pub fn new(keys: Vec<TrustedKey>) -> Result<Self, ManifestError> {
        let mut registered = Vec::with_capacity(keys.len());
        let mut key_ids = BTreeSet::new();

        for key in keys {
            if !key_ids.insert(key.key_id.clone()) {
                return Err(ManifestError::InvalidTrustRegistry {
                    detail: format!("duplicate key ID '{}'", key.key_id),
                });
            }
            if key.algorithm != SIGNING_ALGORITHM {
                return Err(ManifestError::InvalidTrustRegistry {
                    detail: format!(
                        "key '{}' uses unsupported algorithm '{}'",
                        key.key_id, key.algorithm
                    ),
                });
            }

            let verifying_key = VerifyingKey::from_bytes(&key.public_key).map_err(|_| {
                ManifestError::InvalidTrustRegistry {
                    detail: format!("key '{}' has malformed Ed25519 bytes", key.key_id),
                }
            })?;
            let expected_key_id = key_id_for_public_key(&key.public_key);
            if key.key_id != expected_key_id {
                return Err(ManifestError::InvalidTrustRegistry {
                    detail: format!(
                        "key ID '{}' does not match public-key fingerprint",
                        key.key_id
                    ),
                });
            }

            registered.push(RegisteredKey {
                key_id: key.key_id,
                algorithm: key.algorithm,
                verifying_key,
            });
        }

        Ok(Self { keys: registered })
    }

    pub fn len(&self) -> usize {
        self.keys.len()
    }

    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }

    pub(crate) fn resolve(
        &self,
        key_id: &str,
        algorithm: &str,
    ) -> Result<&VerifyingKey, ManifestError> {
        let key = self
            .keys
            .iter()
            .find(|key| key.key_id == key_id)
            .ok_or_else(|| ManifestError::UnknownKey {
                key_id: key_id.to_owned(),
            })?;

        if key.algorithm != algorithm {
            return Err(ManifestError::UnsupportedSignatureAlgorithm {
                algorithm: algorithm.to_owned(),
            });
        }
        Ok(&key.verifying_key)
    }
}

pub fn key_id_for_public_key(public_key: &[u8; 32]) -> String {
    let digest = Sha256::digest(public_key);
    let mut key_id = String::with_capacity("ed25519-sha256-".len() + digest.len() * 2);
    key_id.push_str("ed25519-sha256-");
    for byte in digest {
        write!(&mut key_id, "{byte:02x}").expect("writing to a String cannot fail");
    }
    key_id
}
