use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, OpenOptions},
    path::{Path, PathBuf},
    sync::Arc,
};

use aes_gcm::{
    Aes256Gcm, Nonce,
    aead::{Aead, AeadCore, KeyInit, OsRng, Payload as AeadPayload},
};
use fs2::FileExt;
use serde::{Deserialize, Serialize};

use crate::{config::ConfigContext, persistence::atomic_write_json};

use super::{
    KeyProvider, NewSecret, ResolvedSecret, SecretKind, SecretMetadata, kinds::StoredSecret,
};

const VAULT_FILE: &str = "secrets.v1.json";
const VAULT_LOCK_FILE: &str = "secrets.v1.lock";
const FORMAT_VERSION: u8 = 1;

#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub struct VaultError {
    code: &'static str,
    message: &'static str,
}

impl VaultError {
    pub fn code(&self) -> &'static str {
        self.code
    }

    pub(crate) fn key_unavailable() -> Self {
        Self::new("VAULT_KEY_UNAVAILABLE", "vault key is unavailable")
    }

    fn locked() -> Self {
        Self::new("VAULT_LOCKED", "vault is locked or contains invalid data")
    }

    fn invalid_secret() -> Self {
        Self::new("VAULT_INVALID_SECRET", "secret fields are invalid")
    }

    fn not_initialized() -> Self {
        Self::new("VAULT_NOT_INITIALIZED", "vault is not initialized")
    }

    fn not_found() -> Self {
        Self::new("VAULT_SECRET_NOT_FOUND", "secret was not found")
    }

    fn duplicate() -> Self {
        Self::new("VAULT_SECRET_EXISTS", "secret id already exists")
    }

    fn io() -> Self {
        Self::new("VAULT_IO", "vault storage operation failed")
    }

    const fn new(code: &'static str, message: &'static str) -> Self {
        Self { code, message }
    }
}

pub struct VaultStore {
    path: PathBuf,
    lock_path: PathBuf,
    key_provider: Arc<dyn KeyProvider>,
}

impl VaultStore {
    pub fn new(context: &ConfigContext, key_provider: Box<dyn KeyProvider>) -> Self {
        Self::at(&context.paths().config_dir, Arc::from(key_provider))
    }

    pub fn at(config_dir: &Path, key_provider: Arc<dyn KeyProvider>) -> Self {
        Self {
            path: config_dir.join(VAULT_FILE),
            lock_path: config_dir.join(VAULT_LOCK_FILE),
            key_provider,
        }
    }

    pub fn initialize(&self) -> Result<(), VaultError> {
        self.mutate(true, |document| Ok((document, ())))
    }

    pub fn list_metadata(&self) -> Result<Vec<SecretMetadata>, VaultError> {
        Ok(self
            .read_document()?
            .secrets
            .values()
            .map(StoredSecret::metadata)
            .collect())
    }

    pub fn put(&self, secret: NewSecret) -> Result<SecretMetadata, VaultError> {
        validate(&secret)?;
        let metadata = SecretMetadata {
            id: secret.id.clone(),
            label: secret.label.clone(),
            description: secret.description.clone(),
            kind: secret.kind,
        };
        self.mutate(false, |mut document| {
            if document.secrets.contains_key(&secret.id) {
                return Err(VaultError::duplicate());
            }
            document
                .secrets
                .insert(secret.id.clone(), StoredSecret::from(secret));
            Ok((document, metadata))
        })
    }

    pub fn replace(&self, secret: NewSecret) -> Result<SecretMetadata, VaultError> {
        validate(&secret)?;
        let metadata = SecretMetadata {
            id: secret.id.clone(),
            label: secret.label.clone(),
            description: secret.description.clone(),
            kind: secret.kind,
        };
        self.mutate(false, |mut document| {
            if !document.secrets.contains_key(&secret.id) {
                return Err(VaultError::not_found());
            }
            document
                .secrets
                .insert(secret.id.clone(), StoredSecret::from(secret));
            Ok((document, metadata))
        })
    }

    pub fn remove(&self, id: &str) -> Result<SecretMetadata, VaultError> {
        self.mutate(false, |mut document| {
            let removed = document
                .secrets
                .remove(id)
                .ok_or_else(VaultError::not_found)?;
            Ok((document, removed.metadata()))
        })
    }

    pub fn resolve(&self, id: &str) -> Result<ResolvedSecret, VaultError> {
        self.read_document()?
            .secrets
            .get(id)
            .map(StoredSecret::resolved)
            .ok_or_else(VaultError::not_found)
    }

    fn mutate<T>(
        &self,
        allow_missing: bool,
        operation: impl FnOnce(PlainDocument) -> Result<(PlainDocument, T), VaultError>,
    ) -> Result<T, VaultError> {
        let parent = self.lock_path.parent().ok_or_else(VaultError::io)?;
        fs::create_dir_all(parent).map_err(|_| VaultError::io())?;
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&self.lock_path)
            .map_err(|_| VaultError::io())?;
        FileExt::lock_exclusive(&lock).map_err(|_| VaultError::io())?;
        let document = match (self.path.exists(), allow_missing) {
            (true, _) => self.read_document()?,
            (false, true) => PlainDocument::default(),
            (false, false) => return Err(VaultError::not_initialized()),
        };
        let (document, output) = operation(document)?;
        self.write_document(&document)?;
        Ok(output)
    }

    fn read_document(&self) -> Result<PlainDocument, VaultError> {
        let payload = fs::read(&self.path).map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                VaultError::not_initialized()
            } else {
                VaultError::io()
            }
        })?;
        let envelope: Envelope =
            serde_json::from_slice(&payload).map_err(|_| VaultError::locked())?;
        if envelope.version != FORMAT_VERSION || envelope.nonce.len() != 12 {
            return Err(VaultError::locked());
        }
        let key = self.key_provider.load_or_create()?;
        let cipher = Aes256Gcm::new_from_slice(&key).map_err(|_| VaultError::locked())?;
        let plaintext = cipher
            .decrypt(
                Nonce::from_slice(&envelope.nonce),
                AeadPayload {
                    msg: &envelope.ciphertext,
                    aad: &[envelope.version],
                },
            )
            .map_err(|_| VaultError::locked())?;
        serde_json::from_slice(&plaintext).map_err(|_| VaultError::locked())
    }

    fn write_document(&self, document: &PlainDocument) -> Result<(), VaultError> {
        let key = self.key_provider.load_or_create()?;
        let cipher = Aes256Gcm::new_from_slice(&key).map_err(|_| VaultError::locked())?;
        let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
        let plaintext = serde_json::to_vec(document).map_err(|_| VaultError::locked())?;
        let ciphertext = cipher
            .encrypt(
                &nonce,
                AeadPayload {
                    msg: &plaintext,
                    aad: &[FORMAT_VERSION],
                },
            )
            .map_err(|_| VaultError::locked())?;
        atomic_write_json(
            &self.path,
            &Envelope {
                version: FORMAT_VERSION,
                nonce: nonce.to_vec(),
                ciphertext,
            },
        )
        .map_err(|_| VaultError::io())
    }
}

fn validate(secret: &NewSecret) -> Result<(), VaultError> {
    if secret.id.trim().is_empty() || secret.label.trim().is_empty() {
        return Err(VaultError::invalid_secret());
    }
    let required: BTreeSet<&str> = match secret.kind {
        SecretKind::Postgres => ["password"].into_iter().collect(),
        SecretKind::HttpBasic => ["username", "password"].into_iter().collect(),
        SecretKind::SshKey => ["private_key"].into_iter().collect(),
        SecretKind::GithubToken | SecretKind::GitlabToken => ["token"].into_iter().collect(),
    };
    let allowed: BTreeSet<&str> = match secret.kind {
        SecretKind::SshKey => ["private_key", "passphrase"].into_iter().collect(),
        _ => required.clone(),
    };
    let present = secret
        .values
        .keys()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    if !required.is_subset(&present)
        || !present.is_subset(&allowed)
        || secret.values.values().any(|value| value.is_empty())
    {
        return Err(VaultError::invalid_secret());
    }
    Ok(())
}

#[derive(Default, Serialize, Deserialize)]
struct PlainDocument {
    secrets: BTreeMap<String, StoredSecret>,
}

#[derive(Serialize, Deserialize)]
struct Envelope {
    version: u8,
    nonce: Vec<u8>,
    ciphertext: Vec<u8>,
}

#[cfg(test)]
mod tests {
    use std::{
        collections::{BTreeMap, BTreeSet},
        fs,
        sync::{Arc, Barrier},
    };

    use serde_json::Value;

    use crate::secrets::{KeyProvider, NewSecret, SecretKind, VaultError, VaultStore};

    #[derive(Clone)]
    struct FixedKey([u8; 32]);

    impl KeyProvider for FixedKey {
        fn load_or_create(&self) -> Result<[u8; 32], VaultError> {
            Ok(self.0)
        }
    }

    #[test]
    fn postgres_secret_round_trip_keeps_metadata_redacted() {
        let directory = tempfile::tempdir().unwrap();
        let store = VaultStore::at(directory.path(), Arc::new(FixedKey([7; 32])));
        store.initialize().unwrap();

        let metadata = store
            .put(NewSecret::postgres("qa-lms", "LMS QA", "sample-password"))
            .unwrap();

        assert_eq!(metadata.id, "qa-lms");
        assert_eq!(metadata.label, "LMS QA");
        assert_eq!(metadata.kind, SecretKind::Postgres);
        assert!(
            !serde_json::to_string(&metadata)
                .unwrap()
                .contains("sample-password")
        );
        assert_eq!(
            store.resolve("qa-lms").unwrap().values.get("password"),
            Some(&"sample-password".to_owned())
        );
    }

    #[test]
    fn ciphertext_tampering_returns_locked_without_secret_value() {
        let directory = tempfile::tempdir().unwrap();
        let store = VaultStore::at(directory.path(), Arc::new(FixedKey([9; 32])));
        store.initialize().unwrap();
        store
            .put(NewSecret::postgres("qa-lms", "LMS QA", "sample-password"))
            .unwrap();
        let path = directory.path().join("secrets.v1.json");
        let mut envelope: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        envelope["ciphertext"][0] = Value::from(envelope["ciphertext"][0].as_u64().unwrap() ^ 1);
        fs::write(&path, serde_json::to_vec_pretty(&envelope).unwrap()).unwrap();

        let error = match store.resolve("qa-lms") {
            Err(error) => error,
            Ok(_) => panic!("tampered ciphertext must not resolve"),
        };

        assert_eq!(error.code(), "VAULT_LOCKED");
        assert!(!error.to_string().contains("sample-password"));
    }

    #[test]
    fn operations_require_explicit_initialization() {
        let directory = tempfile::tempdir().unwrap();
        let store = VaultStore::at(directory.path(), Arc::new(FixedKey([1; 32])));

        assert_eq!(
            store.list_metadata().unwrap_err().code(),
            "VAULT_NOT_INITIALIZED"
        );
        assert_eq!(
            store
                .put(NewSecret::postgres("qa-lms", "LMS QA", "sample-password"))
                .unwrap_err()
                .code(),
            "VAULT_NOT_INITIALIZED"
        );
    }

    #[test]
    fn duplicate_id_is_rejected_without_exposing_either_value() {
        let directory = tempfile::tempdir().unwrap();
        let store = VaultStore::at(directory.path(), Arc::new(FixedKey([2; 32])));
        store.initialize().unwrap();
        store
            .put(NewSecret::postgres("same-id", "First", "first-password"))
            .unwrap();

        let error = store
            .put(NewSecret::postgres("same-id", "Second", "second-password"))
            .unwrap_err();

        assert_eq!(error.code(), "VAULT_SECRET_EXISTS");
        assert!(!error.to_string().contains("first-password"));
        assert!(!error.to_string().contains("second-password"));
    }

    #[test]
    fn invalid_labels_and_kind_fields_are_rejected() {
        let directory = tempfile::tempdir().unwrap();
        let store = VaultStore::at(directory.path(), Arc::new(FixedKey([3; 32])));
        store.initialize().unwrap();
        let invalid = [
            NewSecret::postgres("empty-label", "  ", "sample-password"),
            NewSecret::new("missing", "Missing", SecretKind::Postgres, BTreeMap::new()),
            NewSecret::new(
                "wrong",
                "Wrong",
                SecretKind::Postgres,
                BTreeMap::from([
                    ("password".to_owned(), "sample-password".to_owned()),
                    ("username".to_owned(), "alice".to_owned()),
                ]),
            ),
            NewSecret::new(
                "http-missing",
                "HTTP missing",
                SecretKind::HttpBasic,
                BTreeMap::from([("username".to_owned(), "alice".to_owned())]),
            ),
            NewSecret::new(
                "ssh-extra",
                "SSH extra",
                SecretKind::SshKey,
                BTreeMap::from([
                    ("private_key".to_owned(), "key-material".to_owned()),
                    ("password".to_owned(), "sample-password".to_owned()),
                ]),
            ),
        ];

        for secret in invalid {
            assert_eq!(
                store.put(secret).unwrap_err().code(),
                "VAULT_INVALID_SECRET"
            );
        }
    }

    #[test]
    fn ssh_passphrase_is_optional_and_no_other_extra_field_is_allowed() {
        let directory = tempfile::tempdir().unwrap();
        let store = VaultStore::at(directory.path(), Arc::new(FixedKey([4; 32])));
        store.initialize().unwrap();

        store
            .put(NewSecret::ssh_key(
                "plain-key",
                "Plain key",
                "key-one",
                None,
            ))
            .unwrap();
        store
            .put(NewSecret::ssh_key(
                "protected-key",
                "Protected key",
                "key-two",
                Some("sample-passphrase".to_owned()),
            ))
            .unwrap();

        assert_eq!(store.list_metadata().unwrap().len(), 2);
    }

    #[test]
    fn forge_token_kinds_require_exactly_one_token_field() {
        let directory = tempfile::tempdir().unwrap();
        let store = VaultStore::at(directory.path(), Arc::new(FixedKey([11; 32])));
        store.initialize().unwrap();

        for kind in [SecretKind::GithubToken, SecretKind::GitlabToken] {
            let metadata = store
                .put(NewSecret::api_token(
                    kind.as_str(),
                    "Work PAT",
                    kind,
                    "forge-token-value",
                ))
                .unwrap();
            assert_eq!(metadata.kind, kind);
            assert_eq!(
                store.resolve(kind.as_str()).unwrap().values.get("token"),
                Some(&"forge-token-value".to_owned())
            );

            // No extra field is accepted, and the token itself is mandatory.
            for invalid in [
                NewSecret::new(
                    format!("{kind}-extra"),
                    "Extra",
                    kind,
                    BTreeMap::from([
                        ("token".to_owned(), "forge-token-value".to_owned()),
                        ("username".to_owned(), "alice".to_owned()),
                    ]),
                ),
                NewSecret::new(format!("{kind}-missing"), "Missing", kind, BTreeMap::new()),
            ] {
                assert_eq!(
                    store.put(invalid).unwrap_err().code(),
                    "VAULT_INVALID_SECRET"
                );
            }
        }
    }

    #[test]
    fn remove_returns_redacted_metadata_and_makes_secret_unresolvable() {
        let directory = tempfile::tempdir().unwrap();
        let store = VaultStore::at(directory.path(), Arc::new(FixedKey([5; 32])));
        store.initialize().unwrap();
        store
            .put(NewSecret::postgres(
                "remove-me",
                "Remove me",
                "sample-password",
            ))
            .unwrap();

        let metadata = store.remove("remove-me").unwrap();

        assert_eq!(metadata.id, "remove-me");
        let error = match store.resolve("remove-me") {
            Err(error) => error,
            Ok(_) => panic!("removed secret must not resolve"),
        };
        assert_eq!(error.code(), "VAULT_SECRET_NOT_FOUND");
    }

    #[test]
    fn concurrent_puts_do_not_lose_updates() {
        let directory = tempfile::tempdir().unwrap();
        let store = Arc::new(VaultStore::at(
            directory.path(),
            Arc::new(FixedKey([6; 32])),
        ));
        store.initialize().unwrap();
        let barrier = Arc::new(Barrier::new(8));
        let workers = (0..8)
            .map(|index| {
                let store = Arc::clone(&store);
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    store
                        .put(NewSecret::postgres(
                            format!("secret-{index}"),
                            format!("Secret {index}"),
                            format!("password-{index}"),
                        ))
                        .unwrap();
                })
            })
            .collect::<Vec<_>>();
        for worker in workers {
            worker.join().unwrap();
        }

        assert_eq!(store.list_metadata().unwrap().len(), 8);
    }

    #[test]
    fn persisted_envelope_has_only_version_nonce_and_fresh_ciphertext() {
        let directory = tempfile::tempdir().unwrap();
        let store = VaultStore::at(directory.path(), Arc::new(FixedKey([8; 32])));
        store.initialize().unwrap();
        let path = directory.path().join("secrets.v1.json");
        let before: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();

        store
            .put(NewSecret::postgres("qa-lms", "LMS QA", "sample-password"))
            .unwrap();
        let after: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();

        assert_eq!(
            after
                .as_object()
                .unwrap()
                .keys()
                .cloned()
                .collect::<BTreeSet<_>>(),
            BTreeSet::from([
                "ciphertext".to_owned(),
                "nonce".to_owned(),
                "version".to_owned(),
            ])
        );
        assert_eq!(after["version"], 1);
        assert_ne!(before["nonce"], after["nonce"]);
        let persisted = serde_json::to_string(&after).unwrap();
        assert!(!persisted.contains("sample-password"));
        assert!(!persisted.contains("qa-lms"));
    }

    #[test]
    fn metadata_json_contains_no_value_container() {
        let directory = tempfile::tempdir().unwrap();
        let store = VaultStore::at(directory.path(), Arc::new(FixedKey([10; 32])));
        store.initialize().unwrap();
        let metadata = store
            .put(NewSecret::http_basic(
                "service",
                "Service",
                "alice",
                "sample-password",
            ))
            .unwrap();

        assert_eq!(
            serde_json::to_value(metadata).unwrap(),
            serde_json::json!({
                "id": "service",
                "label": "Service",
                "description": null,
                "kind": "http-basic"
            })
        );
    }
}
