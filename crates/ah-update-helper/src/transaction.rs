use std::{
    fmt::Write as _,
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    path::{Component, Path, PathBuf},
};

use ah_updater_core::{
    InstallationIdentityV1, ManagedFile, ManagedFileOperationV1, ReleaseManifest, ReleaseTrust,
    TransactionJournalV1, TransactionPlanV1, TransactionStateV1, UpdaterError, UpdaterErrorCode,
};
use sha2::{Digest, Sha256};

const IDENTITY_FILE: &str = "identity.json";
const INSTALLED_MANIFEST_FILE: &str = "installed.manifest.json";
const INSTALLED_SIGNATURE_FILE: &str = "installed.manifest.sig";
const PLAN_FILE: &str = "plan.json";
const JOURNAL_FILE: &str = "journal.json";
const CANDIDATE_DIRECTORY: &str = "candidate";
const BACKUP_DIRECTORY: &str = "backup";
const FILES_DIRECTORY: &str = "files";
const MAX_IDENTITY_BYTES: usize = 16 * 1024;
const MAX_PLAN_BYTES: usize = 4 * 1024 * 1024;
const MAX_JOURNAL_BYTES: usize = 64 * 1024;
const MAX_MANAGED_FILE_BYTES: u64 = 256 * 1024 * 1024;
const MAX_TOTAL_MANAGED_BYTES: u64 = 1024 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransactionPaths {
    installation_root: PathBuf,
    installation_state_root: PathBuf,
    transaction_root: PathBuf,
}

impl TransactionPaths {
    pub fn new(
        installation_root: impl Into<PathBuf>,
        installation_state_root: impl Into<PathBuf>,
        transaction_root: impl Into<PathBuf>,
    ) -> Self {
        Self {
            installation_root: installation_root.into(),
            installation_state_root: installation_state_root.into(),
            transaction_root: transaction_root.into(),
        }
    }

    pub fn installation_root(&self) -> &Path {
        &self.installation_root
    }

    pub fn installation_state_root(&self) -> &Path {
        &self.installation_state_root
    }

    pub fn transaction_root(&self) -> &Path {
        &self.transaction_root
    }

    pub fn candidate_files_root(&self) -> PathBuf {
        self.transaction_root
            .join(CANDIDATE_DIRECTORY)
            .join(FILES_DIRECTORY)
    }

    pub fn backup_files_root(&self) -> PathBuf {
        self.transaction_root
            .join(BACKUP_DIRECTORY)
            .join(FILES_DIRECTORY)
    }

    fn candidate_root(&self) -> PathBuf {
        self.transaction_root.join(CANDIDATE_DIRECTORY)
    }

    fn backup_root(&self) -> PathBuf {
        self.transaction_root.join(BACKUP_DIRECTORY)
    }
}

#[derive(Debug, Clone)]
pub struct LoadedTransaction {
    paths: TransactionPaths,
    plan: TransactionPlanV1,
    journal: TransactionJournalV1,
    identity: InstallationIdentityV1,
    old_manifest: ReleaseManifest,
    new_manifest: ReleaseManifest,
}

impl LoadedTransaction {
    pub fn paths(&self) -> &TransactionPaths {
        &self.paths
    }

    pub fn plan(&self) -> &TransactionPlanV1 {
        &self.plan
    }

    pub fn journal(&self) -> &TransactionJournalV1 {
        &self.journal
    }

    pub fn identity(&self) -> &InstallationIdentityV1 {
        &self.identity
    }

    pub fn old_manifest(&self) -> &ReleaseManifest {
        &self.old_manifest
    }

    pub fn new_manifest(&self) -> &ReleaseManifest {
        &self.new_manifest
    }
}

pub fn prepare_transaction(
    paths: &TransactionPaths,
    plan: &TransactionPlanV1,
    candidate_source_root: &Path,
    candidate_manifest_bytes: &[u8],
    candidate_signature_bytes: &[u8],
    trust: &ReleaseTrust,
) -> Result<LoadedTransaction, UpdaterError> {
    validate_prepare_paths(paths, candidate_source_root)?;
    plan.validate()?;

    let active = read_release_record(&paths.installation_state_root, trust)?;
    let candidate =
        verify_release_record(candidate_manifest_bytes, candidate_signature_bytes, trust)?;
    plan.validate_for_manifests(&active.manifest, &candidate.manifest)?;

    let identity_bytes = read_bounded(
        &paths.installation_state_root.join(IDENTITY_FILE),
        MAX_IDENTITY_BYTES,
        "failed to read installation identity for transaction backup",
    )?;
    let identity = decode_identity(&identity_bytes)?;
    validate_identity(paths, plan, &identity, &active.manifest)?;
    verify_manifest_files(&paths.installation_root, &active.manifest)?;
    verify_add_destinations_absent(
        &paths.installation_root,
        &plan.operations,
        &candidate.manifest,
    )?;
    verify_manifest_files(candidate_source_root, &candidate.manifest)?;

    create_transaction_layout(paths)?;
    let mut cleanup = PreparationCleanup::new(paths.transaction_root.clone());
    write_new_synced(
        &paths.transaction_root.join(PLAN_FILE),
        &plan.to_canonical_bytes()?,
    )?;
    write_new_synced(
        &paths.candidate_root().join(INSTALLED_MANIFEST_FILE),
        candidate_manifest_bytes,
    )?;
    write_new_synced(
        &paths.candidate_root().join(INSTALLED_SIGNATURE_FILE),
        candidate_signature_bytes,
    )?;
    write_new_synced(&paths.backup_root().join(IDENTITY_FILE), &identity_bytes)?;
    write_new_synced(
        &paths.backup_root().join(INSTALLED_MANIFEST_FILE),
        &active.manifest_bytes,
    )?;
    write_new_synced(
        &paths.backup_root().join(INSTALLED_SIGNATURE_FILE),
        &active.signature_bytes,
    )?;

    let mut journal = TransactionJournalV1::new(plan)?;
    persist_initial_journal(paths, &journal)?;
    copy_manifest_files(
        candidate_source_root,
        &paths.candidate_files_root(),
        &candidate.manifest,
    )?;
    copy_manifest_files(
        &paths.installation_root,
        &paths.backup_files_root(),
        &active.manifest,
    )?;
    verify_manifest_files(&paths.candidate_files_root(), &candidate.manifest)?;
    verify_manifest_files(&paths.backup_files_root(), &active.manifest)?;

    journal.advance(TransactionStateV1::BackupPrepared)?;
    replace_journal(paths, &journal)?;
    cleanup.disarm();
    load_transaction(paths, trust)
}

pub fn load_transaction(
    paths: &TransactionPaths,
    trust: &ReleaseTrust,
) -> Result<LoadedTransaction, UpdaterError> {
    validate_existing_paths(paths)?;
    let plan_bytes = read_bounded(
        &paths.transaction_root.join(PLAN_FILE),
        MAX_PLAN_BYTES,
        "failed to read durable transaction plan",
    )?;
    let plan: TransactionPlanV1 = serde_json::from_slice(&plan_bytes)
        .map_err(|_| recovery("durable transaction plan is malformed"))?;
    plan.validate()?;
    if plan.to_canonical_bytes()? != plan_bytes {
        return Err(recovery(
            "durable transaction plan encoding is not canonical",
        ));
    }

    let journal_bytes = read_bounded(
        &paths.transaction_root.join(JOURNAL_FILE),
        MAX_JOURNAL_BYTES,
        "failed to read durable transaction journal",
    )?;
    let journal: TransactionJournalV1 = serde_json::from_slice(&journal_bytes)
        .map_err(|_| recovery("durable transaction journal is malformed"))?;
    journal.validate_for(&plan)?;
    if encode_journal(&journal)? != journal_bytes {
        return Err(recovery(
            "durable transaction journal encoding is not canonical",
        ));
    }

    let backup = read_release_record(&paths.backup_root(), trust)?;
    let candidate = read_release_record(&paths.candidate_root(), trust)?;
    plan.validate_for_manifests(&backup.manifest, &candidate.manifest)?;
    let identity_bytes = read_bounded(
        &paths.backup_root().join(IDENTITY_FILE),
        MAX_IDENTITY_BYTES,
        "failed to read transaction backup identity",
    )?;
    let identity = decode_identity(&identity_bytes)?;
    validate_identity(paths, &plan, &identity, &backup.manifest)?;
    verify_manifest_files(&paths.backup_files_root(), &backup.manifest)?;
    verify_manifest_files(&paths.candidate_files_root(), &candidate.manifest)?;

    Ok(LoadedTransaction {
        paths: paths.clone(),
        plan,
        journal,
        identity,
        old_manifest: backup.manifest,
        new_manifest: candidate.manifest,
    })
}

pub fn load_prepared_transaction(
    paths: &TransactionPaths,
    trust: &ReleaseTrust,
) -> Result<LoadedTransaction, UpdaterError> {
    let transaction = load_transaction(paths, trust)?;
    if transaction.journal.state != TransactionStateV1::BackupPrepared {
        return Err(recovery(
            "durable transaction is not at the prepared activation boundary",
        ));
    }
    verify_active_old_release(&transaction, trust)?;
    Ok(transaction)
}

fn verify_active_old_release(
    transaction: &LoadedTransaction,
    trust: &ReleaseTrust,
) -> Result<(), UpdaterError> {
    let paths = &transaction.paths;
    let active = read_release_record(&paths.installation_state_root, trust)?;
    if active.manifest != transaction.old_manifest {
        return Err(recovery(
            "active release record does not match the transaction backup",
        ));
    }
    let active_identity = read_bounded(
        &paths.installation_state_root.join(IDENTITY_FILE),
        MAX_IDENTITY_BYTES,
        "failed to read active installation identity",
    )?;
    let backup_identity = read_bounded(
        &paths.backup_root().join(IDENTITY_FILE),
        MAX_IDENTITY_BYTES,
        "failed to read transaction backup identity",
    )?;
    if active_identity != backup_identity {
        return Err(recovery(
            "active installation identity does not match the transaction backup",
        ));
    }
    verify_manifest_files(&paths.installation_root, &transaction.old_manifest)?;
    verify_add_destinations_absent(
        &paths.installation_root,
        &transaction.plan.operations,
        &transaction.new_manifest,
    )
}

#[derive(Debug)]
struct ReleaseRecord {
    manifest: ReleaseManifest,
    manifest_bytes: Vec<u8>,
    signature_bytes: Vec<u8>,
}

fn read_release_record(root: &Path, trust: &ReleaseTrust) -> Result<ReleaseRecord, UpdaterError> {
    let manifest_bytes = read_bounded(
        &root.join(INSTALLED_MANIFEST_FILE),
        ah_updater_core::MAX_MANIFEST_BYTES,
        "failed to read durable release manifest",
    )?;
    let signature_bytes = read_bounded(
        &root.join(INSTALLED_SIGNATURE_FILE),
        ah_updater_core::DETACHED_SIGNATURE_BYTES,
        "failed to read durable release signature",
    )?;
    verify_release_record(&manifest_bytes, &signature_bytes, trust)
}

fn verify_release_record(
    manifest_bytes: &[u8],
    signature_bytes: &[u8],
    trust: &ReleaseTrust,
) -> Result<ReleaseRecord, UpdaterError> {
    let manifest = trust
        .verify(manifest_bytes, signature_bytes)?
        .into_manifest();
    Ok(ReleaseRecord {
        manifest,
        manifest_bytes: manifest_bytes.to_vec(),
        signature_bytes: signature_bytes.to_vec(),
    })
}

fn decode_identity(bytes: &[u8]) -> Result<InstallationIdentityV1, UpdaterError> {
    let identity: InstallationIdentityV1 = serde_json::from_slice(bytes)
        .map_err(|_| transaction("installation identity backup is malformed"))?;
    identity.validate()?;
    Ok(identity)
}

fn validate_identity(
    paths: &TransactionPaths,
    plan: &TransactionPlanV1,
    identity: &InstallationIdentityV1,
    old_manifest: &ReleaseManifest,
) -> Result<(), UpdaterError> {
    if identity.installation_id != plan.installation_id {
        return Err(transaction(
            "installation identity does not match the transaction plan",
        ));
    }
    let executable = Path::new(&identity.executable_path);
    if !executable.is_absolute()
        || executable.to_str().is_none()
        || executable
            .components()
            .any(|component| matches!(component, Component::CurDir | Component::ParentDir))
    {
        return Err(transaction(
            "installation identity executable path is not absolute and normalized",
        ));
    }
    if !old_manifest.files.iter().any(|file| {
        paths_equal(
            executable,
            &paths.installation_root.join(path_from_slashes(&file.path)),
        )
    }) {
        return Err(transaction(
            "installation identity executable is absent from the old manifest",
        ));
    }
    Ok(())
}

fn validate_prepare_paths(
    paths: &TransactionPaths,
    candidate_source_root: &Path,
) -> Result<(), UpdaterError> {
    validate_absolute_paths(paths)?;
    if !candidate_source_root.is_absolute() {
        return Err(transaction("candidate source root must be absolute"));
    }
    ensure_direct_directory(&paths.installation_root)?;
    ensure_direct_directory(&paths.installation_state_root)?;
    ensure_direct_directory(candidate_source_root)?;
    let transaction_parent = paths
        .transaction_root
        .parent()
        .ok_or_else(|| transaction("transaction root has no parent directory"))?;
    ensure_direct_directory(transaction_parent)?;
    match fs::symlink_metadata(&paths.transaction_root) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Ok(_) => return Err(transaction("transaction root already exists")),
        Err(_) => return Err(transaction("failed to inspect transaction root")),
    }

    let installation = fs::canonicalize(&paths.installation_root)
        .map_err(|_| transaction("failed to resolve the installation root"))?;
    let state = fs::canonicalize(&paths.installation_state_root)
        .map_err(|_| transaction("failed to resolve the installation state root"))?;
    let candidate = fs::canonicalize(candidate_source_root)
        .map_err(|_| transaction("failed to resolve the candidate source root"))?;
    let parent = fs::canonicalize(transaction_parent)
        .map_err(|_| transaction("failed to resolve the transaction parent"))?;
    let name = paths
        .transaction_root
        .file_name()
        .ok_or_else(|| transaction("transaction root has no directory name"))?;
    let transaction_root = parent.join(name);
    if overlaps(&installation, &state)
        || overlaps(&installation, &candidate)
        || overlaps(&installation, &transaction_root)
        || overlaps(&candidate, &transaction_root)
    {
        return Err(transaction(
            "installation, state, candidate, and transaction roots overlap unsafely",
        ));
    }
    Ok(())
}

fn validate_existing_paths(paths: &TransactionPaths) -> Result<(), UpdaterError> {
    validate_absolute_paths(paths)?;
    ensure_direct_directory(&paths.installation_root)?;
    ensure_direct_directory(&paths.installation_state_root)?;
    ensure_direct_directory(&paths.transaction_root)?;
    ensure_direct_directory(&paths.candidate_root())?;
    ensure_direct_directory(&paths.candidate_files_root())?;
    ensure_direct_directory(&paths.backup_root())?;
    ensure_direct_directory(&paths.backup_files_root())?;
    let installation = fs::canonicalize(&paths.installation_root)
        .map_err(|_| recovery("failed to resolve the installation root"))?;
    let state = fs::canonicalize(&paths.installation_state_root)
        .map_err(|_| recovery("failed to resolve the installation state root"))?;
    let transaction_root = fs::canonicalize(&paths.transaction_root)
        .map_err(|_| recovery("failed to resolve the transaction root"))?;
    if overlaps(&installation, &state) || overlaps(&installation, &transaction_root) {
        return Err(recovery(
            "installation and durable transaction state roots overlap",
        ));
    }
    Ok(())
}

fn validate_absolute_paths(paths: &TransactionPaths) -> Result<(), UpdaterError> {
    for path in [
        &paths.installation_root,
        &paths.installation_state_root,
        &paths.transaction_root,
    ] {
        if !path.is_absolute()
            || path.to_str().is_none()
            || path
                .components()
                .any(|component| matches!(component, Component::CurDir | Component::ParentDir))
        {
            return Err(transaction(
                "transaction paths must be absolute normalized Unicode paths",
            ));
        }
    }
    Ok(())
}

fn overlaps(left: &Path, right: &Path) -> bool {
    path_starts_with(left, right) || path_starts_with(right, left)
}

#[cfg(windows)]
fn path_starts_with(path: &Path, base: &Path) -> bool {
    let path = path.to_string_lossy().replace('/', "\\").to_lowercase();
    let base = base.to_string_lossy().replace('/', "\\").to_lowercase();
    path == base
        || path
            .strip_prefix(&base)
            .is_some_and(|suffix| suffix.starts_with('\\'))
}

#[cfg(not(windows))]
fn path_starts_with(path: &Path, base: &Path) -> bool {
    path.starts_with(base)
}

fn create_transaction_layout(paths: &TransactionPaths) -> Result<(), UpdaterError> {
    create_synced_directory(&paths.transaction_root)?;
    create_synced_directory(&paths.candidate_root())?;
    create_synced_directory(&paths.candidate_files_root())?;
    create_synced_directory(&paths.backup_root())?;
    create_synced_directory(&paths.backup_files_root())
}

fn create_synced_directory(path: &Path) -> Result<(), UpdaterError> {
    fs::create_dir(path)
        .map_err(|_| transaction("failed to create private transaction directory"))?;
    ensure_direct_directory(path)?;
    sync_parent(path).map_err(|_| transaction("failed to sync transaction directory metadata"))
}

fn persist_initial_journal(
    paths: &TransactionPaths,
    journal: &TransactionJournalV1,
) -> Result<(), UpdaterError> {
    write_new_synced(
        &paths.transaction_root.join(JOURNAL_FILE),
        &encode_journal(journal)?,
    )
}

fn replace_journal(
    paths: &TransactionPaths,
    journal: &TransactionJournalV1,
) -> Result<(), UpdaterError> {
    journal.validate()?;
    let temporary = paths.transaction_root.join(format!(
        ".journal-{}-{}.tmp",
        journal.transaction_id, journal.transition_sequence
    ));
    write_new_synced(&temporary, &encode_journal(journal)?)?;
    atomic_replace(&temporary, &paths.transaction_root.join(JOURNAL_FILE))
        .map_err(|_| transaction("failed to durably replace transaction journal"))
}

fn encode_journal(journal: &TransactionJournalV1) -> Result<Vec<u8>, UpdaterError> {
    serde_json::to_vec(journal)
        .map_err(|_| transaction("failed to serialize durable transaction journal"))
}

fn write_new_synced(path: &Path, bytes: &[u8]) -> Result<(), UpdaterError> {
    let parent = path
        .parent()
        .ok_or_else(|| transaction("transaction state path has no parent"))?;
    ensure_direct_directory(parent)?;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|_| transaction("failed to create durable transaction state file"))?;
    file.write_all(bytes)
        .and_then(|_| file.sync_all())
        .map_err(|_| transaction("failed to write durable transaction state file"))?;
    drop(file);
    ensure_direct_file(path)?;
    sync_parent(path).map_err(|_| transaction("failed to sync durable transaction state metadata"))
}

fn copy_manifest_files(
    source_root: &Path,
    destination_root: &Path,
    manifest: &ReleaseManifest,
) -> Result<(), UpdaterError> {
    validate_inventory_bounds(manifest)?;
    for managed in &manifest.files {
        let source = existing_managed_file(source_root, &managed.path)?;
        let destination = create_managed_destination(destination_root, &managed.path)?;
        copy_verified_file(&source, &destination, managed)?;
    }
    Ok(())
}

fn verify_manifest_files(root: &Path, manifest: &ReleaseManifest) -> Result<(), UpdaterError> {
    validate_inventory_bounds(manifest)?;
    for managed in &manifest.files {
        let path = existing_managed_file(root, &managed.path)?;
        verify_file(&path, managed)?;
    }
    Ok(())
}

fn validate_inventory_bounds(manifest: &ReleaseManifest) -> Result<(), UpdaterError> {
    manifest
        .files
        .iter()
        .try_fold(0_u64, |total, file| {
            if file.size > MAX_MANAGED_FILE_BYTES {
                return Err(transaction(
                    "managed file exceeds the transaction size limit",
                ));
            }
            total
                .checked_add(file.size)
                .filter(|total| *total <= MAX_TOTAL_MANAGED_BYTES)
                .ok_or_else(|| transaction("managed files exceed the total transaction size limit"))
        })
        .map(|_| ())
}

fn verify_add_destinations_absent(
    installation_root: &Path,
    operations: &[ManagedFileOperationV1],
    new_manifest: &ReleaseManifest,
) -> Result<(), UpdaterError> {
    for operation in operations {
        if let ManagedFileOperationV1::Add { new } = operation {
            if !new_manifest.files.iter().any(|file| file == new) {
                return Err(transaction(
                    "transaction add operation is absent from the new manifest",
                ));
            }
            let destination = prospective_managed_file(installation_root, &new.path)?;
            match fs::symlink_metadata(&destination) {
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Ok(_) => {
                    return Err(transaction(
                        "transaction add path collides with an existing file",
                    ));
                }
                Err(_) => {
                    return Err(transaction("failed to inspect transaction add destination"));
                }
            }
        }
    }
    Ok(())
}

fn copy_verified_file(
    source: &Path,
    destination: &Path,
    expected: &ManagedFile,
) -> Result<(), UpdaterError> {
    let metadata = fs::metadata(source)
        .map_err(|_| transaction("failed to inspect transaction source file"))?;
    if metadata.len() != expected.size {
        return Err(transaction(
            "transaction source file size does not match the signed manifest",
        ));
    }
    let mut input =
        File::open(source).map_err(|_| transaction("failed to open transaction source file"))?;
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)
        .map_err(|_| transaction("failed to create transaction copy"))?;
    let digest = copy_exact_hash(&mut input, &mut output, expected.size)?;
    output
        .sync_all()
        .map_err(|_| transaction("failed to sync transaction copy"))?;
    drop(output);
    ensure_direct_file(destination)?;
    if digest != expected.sha256 {
        return Err(transaction(
            "transaction source file digest does not match the signed manifest",
        ));
    }
    sync_parent(destination).map_err(|_| transaction("failed to sync transaction copy metadata"))
}

fn verify_file(path: &Path, expected: &ManagedFile) -> Result<(), UpdaterError> {
    let metadata = fs::metadata(path)
        .map_err(|_| transaction("failed to inspect transaction managed file"))?;
    if metadata.len() != expected.size {
        return Err(transaction(
            "transaction managed file size does not match the signed manifest",
        ));
    }
    let mut input =
        File::open(path).map_err(|_| transaction("failed to open transaction managed file"))?;
    let digest = hash_exact(&mut input, expected.size)?;
    if digest != expected.sha256 {
        return Err(transaction(
            "transaction managed file digest does not match the signed manifest",
        ));
    }
    Ok(())
}

fn copy_exact_hash(
    input: &mut impl Read,
    output: &mut impl Write,
    expected_size: u64,
) -> Result<String, UpdaterError> {
    hash_stream(input, Some(output), expected_size)
}

fn hash_exact(input: &mut impl Read, expected_size: u64) -> Result<String, UpdaterError> {
    hash_stream::<_, io::Sink>(input, None, expected_size)
}

fn hash_stream<R: Read, W: Write>(
    input: &mut R,
    mut output: Option<&mut W>,
    expected_size: u64,
) -> Result<String, UpdaterError> {
    let mut remaining = expected_size;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    while remaining > 0 {
        let requested = usize::try_from(remaining.min(buffer.len() as u64))
            .expect("bounded transaction chunk fits usize");
        let read = input
            .read(&mut buffer[..requested])
            .map_err(|_| transaction("failed to read transaction managed file"))?;
        if read == 0 {
            return Err(transaction(
                "transaction managed file ended before its signed size",
            ));
        }
        if let Some(output) = output.as_deref_mut() {
            output
                .write_all(&buffer[..read])
                .map_err(|_| transaction("failed to write transaction copy"))?;
        }
        digest.update(&buffer[..read]);
        remaining -= read as u64;
    }
    let mut extra = [0_u8; 1];
    if input
        .read(&mut extra)
        .map_err(|_| transaction("failed to read transaction managed file"))?
        != 0
    {
        return Err(transaction(
            "transaction managed file exceeds its signed size",
        ));
    }
    Ok(encode_digest(digest.finalize()))
}

fn encode_digest(bytes: impl AsRef<[u8]>) -> String {
    let bytes = bytes.as_ref();
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        write!(&mut encoded, "{byte:02x}").expect("writing to a String cannot fail");
    }
    encoded
}

fn existing_managed_file(root: &Path, relative: &str) -> Result<PathBuf, UpdaterError> {
    ensure_direct_directory(root)?;
    let path = prospective_managed_file(root, relative)?;
    ensure_direct_file(&path)?;
    Ok(path)
}

fn prospective_managed_file(root: &Path, relative: &str) -> Result<PathBuf, UpdaterError> {
    let components = relative.split('/').collect::<Vec<_>>();
    let file_name = components
        .last()
        .filter(|component| !component.is_empty())
        .ok_or_else(|| transaction("transaction managed path is empty"))?;
    let mut path = root.to_path_buf();
    let mut missing_parent = false;
    for component in components.iter().take(components.len().saturating_sub(1)) {
        path.push(component);
        if missing_parent {
            continue;
        }
        match fs::symlink_metadata(&path) {
            Ok(metadata) => ensure_direct_directory_metadata(&metadata)?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => missing_parent = true,
            Err(_) => return Err(transaction("failed to inspect transaction directory")),
        }
    }
    path.push(file_name);
    Ok(path)
}

fn create_managed_destination(root: &Path, relative: &str) -> Result<PathBuf, UpdaterError> {
    ensure_direct_directory(root)?;
    let components = relative.split('/').collect::<Vec<_>>();
    let file_name = components
        .last()
        .filter(|component| !component.is_empty())
        .ok_or_else(|| transaction("transaction managed path is empty"))?;
    let mut path = root.to_path_buf();
    for component in components.iter().take(components.len().saturating_sub(1)) {
        path.push(component);
        match fs::symlink_metadata(&path) {
            Ok(metadata) => ensure_direct_directory_metadata(&metadata)?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                create_synced_directory(&path)?;
            }
            Err(_) => return Err(transaction("failed to inspect transaction directory")),
        }
    }
    path.push(file_name);
    Ok(path)
}

fn path_from_slashes(path: &str) -> PathBuf {
    path.split('/').collect()
}

#[cfg(windows)]
fn paths_equal(left: &Path, right: &Path) -> bool {
    left.to_string_lossy()
        .replace('/', "\\")
        .eq_ignore_ascii_case(&right.to_string_lossy().replace('/', "\\"))
}

#[cfg(not(windows))]
fn paths_equal(left: &Path, right: &Path) -> bool {
    left == right
}

fn ensure_direct_directory(path: &Path) -> Result<(), UpdaterError> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|_| transaction("failed to inspect transaction directory"))?;
    ensure_direct_directory_metadata(&metadata)
}

fn ensure_direct_directory_metadata(metadata: &fs::Metadata) -> Result<(), UpdaterError> {
    if metadata.file_type().is_symlink() || !metadata.is_dir() || is_reparse_point(metadata) {
        return Err(transaction(
            "transaction path contains a redirected directory",
        ));
    }
    Ok(())
}

fn ensure_direct_file(path: &Path) -> Result<(), UpdaterError> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|_| transaction("failed to inspect transaction file"))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() || is_reparse_point(&metadata) {
        return Err(transaction("transaction path is not a direct regular file"));
    }
    if !has_single_hard_link(path, &metadata)? {
        return Err(transaction(
            "transaction managed file must not be a hard link",
        ));
    }
    Ok(())
}

#[cfg(windows)]
fn has_single_hard_link(path: &Path, _metadata: &fs::Metadata) -> Result<bool, UpdaterError> {
    use std::{mem::MaybeUninit, os::windows::io::AsRawHandle as _};
    use windows_sys::Win32::{
        Foundation::HANDLE,
        Storage::FileSystem::{BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle},
    };

    let file = File::open(path)
        .map_err(|_| transaction("failed to open transaction file for link inspection"))?;
    let mut information = MaybeUninit::<BY_HANDLE_FILE_INFORMATION>::uninit();
    let succeeded = unsafe {
        GetFileInformationByHandle(file.as_raw_handle() as HANDLE, information.as_mut_ptr())
    };
    if succeeded == 0 {
        return Err(transaction("failed to inspect transaction file links"));
    }
    let information = unsafe { information.assume_init() };
    Ok(information.nNumberOfLinks == 1)
}

#[cfg(unix)]
fn has_single_hard_link(_path: &Path, metadata: &fs::Metadata) -> Result<bool, UpdaterError> {
    use std::os::unix::fs::MetadataExt as _;

    Ok(metadata.nlink() == 1)
}

#[cfg(not(any(windows, unix)))]
fn has_single_hard_link(_path: &Path, _metadata: &fs::Metadata) -> Result<bool, UpdaterError> {
    Ok(true)
}

#[cfg(windows)]
fn is_reparse_point(metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt as _;

    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
    metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(windows))]
fn is_reparse_point(_metadata: &fs::Metadata) -> bool {
    false
}

#[cfg(windows)]
fn atomic_replace(source: &Path, destination: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt as _;
    use windows_sys::Win32::Storage::FileSystem::{
        MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
    };

    let source = source
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let destination = destination
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let succeeded = unsafe {
        MoveFileExW(
            source.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if succeeded == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(not(windows))]
fn atomic_replace(source: &Path, destination: &Path) -> io::Result<()> {
    fs::rename(source, destination)?;
    sync_parent(destination)
}

#[cfg(windows)]
fn sync_parent(_path: &Path) -> io::Result<()> {
    Ok(())
}

#[cfg(not(windows))]
fn sync_parent(path: &Path) -> io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "path has no parent"))?;
    File::open(parent)?.sync_all()
}

fn read_bounded(
    path: &Path,
    maximum: usize,
    detail: &'static str,
) -> Result<Vec<u8>, UpdaterError> {
    ensure_direct_file(path)?;
    let maximum_u64 = u64::try_from(maximum).expect("transaction read limit fits u64");
    let metadata = fs::metadata(path).map_err(|_| recovery(detail))?;
    if metadata.len() > maximum_u64 {
        return Err(recovery(detail));
    }
    let file = File::open(path).map_err(|_| recovery(detail))?;
    let mut bytes = Vec::with_capacity(
        usize::try_from(metadata.len()).expect("bounded transaction file length fits usize"),
    );
    file.take(maximum_u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| recovery(detail))?;
    if bytes.len() > maximum {
        return Err(recovery(detail));
    }
    Ok(bytes)
}

struct PreparationCleanup {
    root: PathBuf,
    armed: bool,
}

impl PreparationCleanup {
    fn new(root: PathBuf) -> Self {
        Self { root, armed: true }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for PreparationCleanup {
    fn drop(&mut self) {
        if self.armed {
            let _ = fs::remove_dir_all(&self.root);
        }
    }
}

fn transaction(detail: impl Into<String>) -> UpdaterError {
    UpdaterError::new(UpdaterErrorCode::Transaction, detail)
}

fn recovery(detail: impl Into<String>) -> UpdaterError {
    UpdaterError::new(UpdaterErrorCode::Recovery, detail)
}
