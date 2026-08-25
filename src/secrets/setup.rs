use std::{
    collections::HashMap,
    fmt::Write as _,
    sync::Mutex,
    time::{Duration, Instant},
};

use aes_gcm::aead::{OsRng, rand_core::RngCore};
use ah_mcp::{
    SecretSetupError as HttpSecretSetupError, SecretSetupField, SecretSetupForm,
    SecretSetupMetadata as HttpSecretSetupMetadata, SecretSetupRequest, SecretSetupService,
};

use super::{NewSecret, SecretKind, SecretMetadata, VaultError, VaultStore};

const SETUP_LIFETIME: Duration = Duration::from_secs(10 * 60);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SecretSetupTarget {
    Create {
        id: String,
        kind: SecretKind,
        label: Option<String>,
        description: Option<String>,
    },
    Edit {
        id: String,
        kind: SecretKind,
        label: Option<String>,
        description: Option<String>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SecretSetupError;

impl SecretSetupError {
    pub const fn code(self) -> &'static str {
        "VAULT_SETUP_CAPABILITY_INVALID"
    }
}

struct Capability {
    target: SecretSetupTarget,
    expires_at: Instant,
}

const MAX_SETUP_CAPABILITIES: usize = 128;

#[derive(Default)]
pub struct SecretSetupCapabilities {
    entries: Mutex<HashMap<String, Capability>>,
}

impl SecretSetupCapabilities {
    pub fn issue(&self, target: SecretSetupTarget, lifetime: Duration) -> String {
        let mut entries = self
            .entries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let now = Instant::now();
        entries.retain(|_, capability| capability.expires_at > now);
        if entries.len() >= MAX_SETUP_CAPABILITIES
            && let Some(oldest) = entries
                .iter()
                .min_by_key(|(_, capability)| capability.expires_at)
                .map(|(token, _)| token.clone())
        {
            entries.remove(&oldest);
        }
        loop {
            let mut bytes = [0_u8; 32];
            OsRng.fill_bytes(&mut bytes);
            let mut token = String::with_capacity(64);
            for byte in bytes {
                write!(token, "{byte:02x}").expect("writing to a string cannot fail");
            }
            if !entries.contains_key(&token) {
                entries.insert(
                    token.clone(),
                    Capability {
                        target,
                        expires_at: now + lifetime,
                    },
                );
                return token;
            }
        }
    }

    pub fn inspect(&self, token: &str) -> Result<SecretSetupTarget, SecretSetupError> {
        self.lookup(token, false)
    }

    pub fn consume(&self, token: &str) -> Result<SecretSetupTarget, SecretSetupError> {
        self.lookup(token, true)
    }

    fn finish(&self, token: &str) -> Result<SecretSetupTarget, SecretSetupError> {
        self.entries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(token)
            .map(|capability| capability.target)
            .ok_or(SecretSetupError)
    }

    fn lookup(&self, token: &str, consume: bool) -> Result<SecretSetupTarget, SecretSetupError> {
        let mut entries = self
            .entries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let Some(capability) = entries.get(token) else {
            return Err(SecretSetupError);
        };
        if capability.expires_at <= Instant::now() {
            entries.remove(token);
            return Err(SecretSetupError);
        }
        if consume {
            return entries
                .remove(token)
                .map(|capability| capability.target)
                .ok_or(SecretSetupError);
        }
        Ok(capability.target.clone())
    }
}

pub struct VaultSetupService {
    vault: std::sync::Arc<VaultStore>,
    capabilities: SecretSetupCapabilities,
    submission: Mutex<()>,
}

impl VaultSetupService {
    pub fn new(vault: std::sync::Arc<VaultStore>) -> Self {
        Self {
            vault,
            capabilities: SecretSetupCapabilities::default(),
            submission: Mutex::new(()),
        }
    }
}

impl SecretSetupService for VaultSetupService {
    fn issue(&self, request: SecretSetupRequest) -> Result<String, HttpSecretSetupError> {
        let target = match request {
            SecretSetupRequest::Create {
                id,
                kind,
                label,
                description,
            } => {
                if id.trim().is_empty()
                    || label.as_ref().is_some_and(|label| label.trim().is_empty())
                {
                    return Err(invalid_request());
                }
                SecretSetupTarget::Create {
                    id,
                    kind: kind.parse().map_err(|()| invalid_request())?,
                    label,
                    description,
                }
            }
            SecretSetupRequest::Edit {
                id,
                label,
                description,
            } => {
                let kind = self
                    .vault
                    .resolve(&id)
                    .map_err(vault_setup_error)?
                    .metadata
                    .kind;
                SecretSetupTarget::Edit {
                    id,
                    kind,
                    label,
                    description,
                }
            }
        };
        Ok(self.capabilities.issue(target, SETUP_LIFETIME))
    }

    fn form(&self, capability: &str) -> Result<SecretSetupForm, HttpSecretSetupError> {
        let target = self
            .capabilities
            .inspect(capability)
            .map_err(capability_error)?;
        let (id, kind) = target_identity(&target);
        Ok(SecretSetupForm {
            id: id.to_owned(),
            kind: kind.to_string(),
            fields: setup_fields(kind),
        })
    }

    fn submit(
        &self,
        capability: &str,
        mut values: std::collections::BTreeMap<String, String>,
    ) -> Result<HttpSecretSetupMetadata, HttpSecretSetupError> {
        let _submission = self
            .submission
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let target = self
            .capabilities
            .inspect(capability)
            .map_err(capability_error)?;
        let (_, kind) = target_identity(&target);
        let allowed = setup_fields(kind)
            .into_iter()
            .map(|field| field.name)
            .collect::<Vec<_>>();
        if values.keys().any(|name| !allowed.contains(&name.as_str())) {
            return Err(invalid_submission());
        }

        let metadata = match &target {
            SecretSetupTarget::Create {
                id,
                kind,
                label,
                description,
            } => {
                values.retain(|_, value| !value.is_empty());
                self.vault
                    .put(
                        NewSecret::new(
                            id.clone(),
                            label.clone().unwrap_or_else(|| id.clone()),
                            *kind,
                            values,
                        )
                        .with_description(description.clone()),
                    )
                    .map_err(vault_setup_error)?
            }
            SecretSetupTarget::Edit {
                id,
                kind,
                label,
                description,
            } => {
                let existing = self.vault.resolve(id).map_err(vault_setup_error)?;
                let mut merged = existing.values;
                for (name, value) in values {
                    if !value.is_empty() {
                        merged.insert(name, value);
                    }
                }
                self.vault
                    .replace(
                        NewSecret::new(
                            id.clone(),
                            label
                                .clone()
                                .unwrap_or_else(|| existing.metadata.label.clone()),
                            *kind,
                            merged,
                        )
                        .with_description(
                            description
                                .clone()
                                .or_else(|| existing.metadata.description.clone()),
                        ),
                    )
                    .map_err(vault_setup_error)?
            }
        };
        self.capabilities
            .finish(capability)
            .map_err(capability_error)?;
        Ok(public_metadata(metadata))
    }
}

fn target_identity(target: &SecretSetupTarget) -> (&str, SecretKind) {
    match target {
        SecretSetupTarget::Create { id, kind, .. } | SecretSetupTarget::Edit { id, kind, .. } => {
            (id, *kind)
        }
    }
}

fn setup_fields(kind: SecretKind) -> Vec<SecretSetupField> {
    match kind {
        SecretKind::Postgres => vec![SecretSetupField {
            name: "password",
            label: "PostgreSQL password",
            optional: false,
        }],
        SecretKind::HttpBasic => vec![
            SecretSetupField {
                name: "username",
                label: "HTTP basic username",
                optional: false,
            },
            SecretSetupField {
                name: "password",
                label: "HTTP basic password",
                optional: false,
            },
        ],
        SecretKind::SshKey => vec![
            SecretSetupField {
                name: "private_key",
                label: "SSH private key",
                optional: false,
            },
            SecretSetupField {
                name: "passphrase",
                label: "SSH key passphrase",
                optional: true,
            },
        ],
        SecretKind::GithubToken => vec![SecretSetupField {
            name: "token",
            label: "GitHub personal access token",
            optional: false,
        }],
        SecretKind::GitlabToken => vec![SecretSetupField {
            name: "token",
            label: "GitLab personal access token",
            optional: false,
        }],
    }
}

fn public_metadata(metadata: SecretMetadata) -> HttpSecretSetupMetadata {
    HttpSecretSetupMetadata {
        id: metadata.id,
        kind: metadata.kind.to_string(),
        label: metadata.label,
        description: metadata.description,
    }
}

fn capability_error(error: SecretSetupError) -> HttpSecretSetupError {
    HttpSecretSetupError::new(
        error.code(),
        "secret setup capability is invalid, expired, or already used",
    )
}

fn invalid_request() -> HttpSecretSetupError {
    HttpSecretSetupError::new(
        "VAULT_SETUP_REQUEST_INVALID",
        "secret setup request is invalid",
    )
}

fn invalid_submission() -> HttpSecretSetupError {
    HttpSecretSetupError::new(
        "VAULT_SETUP_SUBMISSION_INVALID",
        "secret setup form is invalid",
    )
}

fn vault_setup_error(error: VaultError) -> HttpSecretSetupError {
    HttpSecretSetupError::new(error.code(), "vault operation failed")
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeMap, sync::Arc, thread, time::Duration};

    use ah_mcp::{SecretSetupRequest, SecretSetupService};

    use super::{SecretSetupCapabilities, SecretSetupTarget, VaultSetupService};
    use crate::secrets::{KeyProvider, SecretKind, VaultError, VaultStore};

    struct FixedKey;

    impl KeyProvider for FixedKey {
        fn load_or_create(&self) -> Result<[u8; 32], VaultError> {
            Ok([29; 32])
        }
    }

    #[test]
    fn setup_capability_is_single_use_and_expires() {
        let capabilities = SecretSetupCapabilities::default();
        let target = SecretSetupTarget::Create {
            id: "qa-lms".to_owned(),
            kind: SecretKind::Postgres,
            label: None,
            description: None,
        };

        let token = capabilities.issue(target.clone(), Duration::from_secs(600));
        assert_eq!(capabilities.consume(&token).unwrap(), target);
        assert_eq!(
            capabilities.consume(&token).unwrap_err().code(),
            "VAULT_SETUP_CAPABILITY_INVALID"
        );

        let expired = capabilities.issue(target, Duration::ZERO);
        thread::sleep(Duration::from_millis(1));
        assert_eq!(
            capabilities.consume(&expired).unwrap_err().code(),
            "VAULT_SETUP_CAPABILITY_INVALID"
        );
    }

    #[test]
    fn issuing_capabilities_purges_expired_entries_and_bounds_active_entries() {
        let capabilities = SecretSetupCapabilities::default();
        let target = SecretSetupTarget::Create {
            id: "qa-lms".to_owned(),
            kind: SecretKind::Postgres,
            label: None,
            description: None,
        };

        capabilities.issue(target.clone(), Duration::ZERO);
        thread::sleep(Duration::from_millis(1));
        capabilities.issue(target.clone(), Duration::from_secs(600));
        assert_eq!(
            capabilities
                .entries
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .len(),
            1
        );

        for _ in 0..200 {
            capabilities.issue(target.clone(), Duration::from_secs(600));
        }
        assert!(
            capabilities
                .entries
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .len()
                <= 128
        );
    }

    #[test]
    fn failed_submission_keeps_capability_and_success_consumes_it() {
        let directory = tempfile::tempdir().unwrap();
        let vault = Arc::new(VaultStore::at(directory.path(), Arc::new(FixedKey)));
        vault.initialize().unwrap();
        let setup = VaultSetupService::new(Arc::clone(&vault));
        let token = setup
            .issue(SecretSetupRequest::Create {
                id: "qa-lms".to_owned(),
                kind: "postgres".to_owned(),
                label: None,
                description: None,
            })
            .unwrap();

        assert!(setup.submit(&token, BTreeMap::new()).is_err());
        let metadata = setup
            .submit(
                &token,
                BTreeMap::from([("password".to_owned(), "browser-secret".to_owned())]),
            )
            .unwrap();
        assert_eq!(metadata.id, "qa-lms");
        assert_eq!(
            vault.resolve("qa-lms").unwrap().values["password"],
            "browser-secret"
        );
        assert_eq!(
            setup.submit(&token, BTreeMap::new()).unwrap_err().code,
            "VAULT_SETUP_CAPABILITY_INVALID"
        );
    }
}
