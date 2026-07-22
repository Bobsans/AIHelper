use ah_release_manifest::{
    ArchiveMetadata, FilePurpose, ManagedFile, ManifestError, ReleaseManifest, ReleaseMetadata,
    RequiredFiles, SCHEMA_VERSION, SignatureAlgorithm, SigningMetadata,
};

const DIGEST_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const DIGEST_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const KEY_ID: &str =
    "ed25519-sha256-cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";

fn valid_manifest() -> ReleaseManifest {
    ReleaseManifest {
        schema_version: SCHEMA_VERSION,
        release: ReleaseMetadata {
            version: "1.1.0".to_owned(),
            target: "x86_64-pc-windows-msvc".to_owned(),
            architecture: "x86_64".to_owned(),
        },
        archive: ArchiveMetadata {
            url: "https://github.com/darkboy/aihelper/releases/download/v1.1.0/ah-windows-x64.zip"
                .to_owned(),
            size: 1024,
            sha256: DIGEST_A.to_owned(),
        },
        minimum_updater_version: "1.1.0".to_owned(),
        signing: SigningMetadata {
            key_id: KEY_ID.to_owned(),
            algorithm: SignatureAlgorithm::Ed25519,
        },
        files: vec![
            ManagedFile {
                path: "ah.exe".to_owned(),
                size: 512,
                sha256: DIGEST_A.to_owned(),
                purpose: FilePurpose::Executable,
            },
            ManagedFile {
                path: "plugins/ah-plugin-github.dll".to_owned(),
                size: 0,
                sha256: DIGEST_B.to_owned(),
                purpose: FilePurpose::Plugin,
            },
        ],
        required: RequiredFiles {
            executables: vec!["ah.exe".to_owned()],
            plugins: vec!["plugins/ah-plugin-github.dll".to_owned()],
        },
    }
}

fn assert_invalid_field(manifest: ReleaseManifest, expected_field: &'static str) {
    match manifest.validate() {
        Err(ManifestError::InvalidField { field, .. }) => assert_eq!(field, expected_field),
        other => panic!("expected invalid field '{expected_field}', got {other:?}"),
    }
}

#[test]
fn accepts_valid_manifest_and_zero_byte_managed_file() {
    valid_manifest().validate().unwrap();
}

#[test]
fn rejects_unsupported_schema() {
    let mut manifest = valid_manifest();
    manifest.schema_version = 2;
    assert_eq!(
        manifest.validate(),
        Err(ManifestError::UnsupportedSchema { found: 2 })
    );
}

#[test]
fn rejects_invalid_versions_without_normalizing() {
    for invalid_version in ["", "1.1", "01.1.0", " 1.1.0", "1.1.0 "] {
        let mut manifest = valid_manifest();
        manifest.release.version = invalid_version.to_owned();
        assert_invalid_field(manifest, "release.version");
    }

    let mut manifest = valid_manifest();
    manifest.minimum_updater_version = "1.01.0".to_owned();
    assert_invalid_field(manifest, "minimum_updater_version");
}

#[test]
fn validates_each_supported_target_pair() {
    for (target, architecture) in [
        ("x86_64-pc-windows-msvc", "x86_64"),
        ("x86_64-unknown-linux-gnu", "x86_64"),
        ("aarch64-apple-darwin", "aarch64"),
    ] {
        let mut manifest = valid_manifest();
        manifest.release.target = target.to_owned();
        manifest.release.architecture = architecture.to_owned();
        manifest.validate().unwrap();
    }

    let mut manifest = valid_manifest();
    manifest.release.architecture = "aarch64".to_owned();
    assert_invalid_field(manifest, "release.target");
}

#[test]
fn rejects_invalid_archive_urls() {
    for invalid_url in [
        "",
        "archive.zip",
        "http://example.com/archive.zip",
        "https://?archive.zip",
        "https://user@example.com/archive.zip",
        "https://user:secret@example.com/archive.zip",
        "https://example.com/archive.zip#digest",
    ] {
        let mut manifest = valid_manifest();
        manifest.archive.url = invalid_url.to_owned();
        assert_invalid_field(manifest, "archive.url");
    }
}

#[test]
fn rejects_zero_archive_size() {
    let mut manifest = valid_manifest();
    manifest.archive.size = 0;
    assert_invalid_field(manifest, "archive.size");
}

#[test]
fn rejects_invalid_digests() {
    for invalid_digest in [
        "",
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaA",
        "gggggggggggggggggggggggggggggggggggggggggggggggggggggggggggggggg",
    ] {
        let mut archive_manifest = valid_manifest();
        archive_manifest.archive.sha256 = invalid_digest.to_owned();
        assert_invalid_field(archive_manifest, "archive.sha256");

        let mut file_manifest = valid_manifest();
        file_manifest.files[0].sha256 = invalid_digest.to_owned();
        assert_invalid_field(file_manifest, "files[].sha256");
    }
}

#[test]
fn rejects_invalid_key_ids() {
    for key_id in [
        "",
        "sha256-cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
        "ed25519-sha256-ccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
        "ed25519-sha256-CCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCC",
    ] {
        let mut manifest = valid_manifest();
        manifest.signing.key_id = key_id.to_owned();
        assert_invalid_field(manifest, "signing.key_id");
    }
}

#[test]
fn rejects_empty_and_oversized_inventories() {
    let mut empty = valid_manifest();
    empty.files.clear();
    assert_invalid_field(empty, "files");

    let mut oversized = valid_manifest();
    let prototype = oversized.files[0].clone();
    oversized.files = (0..=4096)
        .map(|index| ManagedFile {
            path: format!("files/{index:04}.bin"),
            ..prototype.clone()
        })
        .collect();
    assert_invalid_field(oversized, "files");
}

#[test]
fn rejects_invalid_managed_paths_without_normalizing() {
    let long_path = "a".repeat(513);
    for invalid_path in [
        "".to_owned(),
        long_path,
        "/ah.exe".to_owned(),
        "bin/".to_owned(),
        "bin//ah.exe".to_owned(),
        "bin/./ah.exe".to_owned(),
        "bin/../ah.exe".to_owned(),
        "bin\\ah.exe".to_owned(),
        "C:/ah.exe".to_owned(),
        "áh.exe".to_owned(),
        "ah!.exe".to_owned(),
    ] {
        let mut manifest = valid_manifest();
        manifest.files[0].path = invalid_path;
        assert_invalid_field(manifest, "files[].path");
    }
}

#[test]
fn rejects_unsorted_duplicate_and_case_colliding_inventory_paths() {
    let mut unsorted = valid_manifest();
    unsorted.files.swap(0, 1);
    assert_invalid_field(unsorted, "files[].path");

    let mut duplicate = valid_manifest();
    duplicate.files[1].path = "ah.exe".to_owned();
    assert_invalid_field(duplicate, "files[].path");

    let mut case_collision = valid_manifest();
    case_collision.files[0].path = "AH.EXE".to_owned();
    case_collision.files[1].path = "ah.exe".to_owned();
    match case_collision.validate() {
        Err(ManifestError::InvalidField { field, detail }) => {
            assert_eq!(field, "files[].path");
            assert!(detail.contains("ASCII case folding"));
        }
        other => panic!("expected an ASCII case-fold collision, got {other:?}"),
    }
}

#[test]
fn rejects_invalid_required_lists_and_relationships() {
    let mut no_executable = valid_manifest();
    no_executable.required.executables.clear();
    assert_invalid_field(no_executable, "required.executables");

    let mut unsorted = valid_manifest();
    unsorted.required.executables = vec!["tools/z.exe".to_owned(), "ah.exe".to_owned()];
    assert_invalid_field(unsorted, "required.executables");

    let mut duplicate = valid_manifest();
    duplicate
        .required
        .plugins
        .push(duplicate.required.plugins[0].clone());
    assert_invalid_field(duplicate, "required.plugins");

    let mut case_collision = valid_manifest();
    case_collision.required.plugins = vec![
        "PLUGINS/ah-plugin-github.dll".to_owned(),
        "plugins/ah-plugin-github.dll".to_owned(),
    ];
    assert_invalid_field(case_collision, "required.plugins");

    let mut missing_executable = valid_manifest();
    missing_executable.required.executables = vec!["missing.exe".to_owned()];
    assert_invalid_field(missing_executable, "required.executables");

    let mut missing_plugin = valid_manifest();
    missing_plugin.required.plugins = vec!["plugins/missing.dll".to_owned()];
    assert_invalid_field(missing_plugin, "required.plugins");

    let mut executable_with_wrong_purpose = valid_manifest();
    executable_with_wrong_purpose.files[0].purpose = FilePurpose::Support;
    assert_invalid_field(executable_with_wrong_purpose, "required.executables");

    let mut plugin_with_wrong_purpose = valid_manifest();
    plugin_with_wrong_purpose.files[1].purpose = FilePurpose::Support;
    assert_invalid_field(plugin_with_wrong_purpose, "required.plugins");
}

#[test]
fn accepts_update_helper_as_required_executable() {
    let mut manifest = valid_manifest();
    manifest.files[0].purpose = FilePurpose::UpdateHelper;
    manifest.validate().unwrap();
}

#[test]
fn rejects_unknown_duplicate_missing_null_and_wrong_type_json_fields() {
    let valid_json = serde_json::to_string(&valid_manifest()).unwrap();
    let cases = [
        valid_json.replacen(
            "\"schema_version\":1",
            "\"schema_version\":1,\"unknown\":true",
            1,
        ),
        valid_json.replacen(
            "\"schema_version\":1",
            "\"schema_version\":1,\"schema_version\":1",
            1,
        ),
        valid_json.replacen("\"schema_version\":1,", "", 1),
        valid_json.replacen("\"schema_version\":1", "\"schema_version\":null", 1),
        valid_json.replacen("\"schema_version\":1", "\"schema_version\":\"1\"", 1),
        valid_json.replacen("\"algorithm\":\"ed25519\"", "\"algorithm\":\"rsa\"", 1),
    ];

    for raw in cases {
        assert!(
            serde_json::from_str::<ReleaseManifest>(&raw).is_err(),
            "{raw}"
        );
    }
}
