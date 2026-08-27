use std::{
    ffi::OsStr,
    fs, io,
    path::{Path, PathBuf},
    time::Duration,
};

use ah_update_helper::apply::{
    TransactionPaths, inspect_transaction, load_recovery_transaction, recover_transaction,
    remove_completed_transaction,
};
use ah_updater_core::{
    InstallationIdentityV1, ReleaseTrust, TransactionStateV1, UpdateOperation, UpdaterError,
};
use serde::Serialize;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use ah_error::AppError;

const MAX_IDENTITY_BYTES: usize = 16 * 1024;
const MAX_TRANSACTION_DIRECTORIES: usize = 8;
const RECOVERY_LOCK_TIMEOUT: Duration = Duration::from_secs(5);

/// What an invocation consumed by crash recovery did.
///
/// Recovery runs before argument parsing, and when it launches the helper the
/// command the user typed never runs - `ah git status` exits successfully having
/// done something else entirely. The behaviour is deliberate; its invisibility
/// was not, so the outcome is structured rather than a bare flag and the caller
/// both logs it and reports it.
///
/// Three fields, because three questions follow: *which* update was interrupted
/// (`transaction_id`), what it was doing (`operation`), and how far it got
/// (`state`). The last is the journal state recovery found on disk, which is the
/// point the interrupted run stopped at.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct RecoveryReport {
    pub transaction_id: Uuid,
    pub operation: UpdateOperation,
    pub state: TransactionStateV1,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EarlyRecoveryOutcome {
    /// Nothing was pending, or what was pending did not need this invocation.
    Continue,
    /// The helper was launched and this invocation is over. The report is what
    /// the caller has to make visible.
    RecoveryLaunched(RecoveryReport),
}

pub fn recover_before_startup(
    allow_safe_managed_serve: bool,
    guard: &dyn crate::service::ServiceGuard,
) -> Result<EarlyRecoveryOutcome, AppError> {
    #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
    {
        let executable = fs::canonicalize(
            std::env::current_exe()
                .map_err(|_| recovery_error("failed to resolve the running executable"))?,
        )
        .map_err(|_| recovery_error("failed to canonicalize the running executable"))?;
        let Some(updater_root) = crate::installation::updater_root() else {
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

        let hold = guard.hold(RECOVERY_LOCK_TIMEOUT)?;
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
                guard.stop(&hold)?;
            }
        }
        recover_pending_for(
            &executable,
            &updater_root,
            &trust,
            &ProcessRecoveryRunner(&hold),
        )
    }
    #[cfg(not(all(target_os = "windows", target_arch = "x86_64")))]
    {
        let _ = (allow_safe_managed_serve, guard);
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
struct ProcessRecoveryRunner<'a>(&'a crate::service::ServiceHold);

#[cfg(all(target_os = "windows", target_arch = "x86_64"))]
impl RecoveryHelperRunner for ProcessRecoveryRunner<'_> {
    fn launch(&self, helper: &Path, paths: &TransactionPaths) -> Result<HelperRun, AppError> {
        crate::handoff::launch_recovery(
            helper,
            &ah_updater_core::handoff_paths_arguments(
                OsStr::new("recover"),
                paths.installation_root().as_os_str(),
                paths.installation_state_root().as_os_str(),
                paths.transaction_root().as_os_str(),
            ),
            self.0,
        )
        .map_err(crate::handoff::LaunchFailure::into_error)?;
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
                HelperRun::Launched => Ok(EarlyRecoveryOutcome::RecoveryLaunched(RecoveryReport {
                    transaction_id: inspected.journal().transaction_id,
                    operation: inspected.plan().operation,
                    state: inspected.journal().state,
                })),
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

/// The shared bounded read also refuses a hard-linked file, which this path's
/// own copy of the check did not. That is the union the deduplication owed: a
/// hard-linked identity file is one a second name can rewrite while recovery is
/// reading it.
fn read_required_identity(path: &Path) -> Result<InstallationIdentityV1, AppError> {
    let bytes =
        ah_platform::fs::read_bounded(path, MAX_IDENTITY_BYTES).map_err(|reason| match reason {
            ah_platform::fs::BoundedRead::Redirected(_) => {
                recovery_error("updater identity file is not a direct regular file")
            }
            ah_platform::fs::BoundedRead::Inspect(_) => {
                recovery_error("failed to inspect updater identity file")
            }
            ah_platform::fs::BoundedRead::TooLarge => {
                recovery_error("updater identity file exceeds its limit")
            }
            ah_platform::fs::BoundedRead::Open(_) => {
                recovery_error("failed to open updater identity file")
            }
            ah_platform::fs::BoundedRead::Read(_) => {
                recovery_error("failed to read updater identity file")
            }
        })?;
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
    Ok(ah_updater_core::encode_digest(Sha256::digest(
        value.as_bytes(),
    )))
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

fn ensure_direct_directory_metadata(metadata: &fs::Metadata) -> Result<(), AppError> {
    ah_platform::fs::direct_directory_metadata(metadata)
        .map_err(|_| recovery_error("updater state contains a redirected directory"))
}

/// Recovery reports every failure as one code, with the original in the detail.
///
/// Deliberately not `super::map_updater_error`: the phase that consumed the
/// invocation is what a caller has to be able to see.
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

    /// The real Windows runner spawns a detached helper and returns, which is
    /// the path that consumes the invocation. The report it produces is what
    /// the caller logs and prints, so its three fields have to name the
    /// transaction actually found on disk rather than a default.
    #[test]
    fn a_detached_helper_reports_the_transaction_it_was_launched_for() {
        let fixture = Fixture::new();
        fixture.prepare();
        let expected = inspect_transaction(&fixture.paths, &fixture.trust).unwrap();
        let expected_id = expected.journal().transaction_id;
        let expected_operation = expected.plan().operation;
        let expected_state = expected.journal().state;
        let runner = DetachedRunner::default();

        let outcome = recover_pending_for(
            &fixture.executable,
            &fixture.updater_root,
            &fixture.trust,
            &runner,
        )
        .unwrap();

        assert_eq!(
            outcome,
            EarlyRecoveryOutcome::RecoveryLaunched(RecoveryReport {
                transaction_id: expected_id,
                operation: expected_operation,
                state: expected_state,
            })
        );
        assert_eq!(runner.launches.get(), 1);
        // A detached helper owns the transaction directory: the caller must not
        // clean up behind a process that is still using it.
        assert!(fixture.paths.transaction_root().exists());
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

    /// Launches and returns, the way the Windows runner does with a detached
    /// process. `InProcessRunner` above recovers inline instead, which is what
    /// makes it report `Continue`.
    #[derive(Default)]
    struct DetachedRunner {
        launches: Cell<usize>,
    }

    impl RecoveryHelperRunner for DetachedRunner {
        fn launch(&self, _helper: &Path, _paths: &TransactionPaths) -> Result<HelperRun, AppError> {
            self.launches.set(self.launches.get() + 1);
            Ok(HelperRun::Launched)
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
                sha256: ah_updater_core::encode_digest(Sha256::digest(bytes)),
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
