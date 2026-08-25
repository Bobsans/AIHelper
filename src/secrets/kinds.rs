use std::{collections::BTreeMap, fmt, str::FromStr};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SecretKind {
    Postgres,
    HttpBasic,
    SshKey,
    GithubToken,
    GitlabToken,
}

impl SecretKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Postgres => "postgres",
            Self::HttpBasic => "http-basic",
            Self::SshKey => "ssh-key",
            Self::GithubToken => "github-token",
            Self::GitlabToken => "gitlab-token",
        }
    }

    /// Every kind AIHelper accepts, in CLI and schema order.
    pub const ALL: [Self; 5] = [
        Self::Postgres,
        Self::HttpBasic,
        Self::SshKey,
        Self::GithubToken,
        Self::GitlabToken,
    ];
}

impl fmt::Display for SecretKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for SecretKind {
    type Err = ();

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "postgres" => Ok(Self::Postgres),
            "http-basic" => Ok(Self::HttpBasic),
            "ssh-key" => Ok(Self::SshKey),
            "github-token" => Ok(Self::GithubToken),
            "gitlab-token" => Ok(Self::GitlabToken),
            _ => Err(()),
        }
    }
}

pub struct NewSecret {
    pub id: String,
    pub label: String,
    pub description: Option<String>,
    pub kind: SecretKind,
    pub values: BTreeMap<String, String>,
}

impl NewSecret {
    pub fn new(
        id: impl Into<String>,
        label: impl Into<String>,
        kind: SecretKind,
        values: BTreeMap<String, String>,
    ) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            description: None,
            kind,
            values,
        }
    }

    pub fn with_description(mut self, description: Option<String>) -> Self {
        self.description = description;
        self
    }

    pub fn postgres(
        id: impl Into<String>,
        label: impl Into<String>,
        password: impl Into<String>,
    ) -> Self {
        Self::new(
            id,
            label,
            SecretKind::Postgres,
            BTreeMap::from([("password".to_owned(), password.into())]),
        )
    }

    pub fn http_basic(
        id: impl Into<String>,
        label: impl Into<String>,
        username: impl Into<String>,
        password: impl Into<String>,
    ) -> Self {
        Self::new(
            id,
            label,
            SecretKind::HttpBasic,
            BTreeMap::from([
                ("password".to_owned(), password.into()),
                ("username".to_owned(), username.into()),
            ]),
        )
    }

    pub fn api_token(
        id: impl Into<String>,
        label: impl Into<String>,
        kind: SecretKind,
        token: impl Into<String>,
    ) -> Self {
        Self::new(
            id,
            label,
            kind,
            BTreeMap::from([("token".to_owned(), token.into())]),
        )
    }

    pub fn ssh_key(
        id: impl Into<String>,
        label: impl Into<String>,
        private_key: impl Into<String>,
        passphrase: Option<String>,
    ) -> Self {
        let mut values = BTreeMap::from([("private_key".to_owned(), private_key.into())]);
        if let Some(passphrase) = passphrase {
            values.insert("passphrase".to_owned(), passphrase);
        }
        Self::new(id, label, SecretKind::SshKey, values)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SecretMetadata {
    pub id: String,
    pub label: String,
    #[serde(default)]
    pub description: Option<String>,
    pub kind: SecretKind,
}

pub struct ResolvedSecret {
    pub metadata: SecretMetadata,
    pub values: BTreeMap<String, String>,
}

#[derive(Serialize, Deserialize)]
pub(crate) struct StoredSecret {
    pub id: String,
    pub label: String,
    #[serde(default)]
    pub description: Option<String>,
    pub kind: SecretKind,
    pub values: BTreeMap<String, String>,
}

impl From<NewSecret> for StoredSecret {
    fn from(secret: NewSecret) -> Self {
        Self {
            id: secret.id,
            label: secret.label,
            description: secret.description,
            kind: secret.kind,
            values: secret.values,
        }
    }
}

impl StoredSecret {
    pub fn metadata(&self) -> SecretMetadata {
        SecretMetadata {
            id: self.id.clone(),
            label: self.label.clone(),
            description: self.description.clone(),
            kind: self.kind,
        }
    }

    pub fn resolved(&self) -> ResolvedSecret {
        ResolvedSecret {
            metadata: self.metadata(),
            values: self.values.clone(),
        }
    }
}
