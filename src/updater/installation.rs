use std::{
    fmt::Write as _,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
};

use ah_updater_core::{
    DETACHED_SIGNATURE_BYTES, DiscoveredReleaseV1, InstallationIdentityV1, MAX_MANIFEST_BYTES,
    ReleaseAssetV1, ReleaseManifest, ReleaseTrust, UpdaterError, UpdaterErrorCode,
    VerifiedReleaseV1, WINDOWS_X64_TARGET, verify_discovered_release,
};
use semver::Version;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::error::AppError;

const MAX_IDENTITY_BYTES: usize = 16 * 1024;
const MAX_MANAGED_FILE_BYTES: u64 = 256 * 1024 * 1024;
const MAX_TOTAL_MANAGED_BYTES: u64 = 1024 * 1024 * 1024;
const INSTALLED_MANIFEST_FILE: &str = "installed.manifest.json";
const INSTALLED_SIGNATURE_FILE: &str = "installed.manifest.sig";
const IDENTITY_FILE: &str = "identity.json";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PortableInstallation {
    executable: PathBuf,
    root: PathBuf,
}

impl PortableInstallation {
    pub(crate) fn executable(&self) -> &Path {
        &self.executable
    }

    pub(crate) fn root(&self) -> &Path {
        &self.root
    }
}

#[derive(Debug)]
pub(crate) struct ManagedInstallation {
    portable: PortableInstallation,
    state_root: PathBuf,
    identity: InstallationIdentityV1,
    manifest: ReleaseManifest,
}

impl ManagedInstallation {
    pub(crate) fn portable(&self) -> &PortableInstallation {
        &self.portable
    }

    pub(crate) fn identity(&self) -> &InstallationIdentityV1 {
        &self.identity
    }

    pub(crate) fn state_root(&self) -> &Path {
        &self.state_root
    }

    pub(crate) fn manifest(&self) -> &ReleaseManifest {
        &self.manifest
    }
}

pub(crate) trait LegacyReleaseSource {
    fn discover_version(&self, version: &Version) -> Result<DiscoveredReleaseV1, UpdaterError>;
    fn download(&self, asset: &ReleaseAssetV1) -> Result<Vec<u8>, UpdaterError>;
}

#[derive(Debug, Clone)]
struct InstallationStatePaths {
    root: PathBuf,
    bindings: PathBuf,
    installations: PathBuf,
}

impl InstallationStatePaths {
    fn from_root(root: &Path) -> Result<Self, UpdaterError> {
        if !root.is_absolute() || root.to_str().is_none() {
            return Err(installation(
                "updater state root must be an absolute Unicode path",
            ));
        }
        Ok(Self {
            root: root.to_path_buf(),
            bindings: root.join("bindings"),
            installations: root.join("installations"),
        })
    }

    fn discover() -> Result<Self, UpdaterError> {
        #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
        {
            let app_data = std::env::var_os("APPDATA")
                .map(PathBuf::from)
                .filter(|path| !path.as_os_str().is_empty())
                .ok_or_else(|| installation("unable to resolve per-user updater state"))?;
            Self::from_root(&app_data.join("AIHelper").join("updater"))
        }
        #[cfg(not(all(target_os = "windows", target_arch = "x86_64")))]
        {
            Err(UpdaterError::new(
                UpdaterErrorCode::UnsupportedPlatform,
                "self-update requires Windows x86_64",
            ))
        }
    }

    fn binding(&self, executable: &Path) -> Result<PathBuf, UpdaterError> {
        Ok(self
            .bindings
            .join(format!("{}.json", executable_binding_key(executable)?)))
    }

    fn installation(&self, installation_id: Uuid) -> PathBuf {
        self.installations.join(installation_id.to_string())
    }
}

pub(crate) fn inspect_current_portable_installation() -> Result<PortableInstallation, UpdaterError>
{
    let executable = std::env::current_exe()
        .map_err(|_| installation("failed to resolve the running AIHelper executable"))?;
    inspect_portable_installation(&executable)
}

pub(crate) fn resolve_current_managed_installation() -> Result<ManagedInstallation, UpdaterError> {
    ah_updater_core::UpdateTarget::current()?;
    let portable = inspect_current_portable_installation()?;
    let state = InstallationStatePaths::discover()?;
    let trust = super::trust::production_release_trust()?;
    let source = super::github::GitHubReleaseClient::new()?;
    let current_version = Version::parse(env!("CARGO_PKG_VERSION")).map_err(|_| {
        UpdaterError::new(
            UpdaterErrorCode::ReleaseContract,
            "running AIHelper version is not canonical SemVer",
        )
    })?;
    resolve_managed_installation(&portable, &state, &source, &trust, &current_version)
}

pub(crate) fn load_current_managed_installation(
    trust: &ReleaseTrust,
) -> Result<ManagedInstallation, UpdaterError> {
    ah_updater_core::UpdateTarget::current()?;
    let portable = inspect_current_portable_installation()?;
    let state = InstallationStatePaths::discover()?;
    let current_version = Version::parse(env!("CARGO_PKG_VERSION")).map_err(|_| {
        UpdaterError::new(
            UpdaterErrorCode::ReleaseContract,
            "running AIHelper version is not canonical SemVer",
        )
    })?;
    let identity = load_identity(&state.binding(portable.executable())?)?
        .ok_or_else(|| installation("self-update rollback requires a managed installation"))?;
    load_managed_installation(&portable, &state, identity, trust, &current_version)
}

fn resolve_managed_installation(
    portable: &PortableInstallation,
    state: &InstallationStatePaths,
    source: &impl LegacyReleaseSource,
    trust: &ReleaseTrust,
    current_version: &Version,
) -> Result<ManagedInstallation, UpdaterError> {
    let binding_path = state.binding(portable.executable())?;
    if let Some(identity) = load_identity(&binding_path)? {
        return load_managed_installation(portable, state, identity, trust, current_version);
    }

    let discovered = source.discover_version(current_version)?;
    let manifest_bytes = source.download(&discovered.assets.manifest)?;
    let signature_bytes = source.download(&discovered.assets.signature)?;
    let verified = verify_discovered_release(
        discovered,
        &manifest_bytes,
        &signature_bytes,
        trust,
        current_version,
    )?;
    verify_managed_files(portable.root(), verified.manifest())?;
    let identity = persist_verified_legacy_installation(portable, state, verified, &binding_path)?;
    load_managed_installation(portable, state, identity, trust, current_version)
}

fn load_managed_installation(
    portable: &PortableInstallation,
    state: &InstallationStatePaths,
    identity: InstallationIdentityV1,
    trust: &ReleaseTrust,
    current_version: &Version,
) -> Result<ManagedInstallation, UpdaterError> {
    identity.validate()?;
    let persisted_path = Path::new(&identity.executable_path);
    if !paths_equal(persisted_path, portable.executable()) {
        return Err(installation(
            "installation identity is bound to a different executable path",
        ));
    }
    let directory = state.installation(identity.installation_id);
    let stored_identity = load_required_identity(&directory.join(IDENTITY_FILE))?;
    if stored_identity != identity {
        return Err(installation("installation identity records do not match"));
    }
    let manifest_bytes =
        read_bounded_file(&directory.join(INSTALLED_MANIFEST_FILE), MAX_MANIFEST_BYTES)?;
    let signature_bytes = read_bounded_file(
        &directory.join(INSTALLED_SIGNATURE_FILE),
        DETACHED_SIGNATURE_BYTES,
    )?;
    if signature_bytes.len() != DETACHED_SIGNATURE_BYTES {
        return Err(installation(
            "installed release signature has an invalid size",
        ));
    }
    let verified = trust.verify(&manifest_bytes, &signature_bytes)?;
    let manifest = verified.into_manifest();
    validate_installed_manifest(&manifest, current_version)?;
    verify_managed_files(portable.root(), &manifest)?;
    Ok(ManagedInstallation {
        portable: portable.clone(),
        state_root: directory,
        identity,
        manifest,
    })
}

fn persist_verified_legacy_installation(
    portable: &PortableInstallation,
    state: &InstallationStatePaths,
    verified: VerifiedReleaseV1,
    binding_path: &Path,
) -> Result<InstallationIdentityV1, UpdaterError> {
    ensure_state_directory(&state.root)?;
    ensure_state_directory(&state.bindings)?;
    ensure_state_directory(&state.installations)?;
    let executable_path = portable
        .executable()
        .to_str()
        .ok_or_else(|| installation("installation executable path is not valid Unicode"))?
        .to_owned();
    let manifest_bytes = verified.manifest_bytes().to_vec();
    let signature_bytes = verified.signature_bytes().to_vec();

    crate::persistence::transaction(binding_path, || {
        if let Some(existing) = load_identity(binding_path).map_err(persistence_bridge)? {
            return Ok(existing);
        }
        let (identity, directory) =
            create_identity_directory(state, &executable_path).map_err(persistence_bridge)?;
        let write_result =
            write_identity_directory(&directory, &identity, &manifest_bytes, &signature_bytes);
        if let Err(error) = write_result {
            let _ = remove_created_identity_directory(state, &directory);
            return Err(persistence_bridge(error));
        }
        if let Err(error) = crate::persistence::atomic_write_json(binding_path, &identity) {
            let _ = remove_created_identity_directory(state, &directory);
            return Err(error);
        }
        Ok(identity)
    })
    .map_err(|_| installation("failed to persist verified installation identity"))
}

fn create_identity_directory(
    state: &InstallationStatePaths,
    executable_path: &str,
) -> Result<(InstallationIdentityV1, PathBuf), UpdaterError> {
    for _ in 0..4 {
        let identity = InstallationIdentityV1::new(Uuid::new_v4(), executable_path)?;
        let directory = state.installation(identity.installation_id);
        match fs::create_dir(&directory) {
            Ok(()) => return Ok((identity, directory)),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(_) => {
                return Err(installation(
                    "failed to create installation identity directory",
                ));
            }
        }
    }
    Err(installation(
        "failed to allocate a unique installation identity",
    ))
}

fn write_identity_directory(
    directory: &Path,
    identity: &InstallationIdentityV1,
    manifest_bytes: &[u8],
    signature_bytes: &[u8],
) -> Result<(), UpdaterError> {
    let mut identity_bytes = serde_json::to_vec_pretty(identity)
        .map_err(|_| installation("failed to serialize installation identity"))?;
    identity_bytes.push(b'\n');
    write_new_synced(&directory.join(IDENTITY_FILE), &identity_bytes)?;
    write_new_synced(&directory.join(INSTALLED_MANIFEST_FILE), manifest_bytes)?;
    write_new_synced(&directory.join(INSTALLED_SIGNATURE_FILE), signature_bytes)
}

fn write_new_synced(path: &Path, bytes: &[u8]) -> Result<(), UpdaterError> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|_| installation("failed to create verified installation state file"))?;
    file.write_all(bytes)
        .and_then(|_| file.sync_all())
        .map_err(|_| installation("failed to write verified installation state file"))
}

fn remove_created_identity_directory(
    state: &InstallationStatePaths,
    directory: &Path,
) -> Result<(), UpdaterError> {
    if directory.parent() != Some(state.installations.as_path()) {
        return Err(installation(
            "refused to clean an unexpected installation state path",
        ));
    }
    fs::remove_dir_all(directory)
        .map_err(|_| installation("failed to clean incomplete installation state"))
}

fn validate_installed_manifest(
    manifest: &ReleaseManifest,
    current_version: &Version,
) -> Result<(), UpdaterError> {
    if manifest.release.version != current_version.to_string() {
        return Err(installation(
            "installed manifest version does not match the running executable",
        ));
    }
    WINDOWS_X64_TARGET
        .require_manifest_identity(&manifest.release.target, &manifest.release.architecture)?;
    let minimum = Version::parse(&manifest.minimum_updater_version)
        .map_err(|_| installation("installed manifest updater floor is invalid"))?;
    if current_version < &minimum {
        return Err(UpdaterError::new(
            UpdaterErrorCode::Compatibility,
            "running updater is older than the installed manifest compatibility floor",
        ));
    }
    Ok(())
}

fn verify_managed_files(root: &Path, manifest: &ReleaseManifest) -> Result<(), UpdaterError> {
    let total = manifest.files.iter().try_fold(0_u64, |total, file| {
        if file.size > MAX_MANAGED_FILE_BYTES {
            return Err(installation(
                "installed managed file exceeds the verification size limit",
            ));
        }
        total
            .checked_add(file.size)
            .filter(|total| *total <= MAX_TOTAL_MANAGED_BYTES)
            .ok_or_else(|| installation("installed managed files exceed the total size limit"))
    })?;
    let _ = total;
    for file in &manifest.files {
        let path = direct_managed_file(root, &file.path)?;
        let metadata = fs::metadata(&path)
            .map_err(|_| installation("failed to inspect an installed managed file"))?;
        if metadata.len() != file.size {
            return Err(installation(
                "installed managed file size does not match the signed manifest",
            ));
        }
        let digest = hash_file(&path, file.size)?;
        if digest != file.sha256 {
            return Err(installation(
                "installed managed file digest does not match the signed manifest",
            ));
        }
    }
    Ok(())
}

fn direct_managed_file(root: &Path, relative: &str) -> Result<PathBuf, UpdaterError> {
    let mut path = root.to_path_buf();
    let components = relative.split('/').collect::<Vec<_>>();
    for component in components.iter().take(components.len().saturating_sub(1)) {
        path.push(component);
        ensure_direct_directory(&path)?;
    }
    let file_name = components
        .last()
        .ok_or_else(|| installation("signed managed file path is empty"))?;
    path.push(file_name);
    ensure_direct_file(&path)?;
    Ok(path)
}

fn ensure_direct_directory(path: &Path) -> Result<(), UpdaterError> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|_| installation("installed managed directory is missing"))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() || is_reparse_point(&metadata) {
        return Err(installation(
            "installed managed directory is not a direct directory",
        ));
    }
    Ok(())
}

fn ensure_direct_file(path: &Path) -> Result<(), UpdaterError> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|_| installation("installed managed file is missing"))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() || is_reparse_point(&metadata) {
        return Err(installation(
            "installed managed file is not a direct regular file",
        ));
    }
    #[cfg(windows)]
    {
        if !has_single_hard_link(path)? {
            return Err(installation(
                "installed managed file must not be a hard link",
            ));
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;

        if metadata.nlink() != 1 {
            return Err(installation(
                "installed managed file must not be a hard link",
            ));
        }
    }
    Ok(())
}

#[cfg(windows)]
fn has_single_hard_link(path: &Path) -> Result<bool, UpdaterError> {
    use std::{mem::MaybeUninit, os::windows::io::AsRawHandle};
    use windows_sys::Win32::{
        Foundation::HANDLE,
        Storage::FileSystem::{BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle},
    };

    let file =
        File::open(path).map_err(|_| installation("failed to open an installed managed file"))?;
    let mut information = MaybeUninit::<BY_HANDLE_FILE_INFORMATION>::uninit();
    let succeeded = unsafe {
        GetFileInformationByHandle(file.as_raw_handle() as HANDLE, information.as_mut_ptr())
    };
    if succeeded == 0 {
        return Err(installation(
            "failed to inspect installed managed file links",
        ));
    }
    let information = unsafe { information.assume_init() };
    Ok(information.nNumberOfLinks == 1)
}

fn is_reparse_point(metadata: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;

        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
        metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
    }
    #[cfg(not(windows))]
    {
        false
    }
}

fn hash_file(path: &Path, expected_size: u64) -> Result<String, UpdaterError> {
    let mut file =
        File::open(path).map_err(|_| installation("failed to open an installed managed file"))?;
    let mut remaining = expected_size;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    while remaining > 0 {
        let requested = usize::try_from(remaining.min(buffer.len() as u64))
            .expect("bounded managed-file chunk fits usize");
        let read = file
            .read(&mut buffer[..requested])
            .map_err(|_| installation("failed to read an installed managed file"))?;
        if read == 0 {
            return Err(installation(
                "installed managed file ended before its signed size",
            ));
        }
        digest.update(&buffer[..read]);
        remaining -= read as u64;
    }
    let mut extra = [0_u8; 1];
    if file
        .read(&mut extra)
        .map_err(|_| installation("failed to read an installed managed file"))?
        != 0
    {
        return Err(installation(
            "installed managed file exceeds its signed size",
        ));
    }
    Ok(encode_digest(digest.finalize()))
}

fn load_identity(path: &Path) -> Result<Option<InstallationIdentityV1>, UpdaterError> {
    match fs::symlink_metadata(path) {
        Ok(_) => load_required_identity(path).map(Some),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(_) => Err(installation("failed to inspect installation identity")),
    }
}

fn load_required_identity(path: &Path) -> Result<InstallationIdentityV1, UpdaterError> {
    let bytes = read_bounded_file(path, MAX_IDENTITY_BYTES)?;
    let identity = serde_json::from_slice::<InstallationIdentityV1>(&bytes)
        .map_err(|_| installation("installation identity is not valid JSON"))?;
    identity.validate()?;
    Ok(identity)
}

fn read_bounded_file(path: &Path, maximum: usize) -> Result<Vec<u8>, UpdaterError> {
    ensure_direct_file(path)?;
    let metadata = fs::metadata(path)
        .map_err(|_| installation("failed to inspect installation state file"))?;
    if metadata.len() > maximum as u64 {
        return Err(installation(
            "installation state file exceeds its size limit",
        ));
    }
    let mut file =
        File::open(path).map_err(|_| installation("failed to open installation state file"))?;
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.read_to_end(&mut bytes)
        .map_err(|_| installation("failed to read installation state file"))?;
    Ok(bytes)
}

fn ensure_state_directory(path: &Path) -> Result<(), UpdaterError> {
    fs::create_dir_all(path)
        .map_err(|_| installation("failed to create per-user updater state directory"))?;
    ensure_direct_directory(path)
}

fn executable_binding_key(executable: &Path) -> Result<String, UpdaterError> {
    let value = executable
        .to_str()
        .ok_or_else(|| installation("installation executable path is not valid Unicode"))?
        .replace('/', "\\");
    #[cfg(windows)]
    let value = value.to_lowercase();
    Ok(encode_digest(Sha256::digest(value.as_bytes())))
}

fn paths_equal(left: &Path, right: &Path) -> bool {
    let (Some(left), Some(right)) = (left.to_str(), right.to_str()) else {
        return false;
    };
    #[cfg(windows)]
    {
        use windows::Win32::Globalization::{CSTR_EQUAL, CompareStringOrdinal};

        let left = left.replace('/', "\\").encode_utf16().collect::<Vec<_>>();
        let right = right.replace('/', "\\").encode_utf16().collect::<Vec<_>>();
        // SAFETY: both UTF-16 buffers remain alive for the duration of the call.
        unsafe { CompareStringOrdinal(&left, &right, true) == CSTR_EQUAL }
    }
    #[cfg(not(windows))]
    {
        left == right
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

fn persistence_bridge(_error: UpdaterError) -> AppError {
    AppError::external(
        "UPDATER_INSTALLATION",
        "failed to persist verified installation identity",
    )
}

fn inspect_portable_installation(executable: &Path) -> Result<PortableInstallation, UpdaterError> {
    ensure_direct_executable(executable)?;
    let executable = fs::canonicalize(executable)
        .map_err(|_| installation("failed to canonicalize the running AIHelper executable"))?;
    ensure_direct_executable(&executable)?;
    if executable.to_str().is_none() {
        return Err(installation(
            "the running AIHelper executable path is not valid Unicode",
        ));
    }
    if !is_expected_executable_name(&executable) {
        return Err(installation(
            "the running executable does not use the supported AIHelper filename",
        ));
    }
    let root = executable
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .ok_or_else(|| installation("failed to derive the AIHelper installation root"))?
        .to_path_buf();

    if cargo_install_root(&root).is_some() {
        return Err(installation(
            "cargo-managed AIHelper cannot self-update; run `cargo install aihelper --locked --force`",
        ));
    }

    Ok(PortableInstallation { executable, root })
}

fn ensure_direct_executable(path: &Path) -> Result<(), UpdaterError> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|_| installation("failed to inspect the running AIHelper executable"))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(installation(
            "the running AIHelper executable is not a direct regular file",
        ));
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;

        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
        if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(installation(
                "the running AIHelper executable is a reparse point",
            ));
        }
    }
    Ok(())
}

fn is_expected_executable_name(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    #[cfg(windows)]
    {
        name.eq_ignore_ascii_case("ah.exe")
    }
    #[cfg(not(windows))]
    {
        name == "ah"
    }
}

fn cargo_install_root(installation_root: &Path) -> Option<PathBuf> {
    if !installation_root
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.eq_ignore_ascii_case("bin"))
    {
        return None;
    }
    let cargo_root = installation_root.parent()?;
    [".crates2.json", ".crates.toml"]
        .into_iter()
        .any(|marker| is_direct_regular_file(&cargo_root.join(marker)))
        .then(|| cargo_root.to_path_buf())
}

fn is_direct_regular_file(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|metadata| {
        !metadata.file_type().is_symlink() && metadata.is_file() && {
            #[cfg(windows)]
            {
                use std::os::windows::fs::MetadataExt;

                const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
                metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT == 0
            }
            #[cfg(not(windows))]
            {
                true
            }
        }
    })
}

impl LegacyReleaseSource for super::github::GitHubReleaseClient {
    fn discover_version(&self, version: &Version) -> Result<DiscoveredReleaseV1, UpdaterError> {
        self.discover_version(version)
    }

    fn download(&self, asset: &ReleaseAssetV1) -> Result<Vec<u8>, UpdaterError> {
        self.download_asset(asset)
    }
}

fn installation(detail: &'static str) -> UpdaterError {
    UpdaterError::new(UpdaterErrorCode::Installation, detail)
}

#[cfg(test)]
mod tests {
    use std::{cell::Cell, fs};

    use ah_release_manifest::{
        ArchiveMetadata, FilePurpose, ManagedFile, ReleaseMetadata, RequiredFiles, SCHEMA_VERSION,
        SIGNING_ALGORITHM, SIGNING_DOMAIN, SignatureAlgorithm, SigningMetadata, TrustedKey,
        key_id_for_public_key,
    };
    use ah_updater_core::{ReleaseAssetsV1, StableReleaseVersion};
    use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
    use ed25519_dalek::{Signer as _, SigningKey};
    use tempfile::TempDir;

    use super::*;

    const TEST_VERSION: &str = "1.1.0";
    const EXECUTABLE_BYTES: &[u8] = b"portable executable";
    const PLUGIN_BYTES: &[u8] = b"plugin-good";

    #[test]
    fn derives_canonical_root_without_mutating_portable_installation() {
        let temp = TempDir::new().unwrap();
        let root = temp.path().join("Portable AIHelper Юникод");
        fs::create_dir(&root).unwrap();
        let executable = root.join(executable_name());
        fs::write(&executable, b"binary").unwrap();
        let sentinel = root.join("user-owned.txt");
        fs::write(&sentinel, b"unchanged").unwrap();

        let installation = inspect_portable_installation(&executable).unwrap();

        assert_eq!(
            installation.executable(),
            fs::canonicalize(&executable).unwrap()
        );
        assert_eq!(installation.root(), fs::canonicalize(&root).unwrap());
        assert_eq!(fs::read(sentinel).unwrap(), b"unchanged");
        assert_eq!(fs::read_dir(root).unwrap().count(), 2);
    }

    #[test]
    fn refuses_cargo_managed_layout_without_mutation() {
        for marker in [".crates2.json", ".crates.toml"] {
            let temp = TempDir::new().unwrap();
            let bin = temp.path().join("bin");
            fs::create_dir(&bin).unwrap();
            let executable = bin.join(executable_name());
            fs::write(&executable, b"binary").unwrap();
            fs::write(temp.path().join(marker), b"cargo metadata").unwrap();
            let sentinel = temp.path().join("sentinel.txt");
            fs::write(&sentinel, b"unchanged").unwrap();

            let error = inspect_portable_installation(&executable).unwrap_err();

            assert_eq!(error.code(), UpdaterErrorCode::Installation);
            assert!(error.detail().contains("cargo install"));
            assert_eq!(fs::read(sentinel).unwrap(), b"unchanged");
            assert_eq!(fs::read(&executable).unwrap(), b"binary");
        }
    }

    #[test]
    fn accepts_portable_bin_directory_without_cargo_metadata() {
        let temp = TempDir::new().unwrap();
        let bin = temp.path().join("bin");
        fs::create_dir(&bin).unwrap();
        let executable = bin.join(executable_name());
        fs::write(&executable, b"binary").unwrap();

        let installation = inspect_portable_installation(&executable).unwrap();

        assert_eq!(installation.root(), fs::canonicalize(bin).unwrap());
    }

    #[test]
    fn rejects_directory_and_unexpected_executable_name() {
        let temp = TempDir::new().unwrap();
        assert_eq!(
            inspect_portable_installation(temp.path())
                .unwrap_err()
                .code(),
            UpdaterErrorCode::Installation
        );

        let unexpected = temp.path().join(if cfg!(windows) {
            "renamed.exe"
        } else {
            "renamed"
        });
        fs::write(&unexpected, b"binary").unwrap();
        assert_eq!(
            inspect_portable_installation(&unexpected)
                .unwrap_err()
                .code(),
            UpdaterErrorCode::Installation
        );
    }

    #[test]
    fn adopts_verified_legacy_installation_and_reuses_local_record_offline() {
        let installation = TestInstallation::new();
        let fixture = signed_fixture(&installation.portable);
        let source = FakeSource::new(&fixture);
        let current_version = Version::parse(TEST_VERSION).unwrap();

        let managed = resolve_managed_installation(
            &installation.portable,
            &installation.state,
            &source,
            &fixture.trust,
            &current_version,
        )
        .unwrap();

        assert_eq!(source.discoveries.get(), 1);
        assert_eq!(source.downloads.get(), 2);
        assert_eq!(managed.portable(), &installation.portable);
        assert_eq!(managed.manifest().release.version, TEST_VERSION);
        assert_eq!(
            managed.identity().executable_path,
            installation.portable.executable().to_str().unwrap()
        );
        let binding = installation
            .state
            .binding(installation.portable.executable())
            .unwrap();
        let persisted: InstallationIdentityV1 =
            serde_json::from_slice(&fs::read(&binding).unwrap()).unwrap();
        assert_eq!(persisted, *managed.identity());
        let identity_directory = installation.state.installation(persisted.installation_id);
        assert_eq!(
            fs::read(identity_directory.join(INSTALLED_MANIFEST_FILE)).unwrap(),
            fixture.manifest
        );
        assert_eq!(
            fs::read(identity_directory.join(INSTALLED_SIGNATURE_FILE)).unwrap(),
            fixture.signature
        );
        assert_eq!(fs::read(&installation.sentinel).unwrap(), b"unchanged");
        assert_eq!(
            fs::read(installation.portable.executable()).unwrap(),
            EXECUTABLE_BYTES
        );

        let offline = OfflineSource::default();
        let loaded = resolve_managed_installation(
            &installation.portable,
            &installation.state,
            &offline,
            &fixture.trust,
            &current_version,
        )
        .unwrap();

        assert_eq!(
            loaded.identity().installation_id,
            managed.identity().installation_id
        );
        assert_eq!(offline.calls.get(), 0);
        assert_eq!(
            fs::read_dir(&installation.state.installations)
                .unwrap()
                .count(),
            1
        );
    }

    #[test]
    fn rejects_modified_legacy_file_without_publishing_identity() {
        let installation = TestInstallation::new();
        let fixture = signed_fixture(&installation.portable);
        fs::write(&installation.plugin, b"plugin-evil").unwrap();
        let source = FakeSource::new(&fixture);

        let error = resolve_managed_installation(
            &installation.portable,
            &installation.state,
            &source,
            &fixture.trust,
            &Version::parse(TEST_VERSION).unwrap(),
        )
        .unwrap_err();

        assert_eq!(error.code(), UpdaterErrorCode::Installation);
        assert_eq!(source.discoveries.get(), 1);
        assert_eq!(source.downloads.get(), 2);
        assert!(!installation.state.root.exists());
        assert_eq!(fs::read(&installation.sentinel).unwrap(), b"unchanged");
        assert_eq!(
            fs::read(installation.portable.executable()).unwrap(),
            EXECUTABLE_BYTES
        );
    }

    #[test]
    fn rejects_identity_bound_to_another_path_without_network_fallback() {
        let installation = TestInstallation::new();
        let fixture = signed_fixture(&installation.portable);
        let current_version = Version::parse(TEST_VERSION).unwrap();
        let managed = resolve_managed_installation(
            &installation.portable,
            &installation.state,
            &FakeSource::new(&fixture),
            &fixture.trust,
            &current_version,
        )
        .unwrap();
        let binding = installation
            .state
            .binding(installation.portable.executable())
            .unwrap();
        let mut mismatched = managed.identity().clone();
        mismatched.executable_path = installation
            .portable
            .root()
            .join("another-ah.exe")
            .to_str()
            .unwrap()
            .to_owned();
        crate::persistence::atomic_write_json(&binding, &mismatched).unwrap();
        let offline = OfflineSource::default();

        let error = resolve_managed_installation(
            &installation.portable,
            &installation.state,
            &offline,
            &fixture.trust,
            &current_version,
        )
        .unwrap_err();

        assert_eq!(error.code(), UpdaterErrorCode::Installation);
        assert_eq!(offline.calls.get(), 0);
        assert_eq!(fs::read(&installation.sentinel).unwrap(), b"unchanged");
    }

    struct TestInstallation {
        _temp: TempDir,
        portable: PortableInstallation,
        state: InstallationStatePaths,
        sentinel: PathBuf,
        plugin: PathBuf,
    }

    impl TestInstallation {
        fn new() -> Self {
            let temp = TempDir::new().unwrap();
            let root = temp.path().join("Portable AIHelper Юникод");
            fs::create_dir(&root).unwrap();
            let executable = root.join(executable_name());
            fs::write(&executable, EXECUTABLE_BYTES).unwrap();
            let plugin_directory = root.join("plugins");
            fs::create_dir(&plugin_directory).unwrap();
            let plugin = plugin_directory.join("github.dll");
            fs::write(&plugin, PLUGIN_BYTES).unwrap();
            let sentinel = root.join("user-owned.txt");
            fs::write(&sentinel, b"unchanged").unwrap();
            let portable = inspect_portable_installation(&executable).unwrap();
            let state =
                InstallationStatePaths::from_root(&temp.path().join("Updater State Юникод"))
                    .unwrap();
            Self {
                _temp: temp,
                portable,
                state,
                sentinel,
                plugin,
            }
        }
    }

    struct SignedFixture {
        discovered: DiscoveredReleaseV1,
        manifest: Vec<u8>,
        signature: Vec<u8>,
        trust: ReleaseTrust,
    }

    struct FakeSource<'a> {
        fixture: &'a SignedFixture,
        discoveries: Cell<usize>,
        downloads: Cell<usize>,
    }

    impl<'a> FakeSource<'a> {
        fn new(fixture: &'a SignedFixture) -> Self {
            Self {
                fixture,
                discoveries: Cell::new(0),
                downloads: Cell::new(0),
            }
        }
    }

    impl LegacyReleaseSource for FakeSource<'_> {
        fn discover_version(&self, version: &Version) -> Result<DiscoveredReleaseV1, UpdaterError> {
            self.discoveries.set(self.discoveries.get() + 1);
            assert_eq!(version, self.fixture.discovered.version.version());
            Ok(self.fixture.discovered.clone())
        }

        fn download(&self, asset: &ReleaseAssetV1) -> Result<Vec<u8>, UpdaterError> {
            self.downloads.set(self.downloads.get() + 1);
            if asset.name == WINDOWS_X64_TARGET.manifest_name {
                Ok(self.fixture.manifest.clone())
            } else if asset.name == WINDOWS_X64_TARGET.signature_name {
                Ok(self.fixture.signature.clone())
            } else {
                Err(UpdaterError::new(
                    UpdaterErrorCode::Network,
                    "unexpected test asset",
                ))
            }
        }
    }

    #[derive(Default)]
    struct OfflineSource {
        calls: Cell<usize>,
    }

    impl LegacyReleaseSource for OfflineSource {
        fn discover_version(
            &self,
            _version: &Version,
        ) -> Result<DiscoveredReleaseV1, UpdaterError> {
            self.calls.set(self.calls.get() + 1);
            Err(UpdaterError::new(
                UpdaterErrorCode::Network,
                "offline test source must not be called",
            ))
        }

        fn download(&self, _asset: &ReleaseAssetV1) -> Result<Vec<u8>, UpdaterError> {
            self.calls.set(self.calls.get() + 1);
            Err(UpdaterError::new(
                UpdaterErrorCode::Network,
                "offline test source must not be called",
            ))
        }
    }

    fn signed_fixture(portable: &PortableInstallation) -> SignedFixture {
        let signing = SigningKey::from_bytes(&[31_u8; 32]);
        let public_key = signing.verifying_key().to_bytes();
        let key_id = key_id_for_public_key(&public_key);
        let tag = format!("v{TEST_VERSION}");
        let archive_url = format!(
            "https://github.com/Bobsans/AIHelper/releases/download/{tag}/{}",
            WINDOWS_X64_TARGET.archive_name
        );
        let executable_path = portable
            .executable()
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .to_owned();
        let plugin_path = "plugins/github.dll".to_owned();
        let mut files = vec![
            managed_file(
                executable_path.clone(),
                EXECUTABLE_BYTES,
                FilePurpose::Executable,
            ),
            managed_file(plugin_path.clone(), PLUGIN_BYTES, FilePurpose::Plugin),
        ];
        files.sort_by(|left, right| left.path.cmp(&right.path));
        let manifest = ReleaseManifest {
            schema_version: SCHEMA_VERSION,
            release: ReleaseMetadata {
                version: TEST_VERSION.to_owned(),
                target: WINDOWS_X64_TARGET.rust_target.to_owned(),
                architecture: WINDOWS_X64_TARGET.architecture.to_owned(),
            },
            archive: ArchiveMetadata {
                url: archive_url,
                size: 100,
                sha256: "0".repeat(64),
            },
            minimum_updater_version: "1.0.0".to_owned(),
            signing: SigningMetadata {
                key_id: key_id.clone(),
                algorithm: SignatureAlgorithm::Ed25519,
            },
            files,
            required: RequiredFiles {
                executables: vec![executable_path],
                plugins: vec![plugin_path],
            },
        };
        let manifest = manifest.to_canonical_bytes().unwrap();
        let mut preimage = Vec::from(SIGNING_DOMAIN);
        preimage.extend_from_slice(&manifest);
        let signature = URL_SAFE_NO_PAD
            .encode(signing.sign(&preimage).to_bytes())
            .into_bytes();
        assert_eq!(signature.len(), DETACHED_SIGNATURE_BYTES);
        let discovered = DiscoveredReleaseV1 {
            release_id: 1,
            tag: tag.clone(),
            version: StableReleaseVersion::parse_tag(&tag).unwrap(),
            target: WINDOWS_X64_TARGET,
            assets: ReleaseAssetsV1 {
                archive: asset(1, WINDOWS_X64_TARGET.archive_name, 100, &tag),
                manifest: asset(
                    2,
                    WINDOWS_X64_TARGET.manifest_name,
                    manifest.len() as u64,
                    &tag,
                ),
                signature: asset(
                    3,
                    WINDOWS_X64_TARGET.signature_name,
                    signature.len() as u64,
                    &tag,
                ),
            },
        };
        let trust = ReleaseTrust::from_keys(vec![TrustedKey {
            key_id,
            algorithm: SIGNING_ALGORITHM.to_owned(),
            public_key,
        }])
        .unwrap();
        SignedFixture {
            discovered,
            manifest,
            signature,
            trust,
        }
    }

    fn managed_file(path: String, bytes: &[u8], purpose: FilePurpose) -> ManagedFile {
        ManagedFile {
            path,
            size: bytes.len() as u64,
            sha256: encode_digest(Sha256::digest(bytes)),
            purpose,
        }
    }

    fn asset(id: u64, name: &str, size: u64, tag: &str) -> ReleaseAssetV1 {
        ReleaseAssetV1 {
            id,
            name: name.to_owned(),
            size,
            api_url: format!("https://api.github.com/repos/Bobsans/AIHelper/releases/assets/{id}"),
            browser_download_url: format!(
                "https://github.com/Bobsans/AIHelper/releases/download/{tag}/{name}"
            ),
        }
    }

    fn executable_name() -> &'static str {
        if cfg!(windows) { "ah.exe" } else { "ah" }
    }
}
