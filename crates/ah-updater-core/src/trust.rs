use ah_release_manifest::{TrustedKey, TrustedKeyRegistry, VerifiedManifest, verify_manifest};

use crate::{UpdaterError, UpdaterErrorCode};

#[derive(Debug, Clone)]
pub struct ReleaseTrust {
    registry: TrustedKeyRegistry,
}

impl ReleaseTrust {
    pub fn from_keys(keys: Vec<TrustedKey>) -> Result<Self, UpdaterError> {
        if keys.is_empty() {
            return Err(UpdaterError::new(
                UpdaterErrorCode::Trust,
                "release trust registry must contain at least one public key",
            ));
        }
        let registry = TrustedKeyRegistry::new(keys).map_err(UpdaterError::from_manifest)?;
        Ok(Self { registry })
    }

    pub fn key_count(&self) -> usize {
        self.registry.len()
    }

    pub fn verify(
        &self,
        manifest_bytes: &[u8],
        signature_bytes: &[u8],
    ) -> Result<VerifiedManifest, UpdaterError> {
        verify_manifest(manifest_bytes, signature_bytes, &self.registry)
            .map_err(UpdaterError::from_manifest)
    }
}

#[cfg(test)]
mod tests {
    use ah_release_manifest::{
        ArchiveMetadata, FilePurpose, ManagedFile, ReleaseManifest, ReleaseMetadata, RequiredFiles,
        SCHEMA_VERSION, SIGNING_ALGORITHM, SIGNING_DOMAIN, SignatureAlgorithm, SigningMetadata,
        TrustedKey, key_id_for_public_key,
    };
    use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
    use ed25519_dalek::{Signer as _, SigningKey};

    use super::*;

    #[test]
    fn accepts_injected_public_key_registry() {
        let key = SigningKey::from_bytes(&[7_u8; 32]);
        let trust = ReleaseTrust::from_keys(vec![trusted_key(&key)]).unwrap();
        assert_eq!(trust.key_count(), 1);
    }

    #[test]
    fn rejects_empty_trust_registry() {
        let error = ReleaseTrust::from_keys(Vec::new()).unwrap_err();
        assert_eq!(error.code(), UpdaterErrorCode::Trust);
    }

    #[test]
    fn maps_unknown_signing_key_to_sanitized_trust_error() {
        let trusted = SigningKey::from_bytes(&[7_u8; 32]);
        let untrusted = SigningKey::from_bytes(&[9_u8; 32]);
        let untrusted_id = key_id_for_public_key(&untrusted.verifying_key().to_bytes());
        let manifest = ReleaseManifest {
            schema_version: SCHEMA_VERSION,
            release: ReleaseMetadata {
                version: "1.2.0".to_owned(),
                target: "x86_64-pc-windows-msvc".to_owned(),
                architecture: "x86_64".to_owned(),
            },
            archive: ArchiveMetadata {
                url: "https://github.com/example/aihelper/releases/download/v1.2.0/ah-windows-x64.zip"
                    .to_owned(),
                size: 2,
                sha256: "0".repeat(64),
            },
            minimum_updater_version: "1.1.0".to_owned(),
            signing: SigningMetadata {
                key_id: untrusted_id.clone(),
                algorithm: SignatureAlgorithm::Ed25519,
            },
            files: vec![ManagedFile {
                path: "ah.exe".to_owned(),
                size: 1,
                sha256: "1".repeat(64),
                purpose: FilePurpose::Executable,
            }],
            required: RequiredFiles {
                executables: vec!["ah.exe".to_owned()],
                plugins: Vec::new(),
            },
        }
        .to_canonical_bytes()
        .unwrap();
        let mut preimage = Vec::from(SIGNING_DOMAIN);
        preimage.extend_from_slice(&manifest);
        let signature = URL_SAFE_NO_PAD.encode(untrusted.sign(&preimage).to_bytes());

        let error = ReleaseTrust::from_keys(vec![trusted_key(&trusted)])
            .unwrap()
            .verify(&manifest, signature.as_bytes())
            .unwrap_err();

        assert_eq!(error.code(), UpdaterErrorCode::Trust);
        assert_eq!(
            error.detail(),
            "release manifest uses an untrusted signing key"
        );
        assert!(!error.to_string().contains(&untrusted_id));
    }

    fn trusted_key(signing_key: &SigningKey) -> TrustedKey {
        let public_key = signing_key.verifying_key().to_bytes();
        TrustedKey {
            key_id: key_id_for_public_key(&public_key),
            algorithm: SIGNING_ALGORITHM.to_owned(),
            public_key,
        }
    }
}
