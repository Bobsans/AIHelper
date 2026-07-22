use thiserror::Error;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ManifestError {
    #[error("manifest input is {actual} bytes; maximum is {maximum} bytes")]
    InputTooLarge { actual: usize, maximum: usize },

    #[error("manifest JSON is malformed: {detail}")]
    MalformedJson { detail: String },

    #[error("manifest schema version {found} is unsupported")]
    UnsupportedSchema { found: u32 },

    #[error("manifest JSON is not in canonical encoding")]
    NonCanonicalEncoding,

    #[error("invalid manifest field '{field}': {detail}")]
    InvalidField { field: &'static str, detail: String },

    #[error("signature algorithm '{algorithm}' is unsupported")]
    UnsupportedSignatureAlgorithm { algorithm: String },

    #[error("manifest signing key '{key_id}' is not trusted")]
    UnknownKey { key_id: String },

    #[error("release trust registry is invalid: {detail}")]
    InvalidTrustRegistry { detail: String },

    #[error("detached manifest signature is malformed: {detail}")]
    MalformedSignature { detail: String },

    #[error("detached manifest signature is invalid")]
    InvalidSignature,
}
