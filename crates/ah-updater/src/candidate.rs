use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use ah_updater_core::{
    ManagedFile, ReleaseAssetV1, UpdaterError, UpdaterErrorCode, VerifiedReleaseV1,
};
use sha2::{Digest, Sha256};
use tempfile::TempDir;
use zip::{CompressionMethod, ZipArchive};

const MAX_ARCHIVE_BYTES: u64 = 512 * 1024 * 1024;
const MAX_MANAGED_FILE_BYTES: u64 = 256 * 1024 * 1024;
const MAX_TOTAL_UNCOMPRESSED_BYTES: u64 = 1024 * 1024 * 1024;

pub trait CandidateArchiveSource {
    fn download_archive(
        &self,
        asset: &ReleaseAssetV1,
        output: &mut dyn Write,
    ) -> Result<(), UpdaterError>;
}

#[derive(Debug)]
pub struct PreparedCandidate {
    _staging: TempDir,
    root: PathBuf,
    verified_release: VerifiedReleaseV1,
}

impl PreparedCandidate {
    pub fn root(&self) -> &Path {
        &self.root
    }

    pub(crate) fn staging_root(&self) -> &Path {
        self._staging.path()
    }

    pub fn verified_release(&self) -> &VerifiedReleaseV1 {
        &self.verified_release
    }
}

pub fn prepare_candidate(
    source: &impl CandidateArchiveSource,
    verified_release: VerifiedReleaseV1,
    staging_parent: &Path,
) -> Result<PreparedCandidate, UpdaterError> {
    ensure_real_directory(staging_parent)?;
    let staging = tempfile::Builder::new()
        .prefix(".ah-candidate-")
        .tempdir_in(staging_parent)
        .map_err(|_| candidate("failed to create private candidate staging directory"))?;
    ensure_real_directory(staging.path())?;

    let archive_path = staging.path().join("candidate.zip");
    let archive_file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&archive_path)
        .map_err(|_| candidate("failed to create candidate archive staging file"))?;
    let expected_archive = &verified_release.manifest().archive;
    if expected_archive.size > MAX_ARCHIVE_BYTES {
        return Err(candidate("candidate archive exceeds the allowed size"));
    }
    let mut download = BoundedHashWriter::new(archive_file, expected_archive.size);
    source.download_archive(&verified_release.discovered().assets.archive, &mut download)?;
    let (archive_file, archive_size, archive_sha256) = download.finish()?;
    drop(archive_file);
    if archive_size != expected_archive.size {
        return Err(candidate(
            "downloaded candidate archive size does not match the signed manifest",
        ));
    }
    if archive_sha256 != expected_archive.sha256 {
        return Err(candidate(
            "downloaded candidate archive digest does not match the signed manifest",
        ));
    }

    let plan = preflight_archive(&archive_path, &verified_release)?;
    let root = staging.path().join("bundle");
    fs::create_dir(&root)
        .map_err(|_| candidate("failed to create candidate extraction directory"))?;
    ensure_real_directory(&root)?;
    extract_verified_archive(&archive_path, &root, &plan)?;

    Ok(PreparedCandidate {
        _staging: staging,
        root,
        verified_release,
    })
}

#[derive(Debug)]
struct PlannedFile {
    archive_index: usize,
    path: String,
    size: u64,
    sha256: String,
}

fn preflight_archive(
    archive_path: &Path,
    verified_release: &VerifiedReleaseV1,
) -> Result<Vec<PlannedFile>, UpdaterError> {
    ensure_regular_file(archive_path)?;
    let input = File::open(archive_path)
        .map_err(|_| candidate("failed to open downloaded candidate archive"))?;
    let mut archive =
        ZipArchive::new(input).map_err(|_| candidate("candidate archive is not a valid ZIP"))?;
    let expected = verified_release
        .manifest()
        .files
        .iter()
        .map(|file| (file.path.as_str(), file))
        .collect::<BTreeMap<_, _>>();
    validate_inventory_bounds(expected.values().copied())?;
    let expected_directories = expected_directory_paths(expected.keys().copied());
    let mut observed_files = BTreeSet::new();
    let mut equivalent_paths = BTreeSet::new();
    let mut plan = Vec::with_capacity(expected.len());

    for index in 0..archive.len() {
        let entry = archive
            .by_index(index)
            .map_err(|_| candidate("candidate archive contains an unreadable entry"))?;
        let is_directory = entry.is_dir();
        let name = validate_entry_name(entry.name_raw(), is_directory)?;
        validate_entry_type(&entry, is_directory)?;
        let normalized = name.trim_end_matches('/').to_owned();
        if !equivalent_paths.insert(normalized.to_ascii_lowercase()) {
            return Err(candidate(
                "candidate archive contains duplicate or case-colliding paths",
            ));
        }
        if is_directory {
            if entry.size() != 0 || !expected_directories.contains(&normalized) {
                return Err(candidate(
                    "candidate archive contains an unexpected directory entry",
                ));
            }
            continue;
        }

        let expected_file = expected.get(normalized.as_str()).ok_or_else(|| {
            candidate("candidate archive contains a file absent from the signed manifest")
        })?;
        if !observed_files.insert(normalized.clone()) {
            return Err(candidate(
                "candidate archive contains a duplicate file path",
            ));
        }
        if entry.size() != expected_file.size {
            return Err(candidate(
                "candidate archive file size does not match the signed manifest",
            ));
        }
        plan.push(PlannedFile {
            archive_index: index,
            path: normalized,
            size: expected_file.size,
            sha256: expected_file.sha256.clone(),
        });
    }

    let expected_files = expected.keys().copied().collect::<BTreeSet<_>>();
    let observed_refs = observed_files
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    if observed_refs != expected_files {
        return Err(candidate(
            "candidate archive is missing a file from the signed manifest",
        ));
    }
    plan.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(plan)
}

fn validate_inventory_bounds<'a>(
    files: impl Iterator<Item = &'a ManagedFile>,
) -> Result<(), UpdaterError> {
    let mut total = 0_u64;
    for file in files {
        validate_safe_relative_path(&file.path)?;
        if file.size > MAX_MANAGED_FILE_BYTES {
            return Err(candidate(
                "candidate manifest file exceeds the allowed size",
            ));
        }
        total = total
            .checked_add(file.size)
            .ok_or_else(|| candidate("candidate manifest size overflow"))?;
        if total > MAX_TOTAL_UNCOMPRESSED_BYTES {
            return Err(candidate(
                "candidate manifest exceeds the total extraction size limit",
            ));
        }
    }
    Ok(())
}

fn validate_entry_name(raw: &[u8], is_directory: bool) -> Result<&str, UpdaterError> {
    if raw.is_empty()
        || !raw.is_ascii()
        || raw.contains(&b'\\')
        || raw.contains(&0)
        || raw.contains(&b':')
    {
        return Err(candidate("candidate archive contains an unsafe path"));
    }
    let name = std::str::from_utf8(raw)
        .map_err(|_| candidate("candidate archive contains an unsafe path"))?;
    if name.starts_with('/') || is_directory != name.ends_with('/') {
        return Err(candidate("candidate archive contains an unsafe path"));
    }
    let logical = name.trim_end_matches('/');
    validate_safe_relative_path(logical)?;
    Ok(name)
}

fn validate_safe_relative_path(path: &str) -> Result<(), UpdaterError> {
    if path.is_empty()
        || !path.is_ascii()
        || path.starts_with('/')
        || path.ends_with('/')
        || path.contains(['\\', '\0', ':'])
    {
        return Err(candidate("candidate archive contains an unsafe path"));
    }
    for component in path.split('/') {
        if component.is_empty()
            || matches!(component, "." | "..")
            || component.ends_with(['.', ' '])
            || is_windows_reserved_name(component)
        {
            return Err(candidate("candidate archive contains an unsafe path"));
        }
    }
    Ok(())
}

fn is_windows_reserved_name(component: &str) -> bool {
    let stem = component.split('.').next().unwrap_or(component);
    if ["CON", "PRN", "AUX", "NUL"]
        .iter()
        .any(|reserved| stem.eq_ignore_ascii_case(reserved))
    {
        return true;
    }
    let upper = stem.to_ascii_uppercase();
    matches!(
        upper.as_str(),
        "COM1"
            | "COM2"
            | "COM3"
            | "COM4"
            | "COM5"
            | "COM6"
            | "COM7"
            | "COM8"
            | "COM9"
            | "LPT1"
            | "LPT2"
            | "LPT3"
            | "LPT4"
            | "LPT5"
            | "LPT6"
            | "LPT7"
            | "LPT8"
            | "LPT9"
    )
}

fn validate_entry_type<R: Read>(
    entry: &zip::read::ZipFile<'_, R>,
    is_directory: bool,
) -> Result<(), UpdaterError> {
    if entry.encrypted() || entry.is_symlink() {
        return Err(candidate(
            "candidate archive contains an encrypted entry or link",
        ));
    }
    if !matches!(
        entry.compression(),
        CompressionMethod::Stored | CompressionMethod::Deflated
    ) {
        return Err(candidate(
            "candidate archive uses an unsupported compression method",
        ));
    }
    if let Some(mode) = entry.unix_mode() {
        let file_type = mode & 0o170000;
        let supported = if is_directory {
            matches!(file_type, 0 | 0o040000)
        } else {
            matches!(file_type, 0 | 0o100000)
        };
        if !supported {
            return Err(candidate(
                "candidate archive contains an unsupported file type",
            ));
        }
    }
    Ok(())
}

fn expected_directory_paths<'a>(paths: impl Iterator<Item = &'a str>) -> BTreeSet<String> {
    let mut directories = BTreeSet::new();
    for path in paths {
        let components = path.split('/').collect::<Vec<_>>();
        for end in 1..components.len() {
            directories.insert(components[..end].join("/"));
        }
    }
    directories
}

fn extract_verified_archive(
    archive_path: &Path,
    root: &Path,
    plan: &[PlannedFile],
) -> Result<(), UpdaterError> {
    let input =
        File::open(archive_path).map_err(|_| candidate("failed to reopen candidate archive"))?;
    let mut archive = ZipArchive::new(input)
        .map_err(|_| candidate("candidate archive changed after validation"))?;
    for planned in plan {
        ensure_real_directory(root)?;
        let mut entry = archive
            .by_index(planned.archive_index)
            .map_err(|_| candidate("candidate archive changed after validation"))?;
        if entry.name_raw() != planned.path.as_bytes()
            || entry.is_dir()
            || entry.size() != planned.size
        {
            return Err(candidate("candidate archive changed after validation"));
        }
        let destination = root.join(path_from_slashes(&planned.path));
        let parent = destination
            .parent()
            .ok_or_else(|| candidate("candidate destination path is invalid"))?;
        create_safe_directory_chain(root, parent)?;
        let mut output = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&destination)
            .map_err(|_| candidate("failed to create candidate output file"))?;
        let (size, digest) = copy_and_hash(&mut entry, &mut output, planned.size)?;
        output
            .flush()
            .map_err(|_| candidate("failed to flush candidate output file"))?;
        drop(output);
        ensure_regular_file(&destination)?;
        if size != planned.size || digest != planned.sha256 {
            return Err(candidate(
                "extracted candidate file does not match the signed manifest",
            ));
        }
    }
    Ok(())
}

fn create_safe_directory_chain(root: &Path, destination: &Path) -> Result<(), UpdaterError> {
    let relative = destination
        .strip_prefix(root)
        .map_err(|_| candidate("candidate destination escaped the extraction root"))?;
    let mut current = root.to_path_buf();
    ensure_real_directory(&current)?;
    for component in relative.components() {
        let std::path::Component::Normal(component) = component else {
            return Err(candidate("candidate destination path is invalid"));
        };
        current.push(component);
        match fs::symlink_metadata(&current) {
            Ok(metadata) => ensure_real_directory_metadata(&metadata)?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                fs::create_dir(&current)
                    .map_err(|_| candidate("failed to create candidate directory"))?;
                ensure_real_directory(&current)?;
            }
            Err(_) => return Err(candidate("failed to inspect candidate directory")),
        }
    }
    Ok(())
}

fn path_from_slashes(path: &str) -> PathBuf {
    path.split('/').collect()
}

fn ensure_real_directory(path: &Path) -> Result<(), UpdaterError> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|_| candidate("failed to inspect candidate directory"))?;
    ensure_real_directory_metadata(&metadata)
}

fn ensure_real_directory_metadata(metadata: &fs::Metadata) -> Result<(), UpdaterError> {
    if metadata.file_type().is_symlink()
        || !metadata.is_dir()
        || ah_platform::fs::is_reparse_point(metadata)
    {
        return Err(candidate(
            "candidate staging path must be a real directory without reparse points",
        ));
    }
    Ok(())
}

fn ensure_regular_file(path: &Path) -> Result<(), UpdaterError> {
    let metadata =
        fs::symlink_metadata(path).map_err(|_| candidate("failed to inspect candidate file"))?;
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || ah_platform::fs::is_reparse_point(&metadata)
    {
        return Err(candidate(
            "candidate staging file must be a regular file without reparse points",
        ));
    }
    Ok(())
}

fn copy_and_hash(
    input: &mut impl Read,
    output: &mut impl Write,
    maximum: u64,
) -> Result<(u64, String), UpdaterError> {
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    let mut total = 0_u64;
    loop {
        let read = input
            .read(&mut buffer)
            .map_err(|_| candidate("failed to read candidate archive entry"))?;
        if read == 0 {
            break;
        }
        total = total
            .checked_add(read as u64)
            .ok_or_else(|| candidate("candidate file size overflow"))?;
        if total > maximum {
            return Err(candidate("candidate archive entry exceeds its signed size"));
        }
        output
            .write_all(&buffer[..read])
            .map_err(|_| candidate("failed to write candidate output file"))?;
        digest.update(&buffer[..read]);
    }
    Ok((total, encode_digest(digest.finalize())))
}

struct BoundedHashWriter<W> {
    inner: W,
    digest: Sha256,
    total: u64,
    maximum: u64,
}

impl<W> BoundedHashWriter<W> {
    fn new(inner: W, maximum: u64) -> Self {
        Self {
            inner,
            digest: Sha256::new(),
            total: 0,
            maximum,
        }
    }
}

impl<W: Write> BoundedHashWriter<W> {
    fn finish(mut self) -> Result<(W, u64, String), UpdaterError> {
        self.inner
            .flush()
            .map_err(|_| candidate("failed to flush candidate archive"))?;
        let digest = encode_digest(self.digest.finalize());
        Ok((self.inner, self.total, digest))
    }
}

impl<W: Write> Write for BoundedHashWriter<W> {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        let next = self
            .total
            .checked_add(buffer.len() as u64)
            .ok_or_else(|| io::Error::other("candidate archive size overflow"))?;
        if next > self.maximum {
            return Err(io::Error::other(
                "candidate archive exceeds its declared size",
            ));
        }
        self.inner.write_all(buffer)?;
        self.digest.update(buffer);
        self.total = next;
        Ok(buffer.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

fn encode_digest(bytes: impl AsRef<[u8]>) -> String {
    let bytes = bytes.as_ref();
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        write!(&mut encoded, "{byte:02x}").expect("writing to a String cannot fail");
    }
    encoded
}

fn candidate(detail: &'static str) -> UpdaterError {
    UpdaterError::new(UpdaterErrorCode::Candidate, detail)
}

impl CandidateArchiveSource for super::github::GitHubReleaseClient {
    fn download_archive(
        &self,
        asset: &ReleaseAssetV1,
        output: &mut dyn Write,
    ) -> Result<(), UpdaterError> {
        self.download_asset_to(asset, output)
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::io::Cursor;

    use ah_release_manifest::{
        ArchiveMetadata, FilePurpose, ReleaseManifest, ReleaseMetadata, RequiredFiles,
        SCHEMA_VERSION, SIGNING_ALGORITHM, SIGNING_DOMAIN, SignatureAlgorithm, SigningMetadata,
        TrustedKey, key_id_for_public_key,
    };
    use ah_updater_core::{
        DETACHED_SIGNATURE_BYTES, DiscoveredReleaseV1, ReleaseAssetsV1, ReleaseTrust,
        StableReleaseVersion, UpdaterErrorCode, WINDOWS_X64_TARGET, verify_discovered_release,
    };
    use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
    use ed25519_dalek::{Signer as _, SigningKey};
    use semver::Version;
    use tempfile::TempDir;
    use zip::{ZipWriter, write::SimpleFileOptions};

    use super::*;

    #[test]
    fn prepares_exact_candidate_and_retains_signed_release_material() {
        let inventory = valid_inventory();
        let archive = build_archive(&[
            TestEntry::Directory("plugins/"),
            TestEntry::File("ah.exe", b"ah-binary"),
            TestEntry::File("plugins/github.dll", b"plugin-binary"),
        ]);
        let fixture = signed_fixture(archive.clone(), &inventory);
        let source = FakeSource::new(archive);
        let staging = TempDir::new().unwrap();
        let sentinel = staging.path().join("user-owned.txt");
        fs::write(&sentinel, b"unchanged").unwrap();

        let prepared = prepare_candidate(&source, fixture.verified, staging.path()).unwrap();

        assert_eq!(source.downloads.get(), 1);
        assert_eq!(
            fs::read(prepared.root().join("ah.exe")).unwrap(),
            b"ah-binary"
        );
        assert_eq!(
            fs::read(prepared.root().join("plugins/github.dll")).unwrap(),
            b"plugin-binary"
        );
        assert_eq!(
            prepared.verified_release().manifest_bytes(),
            fixture.manifest_bytes
        );
        assert_eq!(
            prepared.verified_release().signature_bytes(),
            fixture.signature_bytes
        );
        assert_eq!(fs::read(&sentinel).unwrap(), b"unchanged");
        assert_eq!(fs::read_dir(staging.path()).unwrap().count(), 2);
        drop(prepared);
        assert_eq!(fs::read_dir(staging.path()).unwrap().count(), 1);
    }

    #[test]
    fn rejects_unsafe_windows_paths_without_leaving_staging_files() {
        for hostile in [
            "../escape.dll",
            "/absolute.dll",
            "plugins\\backslash.dll",
            "plugins/file.dll:stream",
            "plugins/trailing.",
            "plugins/trailing ",
            "CON.txt",
            "plugins/COM1.dll",
            "AH.EXE",
        ] {
            let inventory = valid_inventory();
            let archive = build_archive(&[
                TestEntry::File("ah.exe", b"ah-binary"),
                TestEntry::File("plugins/github.dll", b"plugin-binary"),
                TestEntry::File(hostile, b"hostile"),
            ]);
            assert_candidate_rejected(archive, &inventory);
        }
    }

    #[test]
    fn rejects_links_missing_extra_and_mismatched_files() {
        let inventory = valid_inventory();
        let linked = build_archive(&[
            TestEntry::File("ah.exe", b"ah-binary"),
            TestEntry::File("plugins/github.dll", b"plugin-binary"),
            TestEntry::Symlink("linked.exe", "ah.exe"),
        ]);
        assert_candidate_rejected(linked, &inventory);

        let missing = build_archive(&[TestEntry::File("ah.exe", b"ah-binary")]);
        assert_candidate_rejected(missing, &inventory);

        let extra = build_archive(&[
            TestEntry::File("ah.exe", b"ah-binary"),
            TestEntry::File("plugins/github.dll", b"plugin-binary"),
            TestEntry::File("extra.txt", b"extra"),
        ]);
        assert_candidate_rejected(extra, &inventory);

        let wrong_size = build_archive(&[
            TestEntry::File("ah.exe", b"wrong-size"),
            TestEntry::File("plugins/github.dll", b"plugin-binary"),
        ]);
        assert_candidate_rejected(wrong_size, &inventory);

        let wrong_digest = build_archive(&[
            TestEntry::File("ah.exe", b"no-binary"),
            TestEntry::File("plugins/github.dll", b"plugin-binary"),
        ]);
        assert_candidate_rejected(wrong_digest, &inventory);
    }

    #[test]
    fn rejects_corrupt_or_overlong_archive_downloads() {
        let inventory = valid_inventory();
        let archive = build_archive(&[
            TestEntry::File("ah.exe", b"ah-binary"),
            TestEntry::File("plugins/github.dll", b"plugin-binary"),
        ]);
        let fixture = signed_fixture(archive.clone(), &inventory);
        let staging = TempDir::new().unwrap();

        let mut corrupt = archive.clone();
        corrupt[0] ^= 1;
        let error = prepare_candidate(
            &FakeSource::new(corrupt),
            fixture.verified.clone(),
            staging.path(),
        )
        .unwrap_err();
        assert_eq!(error.code(), UpdaterErrorCode::Candidate);
        assert_eq!(fs::read_dir(staging.path()).unwrap().count(), 0);

        let mut overlong = archive;
        overlong.push(0);
        let error = prepare_candidate(&FakeSource::new(overlong), fixture.verified, staging.path())
            .unwrap_err();
        assert_eq!(error.code(), UpdaterErrorCode::Candidate);
        assert_eq!(fs::read_dir(staging.path()).unwrap().count(), 0);
    }

    #[test]
    fn enforces_per_file_and_total_extraction_limits_before_writes() {
        let oversized = ManagedFile {
            path: "ah.exe".to_owned(),
            size: MAX_MANAGED_FILE_BYTES + 1,
            sha256: "0".repeat(64),
            purpose: FilePurpose::Executable,
        };
        assert_eq!(
            validate_inventory_bounds(std::iter::once(&oversized))
                .unwrap_err()
                .code(),
            UpdaterErrorCode::Candidate
        );

        let first = ManagedFile {
            path: "one.bin".to_owned(),
            size: MAX_MANAGED_FILE_BYTES,
            sha256: "0".repeat(64),
            purpose: FilePurpose::Support,
        };
        let files = [
            first.clone(),
            ManagedFile {
                path: "two.bin".to_owned(),
                ..first.clone()
            },
            ManagedFile {
                path: "three.bin".to_owned(),
                ..first.clone()
            },
            ManagedFile {
                path: "four.bin".to_owned(),
                ..first.clone()
            },
            ManagedFile {
                path: "five.bin".to_owned(),
                size: 1,
                ..first
            },
        ];
        assert_eq!(
            validate_inventory_bounds(files.iter()).unwrap_err().code(),
            UpdaterErrorCode::Candidate
        );
    }

    fn assert_candidate_rejected(archive: Vec<u8>, inventory: &[InventoryFile]) {
        let fixture = signed_fixture(archive.clone(), inventory);
        let source = FakeSource::new(archive);
        let staging = TempDir::new().unwrap();
        let sentinel = staging.path().join("user-owned.txt");
        fs::write(&sentinel, b"unchanged").unwrap();

        let error = prepare_candidate(&source, fixture.verified, staging.path()).unwrap_err();

        assert_eq!(error.code(), UpdaterErrorCode::Candidate);
        assert_eq!(fs::read(&sentinel).unwrap(), b"unchanged");
        assert_eq!(fs::read_dir(staging.path()).unwrap().count(), 1);
    }

    #[derive(Clone, Copy)]
    struct InventoryFile {
        path: &'static str,
        content: &'static [u8],
        purpose: FilePurpose,
    }

    fn valid_inventory() -> Vec<InventoryFile> {
        vec![
            InventoryFile {
                path: "ah.exe",
                content: b"ah-binary",
                purpose: FilePurpose::Executable,
            },
            InventoryFile {
                path: "plugins/github.dll",
                content: b"plugin-binary",
                purpose: FilePurpose::Plugin,
            },
        ]
    }

    enum TestEntry<'a> {
        File(&'a str, &'a [u8]),
        Directory(&'a str),
        Symlink(&'a str, &'a str),
    }

    fn build_archive(entries: &[TestEntry<'_>]) -> Vec<u8> {
        let cursor = Cursor::new(Vec::new());
        let mut writer = ZipWriter::new(cursor);
        let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
        for entry in entries {
            match entry {
                TestEntry::File(path, content) => {
                    writer.start_file(*path, options).unwrap();
                    writer.write_all(content).unwrap();
                }
                TestEntry::Directory(path) => writer.add_directory(*path, options).unwrap(),
                TestEntry::Symlink(path, target) => {
                    writer.add_symlink(*path, *target, options).unwrap()
                }
            }
        }
        writer.finish().unwrap().into_inner()
    }

    struct SignedFixture {
        verified: VerifiedReleaseV1,
        manifest_bytes: Vec<u8>,
        signature_bytes: Vec<u8>,
    }

    fn signed_fixture(archive: Vec<u8>, inventory: &[InventoryFile]) -> SignedFixture {
        let signing = SigningKey::from_bytes(&[23_u8; 32]);
        let public_key = signing.verifying_key().to_bytes();
        let key_id = key_id_for_public_key(&public_key);
        let version = "1.2.0";
        let tag = format!("v{version}");
        let archive_url = format!(
            "https://github.com/Bobsans/AIHelper/releases/download/{tag}/{}",
            WINDOWS_X64_TARGET.archive_name
        );
        let files = inventory
            .iter()
            .map(|file| ManagedFile {
                path: file.path.to_owned(),
                size: file.content.len() as u64,
                sha256: sha256(file.content),
                purpose: file.purpose,
            })
            .collect::<Vec<_>>();
        let manifest = ReleaseManifest {
            schema_version: SCHEMA_VERSION,
            release: ReleaseMetadata {
                version: version.to_owned(),
                target: WINDOWS_X64_TARGET.rust_target.to_owned(),
                architecture: WINDOWS_X64_TARGET.architecture.to_owned(),
            },
            archive: ArchiveMetadata {
                url: archive_url,
                size: archive.len() as u64,
                sha256: sha256(&archive),
            },
            minimum_updater_version: "1.0.0".to_owned(),
            signing: SigningMetadata {
                key_id: key_id.clone(),
                algorithm: SignatureAlgorithm::Ed25519,
            },
            files,
            required: RequiredFiles {
                executables: vec!["ah.exe".to_owned()],
                plugins: vec!["plugins/github.dll".to_owned()],
            },
        };
        let manifest_bytes = manifest.to_canonical_bytes().unwrap();
        let mut preimage = Vec::from(SIGNING_DOMAIN);
        preimage.extend_from_slice(&manifest_bytes);
        let signature_bytes = URL_SAFE_NO_PAD
            .encode(signing.sign(&preimage).to_bytes())
            .into_bytes();
        assert_eq!(signature_bytes.len(), DETACHED_SIGNATURE_BYTES);

        let archive_asset = asset(1, WINDOWS_X64_TARGET.archive_name, archive.len(), &tag);
        let manifest_asset = asset(
            2,
            WINDOWS_X64_TARGET.manifest_name,
            manifest_bytes.len(),
            &tag,
        );
        let signature_asset = asset(
            3,
            WINDOWS_X64_TARGET.signature_name,
            signature_bytes.len(),
            &tag,
        );
        let discovered = DiscoveredReleaseV1 {
            release_id: 1,
            tag: tag.clone(),
            version: StableReleaseVersion::parse_tag(&tag).unwrap(),
            target: WINDOWS_X64_TARGET,
            assets: ReleaseAssetsV1 {
                archive: archive_asset,
                manifest: manifest_asset,
                signature: signature_asset,
            },
        };
        let trust = ReleaseTrust::from_keys(vec![TrustedKey {
            key_id,
            algorithm: SIGNING_ALGORITHM.to_owned(),
            public_key,
        }])
        .unwrap();
        let verified = verify_discovered_release(
            discovered,
            &manifest_bytes,
            &signature_bytes,
            &trust,
            &Version::parse("1.1.0").unwrap(),
        )
        .unwrap();
        SignedFixture {
            verified,
            manifest_bytes,
            signature_bytes,
        }
    }

    fn asset(id: u64, name: &str, size: usize, tag: &str) -> ReleaseAssetV1 {
        ReleaseAssetV1 {
            id,
            name: name.to_owned(),
            size: size as u64,
            api_url: format!("https://api.github.com/repos/Bobsans/AIHelper/releases/assets/{id}"),
            browser_download_url: format!(
                "https://github.com/Bobsans/AIHelper/releases/download/{tag}/{name}"
            ),
        }
    }

    fn sha256(bytes: &[u8]) -> String {
        encode_digest(Sha256::digest(bytes))
    }

    struct FakeSource {
        archive: Vec<u8>,
        downloads: Cell<usize>,
    }

    impl FakeSource {
        fn new(archive: Vec<u8>) -> Self {
            Self {
                archive,
                downloads: Cell::new(0),
            }
        }
    }

    impl CandidateArchiveSource for FakeSource {
        fn download_archive(
            &self,
            _asset: &ReleaseAssetV1,
            output: &mut dyn Write,
        ) -> Result<(), UpdaterError> {
            self.downloads.set(self.downloads.get() + 1);
            output
                .write_all(&self.archive)
                .map_err(|_| candidate("test archive write failed"))
        }
    }
}
