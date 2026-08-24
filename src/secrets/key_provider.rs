use aes_gcm::aead::{OsRng, rand_core::RngCore};
use std::sync::OnceLock;

use super::VaultError;

const MASTER_KEY_ENV: &str = ah_plugin_api::AH_VAULT_MASTER_KEY_ENV;
static STARTUP_MASTER_KEY: OnceLock<StartupMasterKey> = OnceLock::new();

#[derive(Clone, Copy)]
enum StartupMasterKey {
    Absent,
    Available([u8; 32]),
    Invalid,
}

pub trait KeyProvider: Send + Sync {
    fn load_or_create(&self) -> Result<[u8; 32], VaultError>;
}

pub struct ExplicitMasterKey([u8; 32]);

impl ExplicitMasterKey {
    pub fn parse(value: String) -> Result<Self, VaultError> {
        if value.len() != 64 {
            return Err(VaultError::key_unavailable());
        }
        let mut key = [0_u8; 32];
        for (byte, pair) in key.iter_mut().zip(value.as_bytes().chunks_exact(2)) {
            let pair = std::str::from_utf8(pair).map_err(|_| VaultError::key_unavailable())?;
            *byte = u8::from_str_radix(pair, 16).map_err(|_| VaultError::key_unavailable())?;
        }
        Ok(Self(key))
    }
}

impl KeyProvider for ExplicitMasterKey {
    fn load_or_create(&self) -> Result<[u8; 32], VaultError> {
        Ok(self.0)
    }
}

pub struct SystemKeyring {
    service: &'static str,
    user: &'static str,
}

impl SystemKeyring {
    pub fn new(service: &'static str, user: &'static str) -> Self {
        Self { service, user }
    }
}

impl KeyProvider for SystemKeyring {
    fn load_or_create(&self) -> Result<[u8; 32], VaultError> {
        let entry = keyring::Entry::new(self.service, self.user)
            .map_err(|_| VaultError::key_unavailable())?;
        match entry.get_secret() {
            Ok(value) => value.try_into().map_err(|_| VaultError::key_unavailable()),
            Err(keyring::Error::NoEntry) => {
                let mut key = [0_u8; 32];
                OsRng.fill_bytes(&mut key);
                entry
                    .set_secret(&key)
                    .map_err(|_| VaultError::key_unavailable())?;
                Ok(key)
            }
            Err(_) => Err(VaultError::key_unavailable()),
        }
    }
}

pub fn resolve_key_provider() -> Result<Box<dyn KeyProvider>, VaultError> {
    match STARTUP_MASTER_KEY.get().copied() {
        Some(StartupMasterKey::Available(key)) => return Ok(Box::new(ExplicitMasterKey(key))),
        Some(StartupMasterKey::Invalid) => return Err(VaultError::key_unavailable()),
        Some(StartupMasterKey::Absent) | None => {}
    }
    Ok(Box::new(SystemKeyring::new("aihelper", "vault-v1")))
}

/// Captures the optional fallback key before AIHelper starts any worker threads.
pub fn capture_startup_master_key() -> Result<(), VaultError> {
    let value = std::env::var_os(MASTER_KEY_ENV);
    if value.is_some() {
        // SAFETY: the `ah` binary calls this at the first line of single-threaded startup.
        unsafe { std::env::remove_var(MASTER_KEY_ENV) };
    }
    let key = match value {
        Some(value) => match value
            .into_string()
            .ok()
            .and_then(|value| ExplicitMasterKey::parse(value).ok())
        {
            Some(provider) => StartupMasterKey::Available(provider.0),
            None => StartupMasterKey::Invalid,
        },
        None => StartupMasterKey::Absent,
    };
    STARTUP_MASTER_KEY
        .set(key)
        .map_err(|_| VaultError::key_unavailable())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_key_rejects_non_ascii_without_panicking_or_echoing_input() {
        let value = format!("{}a", "€".repeat(21));

        let error = match ExplicitMasterKey::parse(value.clone()) {
            Err(error) => error,
            Ok(_) => panic!("non-hex key must be rejected"),
        };

        assert_eq!(error.code(), "VAULT_KEY_UNAVAILABLE");
        assert!(!error.to_string().contains(&value));
    }
}
