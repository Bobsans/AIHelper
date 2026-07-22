use ah_release_manifest::{ManifestError, ReleaseManifest};

const FIXTURES: [(&str, &[u8], &str, &str, &str); 3] = [
    (
        "windows-x64",
        include_bytes!("fixtures/windows-x64.manifest.json"),
        "x86_64-pc-windows-msvc",
        "x86_64",
        ".dll",
    ),
    (
        "linux-x64",
        include_bytes!("fixtures/linux-x64.manifest.json"),
        "x86_64-unknown-linux-gnu",
        "x86_64",
        ".so",
    ),
    (
        "macos-arm64",
        include_bytes!("fixtures/macos-arm64.manifest.json"),
        "aarch64-apple-darwin",
        "aarch64",
        ".dylib",
    ),
];

#[test]
fn fixtures_are_valid_byte_exact_canonical_manifests() {
    for (name, bytes, target, architecture, plugin_suffix) in FIXTURES {
        assert!(!bytes.starts_with(&[0xef, 0xbb, 0xbf]), "{name}");
        assert!(!bytes.ends_with(b"\n"), "{name}");
        assert!(!bytes.ends_with(b"\r"), "{name}");

        let raw = std::str::from_utf8(bytes).unwrap();
        assert!(
            !raw.bytes().any(|byte| byte.is_ascii_whitespace()),
            "{name}"
        );
        assert!(
            raw.starts_with("{\"schema_version\":1,\"release\":"),
            "{name}"
        );
        assert!(raw.ends_with('}'), "{name}");

        let manifest = serde_json::from_slice::<ReleaseManifest>(bytes).unwrap();
        assert_eq!(manifest.release.target, target, "{name}");
        assert_eq!(manifest.release.architecture, architecture, "{name}");
        assert_eq!(manifest.files.len(), 5, "{name}");
        assert_eq!(manifest.required.plugins.len(), 4, "{name}");
        assert!(
            manifest.files[1..]
                .iter()
                .all(|file| file.path.ends_with(plugin_suffix)),
            "{name}"
        );
        assert_eq!(manifest.to_canonical_bytes().unwrap(), bytes, "{name}");
    }
}

#[test]
fn canonical_encoder_rejects_semantically_invalid_values() {
    let mut manifest = serde_json::from_slice::<ReleaseManifest>(FIXTURES[0].1).unwrap();
    manifest.files.swap(0, 1);
    assert!(matches!(
        manifest.to_canonical_bytes(),
        Err(ManifestError::InvalidField {
            field: "files[].path",
            ..
        })
    ));
}
