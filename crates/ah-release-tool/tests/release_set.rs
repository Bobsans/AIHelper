use std::collections::BTreeSet;
use std::fs::{self, File};
use std::io::Write;
use std::path::Path;

use ah_release_manifest::verify_manifest;
use ah_release_tool::{
    RELEASE_PROFILES, ReleaseProfile, ReleaseRequest, SigningMaterial, sign_release_set,
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use ed25519_dalek::SigningKey;
use tempfile::TempDir;
use zip::ZipWriter;
use zip::write::SimpleFileOptions;

#[test]
fn creates_and_verifies_exact_nine_file_release_set() {
    let input = TempDir::new().unwrap();
    for profile in RELEASE_PROFILES {
        write_profile_archive(input.path(), *profile);
    }
    let output_parent = TempDir::new().unwrap();
    let output = output_parent.path().join("signed");
    let signing = signing_material([11_u8; 32]);

    let paths = sign_release_set(
        &ReleaseRequest {
            assets_dir: input.path().to_path_buf(),
            output_dir: output.clone(),
            repository: "example/aihelper".to_owned(),
            tag: "v1.1.0".to_owned(),
        },
        &signing,
    )
    .unwrap();

    assert_eq!(paths.len(), 9);
    assert!(paths.iter().all(|path| path.is_file()));
    let observed = fs::read_dir(&output)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect::<BTreeSet<_>>();
    assert_eq!(observed.len(), 9);

    let registry = signing.trusted_registry().unwrap();
    for profile in RELEASE_PROFILES {
        let base = profile.asset_name.trim_end_matches(".zip");
        let manifest = fs::read(output.join(format!("{base}.manifest.json"))).unwrap();
        let signature = fs::read(output.join(format!("{base}.manifest.sig"))).unwrap();
        assert_eq!(signature.len(), 86);
        assert!(!signature.ends_with(b"\n"));
        let verified = verify_manifest(&manifest, &signature, &registry).unwrap();
        assert_eq!(verified.manifest().release.target, profile.target);
        assert_eq!(verified.manifest().release.version, "1.1.0");
        assert_eq!(verified.manifest().minimum_updater_version, "1.1.0");
        assert_eq!(
            verified.manifest().archive.url,
            format!(
                "https://github.com/example/aihelper/releases/download/v1.1.0/{}",
                profile.asset_name
            )
        );
    }
}

#[test]
fn leaves_no_output_on_metadata_or_archive_failure() {
    let input = TempDir::new().unwrap();
    for profile in RELEASE_PROFILES {
        write_profile_archive(input.path(), *profile);
    }
    let output_parent = TempDir::new().unwrap();
    let output = output_parent.path().join("signed");
    let signing = signing_material([11_u8; 32]);

    let error = sign_release_set(
        &ReleaseRequest {
            assets_dir: input.path().to_path_buf(),
            output_dir: output.clone(),
            repository: "unsafe/repository/extra".to_owned(),
            tag: "v1.1.0".to_owned(),
        },
        &signing,
    )
    .unwrap_err()
    .to_string();
    assert!(error.contains("owner/name"));
    assert!(!output.exists());

    fs::write(input.path().join("unexpected"), b"extra").unwrap();
    let error = sign_release_set(
        &ReleaseRequest {
            assets_dir: input.path().to_path_buf(),
            output_dir: output.clone(),
            repository: "example/aihelper".to_owned(),
            tag: "v1.1.0".to_owned(),
        },
        &signing,
    )
    .unwrap_err()
    .to_string();
    assert!(error.contains("archive set is invalid"));
    assert!(!output.exists());
}

#[test]
fn rejects_noncanonical_or_mismatched_release_versions() {
    let input = TempDir::new().unwrap();
    for profile in RELEASE_PROFILES {
        write_profile_archive(input.path(), *profile);
    }
    let output_parent = TempDir::new().unwrap();
    let signing = signing_material([11_u8; 32]);

    for tag in ["1.1.0", "v1.01.0", "v1.2.0"] {
        let output = output_parent.path().join(tag.replace('.', "-"));
        let error = sign_release_set(
            &ReleaseRequest {
                assets_dir: input.path().to_path_buf(),
                output_dir: output.clone(),
                repository: "example/aihelper".to_owned(),
                tag: tag.to_owned(),
            },
            &signing,
        )
        .unwrap_err();
        assert!(!output.exists());
        assert!(error.to_string().contains("release tag"));
    }
}

fn signing_material(seed: [u8; 32]) -> SigningMaterial {
    let signing = SigningKey::from_bytes(&seed);
    SigningMaterial::parse(
        &URL_SAFE_NO_PAD.encode(seed),
        &URL_SAFE_NO_PAD.encode(signing.verifying_key().to_bytes()),
    )
    .unwrap()
}

fn write_profile_archive(directory: &Path, profile: ReleaseProfile) {
    let file = File::create(directory.join(profile.asset_name)).unwrap();
    let mut writer = ZipWriter::new(file);
    writer
        .add_directory("plugins/", SimpleFileOptions::default())
        .unwrap();
    for (path, _) in profile.managed_paths() {
        writer
            .start_file(path.as_str(), SimpleFileOptions::default())
            .unwrap();
        writer.write_all(path.as_bytes()).unwrap();
    }
    writer.finish().unwrap();
}
