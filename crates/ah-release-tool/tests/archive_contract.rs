use std::fs::{self, File};
use std::io::Write;
use std::path::Path;

use ah_release_manifest::FilePurpose;
use ah_release_tool::{RELEASE_PROFILES, ReleaseProfile, validate_archive, validate_archive_set};
use tempfile::TempDir;
use zip::ZipWriter;
use zip::write::SimpleFileOptions;

#[test]
fn validates_exact_supported_archive_set() {
    let temp = TempDir::new().unwrap();
    for profile in RELEASE_PROFILES {
        write_profile_archive(temp.path(), *profile, &[]);
    }

    let inventories = validate_archive_set(temp.path()).unwrap();
    assert_eq!(inventories.len(), RELEASE_PROFILES.len());
    for (inventory, profile) in inventories.iter().zip(RELEASE_PROFILES) {
        assert_eq!(inventory.profile, *profile);
        assert!(inventory.archive_size > 0);
        assert_eq!(inventory.archive_sha256.len(), 64);
        assert_eq!(inventory.files.len(), 5);
        assert!(
            inventory
                .files
                .windows(2)
                .all(|pair| pair[0].path < pair[1].path)
        );
        assert_eq!(
            inventory
                .files
                .iter()
                .filter(|file| file.purpose == FilePurpose::Executable)
                .count(),
            1
        );
        assert_eq!(
            inventory
                .files
                .iter()
                .filter(|file| file.purpose == FilePurpose::Plugin)
                .count(),
            4
        );
    }
}

#[test]
fn rejects_incomplete_or_extra_archive_sets() {
    let temp = TempDir::new().unwrap();
    write_profile_archive(temp.path(), RELEASE_PROFILES[0], &[]);
    let error = validate_archive_set(temp.path()).unwrap_err().to_string();
    assert!(error.contains("expected [ah-linux-x64.zip, ah-macos-arm64.zip, ah-windows-x64.zip]"));

    for profile in &RELEASE_PROFILES[1..] {
        write_profile_archive(temp.path(), *profile, &[]);
    }
    fs::write(temp.path().join("unexpected.txt"), b"unexpected").unwrap();
    let error = validate_archive_set(temp.path()).unwrap_err().to_string();
    assert!(error.contains("unexpected.txt"));
}

#[test]
fn rejects_missing_extra_unsafe_and_case_colliding_entries() {
    let profile = RELEASE_PROFILES[0];

    let missing = TempDir::new().unwrap();
    write_profile_archive(missing.path(), profile, &[(profile.executable, None)]);
    assert!(
        validate_archive(&missing.path().join(profile.asset_name), profile)
            .unwrap_err()
            .to_string()
            .contains("missing required files")
    );

    let extra = TempDir::new().unwrap();
    write_profile_archive(extra.path(), profile, &[("notes.txt", Some(b"extra"))]);
    assert!(
        validate_archive(&extra.path().join(profile.asset_name), profile)
            .unwrap_err()
            .to_string()
            .contains("not part of the release profile")
    );

    let unsafe_path = TempDir::new().unwrap();
    write_profile_archive(unsafe_path.path(), profile, &[("../escape", Some(b"bad"))]);
    assert!(
        validate_archive(&unsafe_path.path().join(profile.asset_name), profile)
            .unwrap_err()
            .to_string()
            .contains("unsafe component")
    );

    let collision = TempDir::new().unwrap();
    write_profile_archive(collision.path(), profile, &[("AH", Some(b"collision"))]);
    assert!(
        validate_archive(&collision.path().join(profile.asset_name), profile)
            .unwrap_err()
            .to_string()
            .contains("case-colliding")
    );
}

#[test]
fn rejects_links_and_malformed_zip_files() {
    let profile = RELEASE_PROFILES[0];
    let linked = TempDir::new().unwrap();
    write_symlink_archive(linked.path(), profile, "linked", "ah");
    assert!(
        validate_archive(&linked.path().join(profile.asset_name), profile)
            .unwrap_err()
            .to_string()
            .contains("symbolic links are not allowed")
    );

    let malformed = TempDir::new().unwrap();
    fs::write(malformed.path().join(profile.asset_name), b"not a zip").unwrap();
    assert!(
        validate_archive(&malformed.path().join(profile.asset_name), profile)
            .unwrap_err()
            .to_string()
            .contains("malformed ZIP")
    );
}

fn write_profile_archive(
    directory: &Path,
    profile: ReleaseProfile,
    overrides: &[(&str, Option<&[u8]>)],
) {
    let file = File::create(directory.join(profile.asset_name)).unwrap();
    let mut writer = ZipWriter::new(file);
    for (path, _) in profile.managed_paths() {
        if overrides
            .iter()
            .any(|(override_path, content)| *override_path == path && content.is_none())
        {
            continue;
        }
        writer
            .start_file(path.as_str(), SimpleFileOptions::default())
            .unwrap();
        let content = overrides
            .iter()
            .find_map(|(override_path, content)| (*override_path == path).then_some(*content))
            .flatten()
            .unwrap_or(path.as_bytes());
        writer.write_all(content).unwrap();
    }
    for (path, content) in overrides {
        if profile
            .managed_paths()
            .iter()
            .any(|(managed_path, _)| managed_path == path)
        {
            continue;
        }
        if let Some(content) = content {
            writer
                .start_file(*path, SimpleFileOptions::default())
                .unwrap();
            writer.write_all(content).unwrap();
        }
    }
    writer.finish().unwrap();
}

fn write_symlink_archive(directory: &Path, profile: ReleaseProfile, path: &str, target: &str) {
    let file = File::create(directory.join(profile.asset_name)).unwrap();
    let mut writer = ZipWriter::new(file);
    writer
        .add_symlink(path, target, SimpleFileOptions::default())
        .unwrap();
    writer.finish().unwrap();
}
