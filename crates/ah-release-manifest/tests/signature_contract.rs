mod support;

use ah_release_manifest::{
    DETACHED_SIGNATURE_BYTES, MAX_MANIFEST_BYTES, ManifestError, ReleaseManifest,
    SIGNING_ALGORITHM, TrustedKey, TrustedKeyRegistry, key_id_for_public_key, verify_manifest,
};

const FIXTURES: [(&str, &[u8], &[u8]); 3] = [
    (
        "windows-x64",
        include_bytes!("fixtures/windows-x64.manifest.json"),
        include_bytes!("fixtures/windows-x64.manifest.sig"),
    ),
    (
        "linux-x64",
        include_bytes!("fixtures/linux-x64.manifest.json"),
        include_bytes!("fixtures/linux-x64.manifest.sig"),
    ),
    (
        "macos-arm64",
        include_bytes!("fixtures/macos-arm64.manifest.json"),
        include_bytes!("fixtures/macos-arm64.manifest.sig"),
    ),
];

fn replace_once(input: &[u8], from: &str, to: &str) -> Vec<u8> {
    let raw = std::str::from_utf8(input).unwrap();
    assert_eq!(raw.matches(from).count(), 1, "replacement must be unique");
    raw.replacen(from, to, 1).into_bytes()
}

#[test]
fn verifies_all_platform_signature_fixtures() {
    let registry = support::registry();
    for (name, manifest, signature) in FIXTURES {
        assert_eq!(signature.len(), DETACHED_SIGNATURE_BYTES, "{name}");
        assert!(signature.iter().all(|byte| byte.is_ascii()), "{name}");
        assert_eq!(support::sign_with_primary(manifest), signature, "{name}");

        let verified = verify_manifest(manifest, signature, &registry).unwrap();
        assert_eq!(verified.manifest().schema_version, 1, "{name}");
        assert_eq!(verified.into_manifest().files.len(), 5, "{name}");
    }
}

#[test]
fn rejects_changed_signed_bytes_fields_and_array_order() {
    let (manifest, signature) = (FIXTURES[0].1, FIXTURES[0].2);
    let registry = support::registry();

    let changed_byte = replace_once(manifest, "\"size\":9000000", "\"size\":9000001");
    assert_eq!(
        verify_manifest(&changed_byte, signature, &registry),
        Err(ManifestError::InvalidSignature)
    );

    let mut changed_field = serde_json::from_slice::<ReleaseManifest>(manifest).unwrap();
    changed_field.release.version = "1.2.1".to_owned();
    let changed_field = serde_json::to_vec(&changed_field).unwrap();
    assert_eq!(
        verify_manifest(&changed_field, signature, &registry),
        Err(ManifestError::InvalidSignature)
    );

    let mut changed_order = serde_json::from_slice::<ReleaseManifest>(manifest).unwrap();
    changed_order.files.swap(0, 1);
    let changed_order = serde_json::to_vec(&changed_order).unwrap();
    assert_eq!(
        verify_manifest(&changed_order, signature, &registry),
        Err(ManifestError::InvalidSignature)
    );
}

#[test]
fn rejects_signatures_from_another_key() {
    let (manifest, _) = (FIXTURES[0].1, FIXTURES[0].2);
    let other_signature = support::sign_with_secondary(manifest);
    assert_eq!(
        verify_manifest(manifest, &other_signature, &support::registry()),
        Err(ManifestError::InvalidSignature)
    );
}

#[test]
fn rejects_unknown_key_and_unsupported_manifest_selectors() {
    let (manifest, signature) = (FIXTURES[0].1, FIXTURES[0].2);
    let key_id = support::primary_trusted_key().key_id;
    let unknown_id = format!("ed25519-sha256-{}", "d".repeat(64));
    let unknown_key = replace_once(manifest, &key_id, &unknown_id);
    assert_eq!(
        verify_manifest(&unknown_key, signature, &support::registry()),
        Err(ManifestError::UnknownKey { key_id: unknown_id })
    );

    let unsupported_algorithm = replace_once(manifest, "\"ed25519\"", "\"rsa\"");
    assert_eq!(
        verify_manifest(&unsupported_algorithm, signature, &support::registry()),
        Err(ManifestError::UnsupportedSignatureAlgorithm {
            algorithm: "rsa".to_owned(),
        })
    );

    let unsupported_schema = replace_once(manifest, "\"schema_version\":1", "\"schema_version\":2");
    assert_eq!(
        verify_manifest(&unsupported_schema, signature, &support::registry()),
        Err(ManifestError::UnsupportedSchema { found: 2 })
    );
}

#[test]
fn rejects_invalid_trust_registries() {
    let malformed_public_key = [0x02; 32];
    let malformed = TrustedKey {
        key_id: key_id_for_public_key(&malformed_public_key),
        algorithm: SIGNING_ALGORITHM.to_owned(),
        public_key: malformed_public_key,
    };
    assert!(matches!(
        TrustedKeyRegistry::new(vec![malformed]),
        Err(ManifestError::InvalidTrustRegistry { .. })
    ));

    let mut fingerprint_mismatch = support::primary_trusted_key();
    fingerprint_mismatch.key_id = format!("ed25519-sha256-{}", "0".repeat(64));
    assert!(matches!(
        TrustedKeyRegistry::new(vec![fingerprint_mismatch]),
        Err(ManifestError::InvalidTrustRegistry { .. })
    ));

    let key = support::primary_trusted_key();
    assert!(matches!(
        TrustedKeyRegistry::new(vec![key.clone(), key]),
        Err(ManifestError::InvalidTrustRegistry { .. })
    ));

    let mut unsupported_algorithm = support::primary_trusted_key();
    unsupported_algorithm.algorithm = "rsa".to_owned();
    assert!(matches!(
        TrustedKeyRegistry::new(vec![unsupported_algorithm]),
        Err(ManifestError::InvalidTrustRegistry { .. })
    ));
}

#[test]
fn rejects_malformed_detached_signatures() {
    let (manifest, signature) = (FIXTURES[0].1, FIXTURES[0].2);
    let registry = support::registry();

    let mut padded = signature.to_vec();
    padded.extend_from_slice(b"==");
    let mut whitespace = signature.to_vec();
    *whitespace.last_mut().unwrap() = b' ';
    let mut invalid_alphabet = signature.to_vec();
    *invalid_alphabet.last_mut().unwrap() = b'+';
    let truncated = signature[..signature.len() - 1].to_vec();
    let mut oversized = signature.to_vec();
    oversized.push(b'A');

    for malformed in [padded, whitespace, invalid_alphabet, truncated, oversized] {
        assert!(matches!(
            verify_manifest(manifest, &malformed, &registry),
            Err(ManifestError::MalformedSignature { .. })
        ));
    }
}

#[test]
fn rejects_oversized_manifest_before_selector_parsing() {
    let oversized = vec![b' '; MAX_MANIFEST_BYTES + 1];
    assert_eq!(
        verify_manifest(&oversized, FIXTURES[0].2, &support::registry()),
        Err(ManifestError::InputTooLarge {
            actual: MAX_MANIFEST_BYTES + 1,
            maximum: MAX_MANIFEST_BYTES,
        })
    );
}

#[test]
fn rejects_non_canonical_manifest_before_signature_verification() {
    let mut non_canonical = vec![b' '];
    non_canonical.extend_from_slice(FIXTURES[0].1);
    assert_eq!(
        verify_manifest(&non_canonical, FIXTURES[0].2, &support::registry()),
        Err(ManifestError::NonCanonicalEncoding)
    );
}

#[test]
fn verifies_signature_before_semantic_validation() {
    let mut invalid = serde_json::from_slice::<ReleaseManifest>(FIXTURES[0].1).unwrap();
    invalid.archive.size = 0;
    let invalid = serde_json::to_vec(&invalid).unwrap();
    let invalid_signature = vec![b'A'; DETACHED_SIGNATURE_BYTES];

    assert_eq!(
        verify_manifest(&invalid, &invalid_signature, &support::registry()),
        Err(ManifestError::InvalidSignature)
    );

    let valid_signature = support::sign_with_primary(&invalid);
    assert!(matches!(
        verify_manifest(&invalid, &valid_signature, &support::registry()),
        Err(ManifestError::InvalidField {
            field: "archive.size",
            ..
        })
    ));
}

#[test]
fn supports_two_key_rotation_and_exact_selection() {
    let mut manifest = serde_json::from_slice::<ReleaseManifest>(FIXTURES[0].1).unwrap();
    manifest.signing.key_id = support::secondary_trusted_key().key_id;
    let manifest = serde_json::to_vec(&manifest).unwrap();
    let signature = support::sign_with_secondary(&manifest);
    let registry = support::two_key_registry();

    assert_eq!(registry.len(), 2);
    assert!(!registry.is_empty());
    verify_manifest(&manifest, &signature, &registry).unwrap();
    assert!(matches!(
        verify_manifest(&manifest, &signature, &support::registry()),
        Err(ManifestError::UnknownKey { .. })
    ));
}
