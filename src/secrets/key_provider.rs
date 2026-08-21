use aes_gcm::aead::{OsRng, rand_core::RngCore};

use super::VaultError;

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
    if let Ok(value) = std::env::var("AH_VAULT_MASTER_KEY") {
        return Ok(Box::new(ExplicitMasterKey::parse(value)?));
    }
    Ok(Box::new(SystemKeyring::new("aihelper", "vault-v1")))
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
