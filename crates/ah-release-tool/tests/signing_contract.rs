use ah_release_tool::SigningMaterial;
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use ed25519_dalek::SigningKey;

#[test]
fn parses_one_strict_seed_and_matching_public_key() {
    let (seed, public_key) = encoded_key_pair([7_u8; 32]);
    let signing = SigningMaterial::parse(&seed, &public_key).unwrap();

    assert_eq!(URL_SAFE_NO_PAD.encode(signing.public_key()), public_key);
    assert!(signing.key_id().starts_with("ed25519-sha256-"));
    assert_eq!(signing.key_id().len(), 79);
    let signature = signing.sign_manifest(b"canonical manifest bytes");
    assert_eq!(signature.len(), 86);
    assert!(signature.is_ascii());
    assert!(!signature.contains(['=', '\n', '\r', ' ']));
}

#[test]
fn rejects_ambiguous_or_malformed_key_encodings_without_echoing_them() {
    let (seed, public_key) = encoded_key_pair([7_u8; 32]);
    let invalid = [
        String::new(),
        "short".to_owned(),
        format!("{seed}="),
        format!(" {seed}"),
        format!("+{}", &seed[1..]),
    ];
    for value in invalid {
        let error = match SigningMaterial::parse(&value, &public_key) {
            Err(error) => error.to_string(),
            Ok(_) => panic!("invalid signing seed was accepted"),
        };
        assert!(!error.contains(&value) || value.is_empty(), "{error}");
    }

    let (_, different_public_key) = encoded_key_pair([8_u8; 32]);
    let error = match SigningMaterial::parse(&seed, &different_public_key) {
        Err(error) => error.to_string(),
        Ok(_) => panic!("mismatched public key was accepted"),
    };
    assert!(error.contains("does not match"));
    assert!(!error.contains(&seed));
    assert!(!error.contains(&different_public_key));
}

fn encoded_key_pair(seed: [u8; 32]) -> (String, String) {
    let signing = SigningKey::from_bytes(&seed);
    (
        URL_SAFE_NO_PAD.encode(seed),
        URL_SAFE_NO_PAD.encode(signing.verifying_key().to_bytes()),
    )
}
