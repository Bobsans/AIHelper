use std::{
    ffi::OsStr,
    fmt::Write as _,
    fs::{self, File},
    io,
    path::{Path, PathBuf},
    time::Duration,
};

use ah_update_helper::apply::{
    TransactionPaths, inspect_transaction, load_recovery_transaction, recover_transaction,
    remove_completed_transaction,
};
use ah_updater_core::{InstallationIdentityV1, ReleaseTrust, TransactionStateV1, UpdaterError};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::error::AppError;

const MAX_IDENTITY_BYTES: usize = 16 * 1024;
const MAX_TRANSACTION_DIRECTORIES: usize = 8;
const RECOVERY_LOCK_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EarlyRecoveryOutcome {
    Continue,
    RecoveryLaunched,
}

pub(crate) fn recover_before_startup(
    allow_safe_managed_serve: bool,
) -> Result<EarlyRecoveryOutcome, AppError> {
    #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
    {
        let executable = fs::canonicalize(
            std::env::current_exe()
                .map_err(|_| recovery_error("failed to resolve the running executable"))?,
        )
        .map_err(|_| recovery_error("failed to canonicalize the running executable"))?;
        let Some(updater_root) = crate::updater::installation::updater_root() else {
            return Ok(EarlyRecoveryOutcome::Continue);
        };
        let Some(initial_paths) = discover_pending(&executable, &updater_root)? else {
            return Ok(EarlyRecoveryOutcome::Continue);
        };
        let trust = ah_updater_core::production_release_trust().map_err(map_updater_error)?;
        let initial = inspect_transaction(&initial_paths, &trust).map_err(map_updater_error)?;
        if allow_safe_managed_serve
            && matches!(
                initial.journal().state,
                TransactionStateV1::BackupPrepared
                    | TransactionStateV1::Committed
                    | TransactionStateV1::RolledBack
            )
        {
            recover_transaction(&initial_paths, &trust).map_err(map_updater_error)?;
            return Ok(EarlyRecoveryOutcome::Continue);
        }

        let service_paths = crate::mcp_service::paths::ServicePaths::discover()?;
        let lease = crate::mcp_service::lock::FileLease::acquire(
            &service_paths.lifecycle_lock,
            RECOVERY_LOCK_TIMEOUT,
        )?;
        if let Some(paths) = discover_pending(&executable, &updater_root)? {
            let inspected = inspect_transaction(&paths, &trust).map_err(map_updater_error)?;
            if matches!(
                inspected.journal().state,
                TransactionStateV1::ActivationStarted
                    | TransactionStateV1::CandidateActivated
                    | TransactionStateV1::PermanentVerified
                    | TransactionStateV1::CommitStarted
                    | TransactionStateV1::RollbackStarted
            ) {
                crate::mcp_service::lifecycle::stop_for_update_while_locked()?;
            }
        }
        recover_pending_for(
            &executable,
            &updater_root,
            &trust,
            &ProcessRecoveryRunner(&lease),
        )
    }
    #[cfg(not(all(target_os = "windows", target_arch = "x86_64")))]
    {
        let _ = allow_safe_managed_serve;
        Ok(EarlyRecoveryOutcome::Continue)
    }
}

trait RecoveryHelperRunner {
    fn launch(&self, helper: &Path, paths: &TransactionPaths) -> Result<HelperRun, AppError>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
enum HelperRun {
    Completed,
    Launched,
}

#[cfg(all(target_os = "windows", target_arch = "x86_64"))]
struct ProcessRecoveryRunner<'a>(&'a crate::mcp_service::lock::FileLease);

#[cfg(all(target_os = "windows", target_arch = "x86_64"))]
impl RecoveryHelperRunner for ProcessRecoveryRunner<'_> {
    fn launch(&self, helper: &Path, paths: &TransactionPaths) -> Result<HelperRun, AppError> {
        crate::updater::handoff::launch_recovery(
            helper,
            &[
                OsStr::new("recover"),
                OsStr::new("--installation-root"),
                paths.installation_root().as_os_str(),
                OsStr::new("--installation-state-root"),
                paths.installation_state_root().as_os_str(),
                OsStr::new("--transaction-root"),
                paths.transaction_root().as_os_str(),
            ],
            self.0,
        )
        .map_err(crate::updater::handoff::LaunchFailure::into_error)?;
        Ok(HelperRun::Launched)
    }
}

fn recover_pending_for(
    executable: &Path,
    updater_root: &Path,
    trust: &ReleaseTrust,
    runner: &impl RecoveryHelperRunner,
) -> Result<EarlyRecoveryOutcome, AppError> {
    let Some(paths) = discover_pending(executable, updater_root)? else {
        return Ok(EarlyRecoveryOutcome::Continue);
    };
    let inspected = inspect_transaction(&paths, trust).map_err(map_updater_error)?;
    match inspected.journal().state {
        TransactionStateV1::Planned => {
            remove_completed_transaction(&paths, trust).map_err(map_updater_error)?;
            Ok(EarlyRecoveryOutcome::Continue)
        }
        TransactionStateV1::BackupPrepared
        | TransactionStateV1::ActivationStarted
        | TransactionStateV1::CandidateActivated
        | TransactionStateV1::PermanentVerified
        | TransactionStateV1::CommitStarted
        | TransactionStateV1::Committed
        | TransactionStateV1::RollbackStarted
        | TransactionStateV1::RolledBack => {
            let transaction = if inspected.journal().state == TransactionStateV1::Committed
                && matches!(
                    inspected.plan().operation,
                    ah_updater_core::UpdateOperation::Upgrade
                        | ah_updater_core::UpdateOperation::Version
                ) {
                ah_update_helper::apply::load_transaction(&paths, trust)
            } else {
                load_recovery_transaction(&paths, trust)
            }
            .map_err(map_updater_error)?;
            let helper = recovery_helper_path(&transaction)?;
            match runner.launch(&helper, &paths)? {
                HelperRun::Launched => Ok(EarlyRecoveryOutcome::RecoveryLaunched),
                HelperRun::Completed => {
                    remove_completed_transaction(&paths, trust).map_err(map_updater_error)?;
                    Ok(EarlyRecoveryOutcome::Continue)
                }
            }
        }
    }
}

fn recovery_helper_path(
    transaction: &ah_update_helper::apply::LoadedTransaction,
) -> Result<PathBuf, AppError> {
    let use_candidate = transaction.journal().state == TransactionStateV1::Committed
        && matches!(
            transaction.plan().operation,
            ah_updater_core::UpdateOperation::Upgrade | ah_updater_core::UpdateOperation::Version
        );
    let manifest = if use_candidate {
        transaction.new_manifest()
    } else {
        transaction.old_manifest()
    };
    let root = if use_candidate {
        transaction.paths().candidate_files_root()
    } else {
        transaction.paths().backup_files_root()
    };
    #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
    {
        super::activate::cleanup_activation_helpers(transaction.paths().installation_state_root())
            .map_err(map_updater_error)?;
        super::activate::copy_activation_helper(
            &root,
            manifest,
            transaction.paths().installation_state_root(),
            Uuid::new_v4(),
        )
        .map_err(map_updater_error)
    }
    #[cfg(not(all(target_os = "windows", target_arch = "x86_64")))]
    {
        let helpers = manifest
            .files
            .iter()
            .filter(|file| file.purpose == ah_updater_core::FilePurpose::UpdateHelper)
            .collect::<Vec<_>>();
        let [helper] = helpers.as_slice() else {
            return Err(recovery_error(
                "transaction backup must contain exactly one update helper",
            ));
        };
        Ok(root.join(helper.path.split('/').collect::<PathBuf>()))
    }
}

fn discover_pending(
    executable: &Path,
    updater_root: &Path,
) -> Result<Option<TransactionPaths>, AppError> {
    match fs::symlink_metadata(updater_root) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Ok(metadata) => ensure_direct_directory_metadata(&metadata)?,
        Err(_) => return Err(recovery_error("failed to inspect updater state root")),
    }
    let binding = updater_root
        .join("bindings")
        .join(format!("{}.json", executable_binding_key(executable)?));
    let Some(identity) = read_optional_identity(&binding)? else {
        return Ok(None);
    };
    if !paths_equal(Path::new(&identity.executable_path), executable) {
        return Err(recovery_error(
            "updater binding belongs to another executable",
        ));
    }
    let state_root = updater_root
        .join("installations")
        .join(identity.installation_id.to_string());
    let stored = read_required_identity(&state_root.join("identity.json"))?;
    if stored != identity {
        return Err(recovery_error(
            "updater installation identities do not match",
        ));
    }
    let transactions = state_root.join("transactions");
    match fs::symlink_metadata(&transactions) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Ok(metadata) => ensure_direct_directory_metadata(&metadata)?,
        Err(_) => return Err(recovery_error("failed to inspect transaction directory")),
    }

    let mut roots = Vec::new();
    let entries = fs::read_dir(&transactions)
        .map_err(|_| recovery_error("failed to enumerate update transactions"))?;
    for entry in entries {
        let entry = entry.map_err(|_| recovery_error("failed to enumerate update transactions"))?;
        let metadata = fs::symlink_metadata(entry.path())
            .map_err(|_| recovery_error("failed to inspect update transaction"))?;
        ensure_direct_directory_metadata(&metadata)?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| recovery_error("transaction directory name is not valid Unicode"))?;
        Uuid::parse_str(&name)
            .map_err(|_| recovery_error("transaction directory name is not a UUID"))?;
        roots.push(entry.path());
        if roots.len() > MAX_TRANSACTION_DIRECTORIES {
            return Err(recovery_error(
                "update transaction directory count exceeds the limit",
            ));
        }
    }
    roots.sort();
    match roots.as_slice() {
        [] => Ok(None),
        [transaction_root] => Ok(Some(TransactionPaths::new(
            executable
                .parent()
                .ok_or_else(|| recovery_error("running executable has no installation root"))?,
            state_root,
            transaction_root,
        ))),
        _ => Err(recovery_error(
            "multiple update transactions require manual recovery",
        )),
    }
}

fn read_optional_identity(path: &Path) -> Result<Option<InstallationIdentityV1>, AppError> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Ok(_) => read_required_identity(path).map(Some),
        Err(_) => Err(recovery_error("failed to inspect updater binding")),
    }
}

fn read_required_identity(path: &Path) -> Result<InstallationIdentityV1, AppError> {
    ensure_direct_file(path)?;
    let metadata = fs::metadata(path)
        .map_err(|_| recovery_error("failed to inspect updater identity file"))?;
    if metadata.len() > MAX_IDENTITY_BYTES as u64 {
        return Err(recovery_error("updater identity file exceeds its limit"));
    }
    let file =
        File::open(path).map_err(|_| recovery_error("failed to open updater identity file"))?;
    let bytes = match super::installation::read_at_most(file, MAX_IDENTITY_BYTES)
        .map_err(|_| recovery_error("failed to read updater identity file"))?
    {
        Some(bytes) => bytes,
        None => return Err(recovery_error("updater identity file exceeds its limit")),
    };
    let identity: InstallationIdentityV1 = serde_json::from_slice(&bytes)
        .map_err(|_| recovery_error("updater identity file is malformed"))?;
    identity.validate().map_err(map_updater_error)?;
    Ok(identity)
}

fn executable_binding_key(executable: &Path) -> Result<String, AppError> {
    let value = executable
        .to_str()
        .ok_or_else(|| recovery_error("running executable path is not valid Unicode"))?
        .replace('/', "\\");
    #[cfg(windows)]
    let value = value.to_lowercase();
    Ok(encode_digest(Sha256::digest(value.as_bytes())))
}

fn encode_digest(bytes: impl AsRef<[u8]>) -> String {
    let bytes = bytes.as_ref();
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        write!(&mut encoded, "{byte:02x}").expect("writing to a String cannot fail");
    }
    encoded
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

fn ensure_direct_file(path: &Path) -> Result<(), AppError> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|_| recovery_error("failed to inspect updater state file"))?;
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || ah_platform::fs::is_reparse_point(&metadata)
    {
        return Err(recovery_error(
            "updater state file is not a direct regular file",
        ));
    }
    Ok(())
}

fn ensure_direct_directory_metadata(metadata: &fs::Metadata) -> Result<(), AppError> {
    if metadata.file_type().is_symlink()
        || !metadata.is_dir()
        || ah_platform::fs::is_reparse_point(metadata)
    {
        return Err(recovery_error(
            "updater state contains a redirected directory",
        ));
    }
    Ok(())
}

fn map_updater_error(error: UpdaterError) -> AppError {
    AppError::external(
        "UPDATER_RECOVERY",
        format!("{}: {}", error.code(), error.detail()),
    )
}

fn recovery_error(detail: impl Into<String>) -> AppError {
    AppError::external("UPDATER_RECOVERY", detail)
}

#[cfg(test)]
mod tests {
    use std::{
        cell::{Cell, RefCell},
        collections::BTreeMap,
    };

    use ah_release_manifest::{
        ArchiveMetadata, FilePurpose, ManagedFile, ReleaseManifest, ReleaseMetadata, RequiredFiles,
        SCHEMA_VERSION, SIGNING_ALGORITHM, SIGNING_DOMAIN, SignatureAlgorithm, SigningMetadata,
        TrustedKey, key_id_for_public_key,
    };
    use ah_update_helper::apply::{
        FailureInjector, FailurePoint, TransactionRunError, activate_transaction_with_injector,
        prepare_transaction,
    };
    use ah_updater_core::TransactionPlanV1;
    use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
    use ed25519_dalek::{Signer as _, SigningKey};
    use tempfile::TempDir;

    use super::*;

    #[test]
    fn launches_verified_private_helper_copy_and_cleans_recovered_transaction() {
        let fixture = Fixture::new();
        fixture.prepare();
        let mut interrupt = InterruptAfterFirstActivation;
        assert!(matches!(
            activate_transaction_with_injector(&fixture.paths, &fixture.trust, &mut interrupt),
            Err(TransactionRunError::Interrupted(_))
        ));
        assert_eq!(
            fs::read(fixture.installation_root.join("ah.exe")).unwrap(),
            b"new-ah"
        );
        let runner = InProcessRunner::new(fixture.trust.clone());

        let outcome = recover_pending_for(
            &fixture.executable,
            &fixture.updater_root,
            &fixture.trust,
            &runner,
        )
        .unwrap();

        assert_eq!(outcome, EarlyRecoveryOutcome::Continue);
        assert_eq!(runner.launches.get(), 1);
        let helper = runner.helper.borrow();
        let helper = helper.as_deref().unwrap();
        #[cfg(windows)]
        {
            assert_eq!(
                helper.parent(),
                Some(fixture.paths.installation_state_root())
            );
            assert!(
                helper
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with("activation-helper-")
            );
        }
        #[cfg(not(windows))]
        assert_eq!(
            helper,
            fixture
                .paths
                .backup_files_root()
                .join("ah-update-helper.exe")
        );
        assert!(!fixture.paths.transaction_root().exists());
        assert_eq!(
            fs::read(fixture.installation_root.join("ah.exe")).unwrap(),
            b"old-ah"
        );
        assert_eq!(
            fs::read(fixture.installation_root.join("user-owned.txt")).unwrap(),
            b"preserve me"
        );

        assert_eq!(
            recover_pending_for(
                &fixture.executable,
                &fixture.updater_root,
                &fixture.trust,
                &runner,
            )
            .unwrap(),
            EarlyRecoveryOutcome::Continue
        );
        assert_eq!(runner.launches.get(), 1);
    }

    #[test]
    fn prepared_transaction_runs_helper_before_cleanup() {
        let fixture = Fixture::new();
        fixture.prepare();
        let runner = InProcessRunner::new(fixture.trust.clone());

        let outcome = recover_pending_for(
            &fixture.executable,
            &fixture.updater_root,
            &fixture.trust,
            &runner,
        )
        .unwrap();

        assert_eq!(outcome, EarlyRecoveryOutcome::Continue);
        assert_eq!(runner.launches.get(), 1);
        assert!(!fixture.paths.transaction_root().exists());
        assert_eq!(
            fs::read(fixture.installation_root.join("ah.exe")).unwrap(),
            b"old-ah"
        );
    }

    struct InProcessRunner {
        trust: ReleaseTrust,
        launches: Cell<usize>,
        helper: RefCell<Option<PathBuf>>,
    }

    impl InProcessRunner {
        fn new(trust: ReleaseTrust) -> Self {
            Self {
                trust,
                launches: Cell::new(0),
                helper: RefCell::new(None),
            }
        }
    }

    impl RecoveryHelperRunner for InProcessRunner {
        fn launch(&self, helper: &Path, paths: &TransactionPaths) -> Result<HelperRun, AppError> {
            self.launches.set(self.launches.get() + 1);
            self.helper.replace(Some(helper.to_path_buf()));
            recover_transaction(paths, &self.trust).map_err(map_updater_error)?;
            Ok(HelperRun::Completed)
        }
    }

    struct InterruptAfterFirstActivation;

    impl FailureInjector for InterruptAfterFirstActivation {
        fn interrupt(&mut self, point: &FailurePoint) -> bool {
            matches!(
                point,
                FailurePoint::AfterFileMutation {
                    phase: ah_update_helper::apply::FileMutationPhase::ActivateManaged,
                    index: 0,
                    ..
                }
            )
        }
    }

    struct Fixture {
        _temporary: TempDir,
        updater_root: PathBuf,
        installation_root: PathBuf,
        executable: PathBuf,
        candidate_root: PathBuf,
        paths: TransactionPaths,
        plan: TransactionPlanV1,
        trust: ReleaseTrust,
        new_manifest_bytes: Vec<u8>,
        new_signature_bytes: Vec<u8>,
    }

    impl Fixture {
        fn new() -> Self {
            let temporary = TempDir::new().unwrap();
            let updater_root = temporary.path().join("updater");
            let installation_root = temporary.path().join("installation");
            let candidate_root = temporary.path().join("candidate");
            fs::create_dir_all(updater_root.join("bindings")).unwrap();
            fs::create_dir_all(&installation_root).unwrap();
            fs::create_dir_all(&candidate_root).unwrap();

            let old_files = BTreeMap::from([
                ("ah-update-helper.exe".to_owned(), b"helper".to_vec()),
                ("ah.exe".to_owned(), b"old-ah".to_vec()),
            ]);
            let new_files = BTreeMap::from([
                ("ah-update-helper.exe".to_owned(), b"helper".to_vec()),
                ("ah.exe".to_owned(), b"new-ah".to_vec()),
            ]);
            write_tree(&installation_root, &old_files);
            write_tree(&candidate_root, &new_files);
            fs::write(installation_root.join("user-owned.txt"), b"preserve me").unwrap();
            let executable = installation_root.join("ah.exe");

            let key = SigningKey::from_bytes(&[31_u8; 32]);
            let trusted = trusted_key(&key);
            let trust = ReleaseTrust::from_keys(vec![trusted.clone()]).unwrap();
            let (old_manifest, old_manifest_bytes, old_signature_bytes) =
                signed_manifest("1.1.0", &old_files, &trusted.key_id, &key);
            let (new_manifest, new_manifest_bytes, new_signature_bytes) =
                signed_manifest("1.2.0", &new_files, &trusted.key_id, &key);
            let installation_id = Uuid::new_v4();
            let transaction_id = Uuid::new_v4();
            let identity = InstallationIdentityV1::new(
                installation_id,
                executable.to_str().unwrap().to_owned(),
            )
            .unwrap();
            let state_root = updater_root
                .join("installations")
                .join(installation_id.to_string());
            fs::create_dir_all(state_root.join("transactions")).unwrap();
            let mut identity_bytes = serde_json::to_vec_pretty(&identity).unwrap();
            identity_bytes.push(b'\n');
            fs::write(state_root.join("identity.json"), &identity_bytes).unwrap();
            fs::write(
                state_root.join("installed.manifest.json"),
                old_manifest_bytes,
            )
            .unwrap();
            fs::write(
                state_root.join("installed.manifest.sig"),
                old_signature_bytes,
            )
            .unwrap();
            fs::write(
                updater_root.join("bindings").join(format!(
                    "{}.json",
                    executable_binding_key(&executable).unwrap()
                )),
                &identity_bytes,
            )
            .unwrap();
            let plan = TransactionPlanV1::build(
                transaction_id,
                installation_id,
                &old_manifest,
                &new_manifest,
            )
            .unwrap();
            let paths = TransactionPaths::new(
                &installation_root,
                &state_root,
                state_root
                    .join("transactions")
                    .join(transaction_id.to_string()),
            );
            Self {
                _temporary: temporary,
                updater_root,
                installation_root,
                executable,
                candidate_root,
                paths,
                plan,
                trust,
                new_manifest_bytes,
                new_signature_bytes,
            }
        }

        fn prepare(&self) {
            prepare_transaction(
                &self.paths,
                &self.plan,
                &self.candidate_root,
                &self.new_manifest_bytes,
                &self.new_signature_bytes,
                &self.trust,
            )
            .unwrap();
        }
    }

    fn signed_manifest(
        version: &str,
        files: &BTreeMap<String, Vec<u8>>,
        key_id: &str,
        key: &SigningKey,
    ) -> (ReleaseManifest, Vec<u8>, Vec<u8>) {
        let managed = files
            .iter()
            .map(|(path, bytes)| ManagedFile {
                path: path.clone(),
                size: bytes.len() as u64,
                sha256: encode_digest(Sha256::digest(bytes)),
                purpose: if path == "ah-update-helper.exe" {
                    FilePurpose::UpdateHelper
                } else {
                    FilePurpose::Executable
                },
            })
            .collect::<Vec<_>>();
        let manifest = ReleaseManifest {
            schema_version: SCHEMA_VERSION,
            release: ReleaseMetadata {
                version: version.to_owned(),
                target: "x86_64-pc-windows-msvc".to_owned(),
                architecture: "x86_64".to_owned(),
            },
            archive: ArchiveMetadata {
                url: format!("https://example.test/v{version}/ah-windows-x64.zip"),
                size: 1,
                sha256: "0".repeat(64),
            },
            minimum_updater_version: "1.0.0".to_owned(),
            signing: SigningMetadata {
                key_id: key_id.to_owned(),
                algorithm: SignatureAlgorithm::Ed25519,
            },
            files: managed,
            required: RequiredFiles {
                executables: vec!["ah-update-helper.exe".to_owned(), "ah.exe".to_owned()],
                plugins: Vec::new(),
            },
        };
        let bytes = manifest.to_canonical_bytes().unwrap();
        let mut preimage = Vec::from(SIGNING_DOMAIN);
        preimage.extend_from_slice(&bytes);
        let signature = URL_SAFE_NO_PAD
            .encode(key.sign(&preimage).to_bytes())
            .into_bytes();
        (manifest, bytes, signature)
    }

    fn trusted_key(key: &SigningKey) -> TrustedKey {
        let public_key = key.verifying_key().to_bytes();
        TrustedKey {
            key_id: key_id_for_public_key(&public_key),
            algorithm: SIGNING_ALGORITHM.to_owned(),
            public_key,
        }
    }

    fn write_tree(root: &Path, files: &BTreeMap<String, Vec<u8>>) {
        for (relative, bytes) in files {
            fs::write(root.join(relative), bytes).unwrap();
        }
    }
}
