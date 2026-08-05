use std::{
    fmt::Write as _,
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    path::{Component, Path, PathBuf},
};

use ah_updater_core::{
    InstallationIdentityV1, ManagedFile, ManagedFileOperationV1, ReleaseManifest, ReleaseTrust,
    TransactionJournalV1, TransactionPlanV1, TransactionStateV1, UpdateOperation, UpdaterError,
    UpdaterErrorCode,
};
use sha2::{Digest, Sha256};
use uuid::Uuid;

const IDENTITY_FILE: &str = "identity.json";
const INSTALLED_MANIFEST_FILE: &str = "installed.manifest.json";
const INSTALLED_SIGNATURE_FILE: &str = "installed.manifest.sig";
const PLAN_FILE: &str = "plan.json";
const JOURNAL_FILE: &str = "journal.json";
const CANDIDATE_DIRECTORY: &str = "candidate";
const BACKUP_DIRECTORY: &str = "backup";
const FILES_DIRECTORY: &str = "files";
const PERMANENT_BACKUP_DIRECTORY: &str = "backup";
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

#[derive(Debug, Clone)]
pub struct PermanentBackup {
    root: PathBuf,
    identity: InstallationIdentityV1,
    manifest: ReleaseManifest,
    manifest_bytes: Vec<u8>,
    signature_bytes: Vec<u8>,
}

impl PermanentBackup {
    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn files_root(&self) -> PathBuf {
        self.root.join(FILES_DIRECTORY)
    }

    pub fn identity(&self) -> &InstallationIdentityV1 {
        &self.identity
    }

    pub fn manifest(&self) -> &ReleaseManifest {
        &self.manifest
    }

    pub fn manifest_bytes(&self) -> &[u8] {
        &self.manifest_bytes
    }

    pub fn signature_bytes(&self) -> &[u8] {
        &self.signature_bytes
    }
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileMutationPhase {
    StageCandidate,
    BackupManaged,
    ActivateManaged,
    PublishInstalledRecord,
    RollbackManaged,
    RestoreInstalledRecord,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FailurePoint {
    BeforeJournalWrite {
        state: TransactionStateV1,
        sequence: u32,
    },
    AfterJournalWrite {
        state: TransactionStateV1,
        sequence: u32,
    },
    BeforeFileMutation {
        phase: FileMutationPhase,
        index: usize,
        relative_path: String,
    },
    AfterFileMutation {
        phase: FileMutationPhase,
        index: usize,
        relative_path: String,
    },
}

pub trait FailureInjector {
    fn interrupt(&mut self, point: &FailurePoint) -> bool;
}

#[derive(Debug, Clone, Copy, Default)]
pub struct NoFailureInjection;

impl FailureInjector for NoFailureInjection {
    fn interrupt(&mut self, _point: &FailurePoint) -> bool {
        false
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransactionRunError {
    Interrupted(FailurePoint),
    Failed(UpdaterError),
}

impl TransactionRunError {
    pub fn into_updater_error(self) -> UpdaterError {
        match self {
            Self::Interrupted(_) => recovery("update transaction was interrupted"),
            Self::Failed(error) => error,
        }
    }
}

impl From<UpdaterError> for TransactionRunError {
    fn from(error: UpdaterError) -> Self {
        Self::Failed(error)
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
    let mut injector = NoFailureInjection;
    prepare_transaction_with_injector(
        paths,
        plan,
        candidate_source_root,
        candidate_manifest_bytes,
        candidate_signature_bytes,
        trust,
        &mut injector,
    )
    .map_err(TransactionRunError::into_updater_error)
}

pub fn prepare_transaction_with_injector(
    paths: &TransactionPaths,
    plan: &TransactionPlanV1,
    candidate_source_root: &Path,
    candidate_manifest_bytes: &[u8],
    candidate_signature_bytes: &[u8],
    trust: &ReleaseTrust,
    injector: &mut impl FailureInjector,
) -> Result<LoadedTransaction, TransactionRunError> {
    validate_prepare_paths(paths, candidate_source_root)?;
    plan.validate()?;
    validate_transaction_location(paths, plan)?;

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
    let result = (|| {
        copy_manifest_files_with_injector(
            injector,
            FileMutationPhase::StageCandidate,
            candidate_source_root,
            &paths.candidate_files_root(),
            &candidate.manifest,
        )?;
        copy_manifest_files_with_injector(
            injector,
            FileMutationPhase::BackupManaged,
            &paths.installation_root,
            &paths.backup_files_root(),
            &active.manifest,
        )?;
        verify_manifest_files(&paths.candidate_files_root(), &candidate.manifest)?;
        verify_manifest_files(&paths.backup_files_root(), &active.manifest)?;

        advance_journal_with(
            paths,
            &mut journal,
            TransactionStateV1::BackupPrepared,
            injector,
        )?;
        load_transaction(paths, trust).map_err(Into::into)
    })();
    if result.is_ok() || matches!(result, Err(TransactionRunError::Interrupted(_))) {
        cleanup.disarm();
    }
    result
}

pub fn load_transaction(
    paths: &TransactionPaths,
    trust: &ReleaseTrust,
) -> Result<LoadedTransaction, UpdaterError> {
    let transaction = load_transaction_metadata(paths, trust)?;
    verify_manifest_files(
        &transaction.paths.backup_files_root(),
        &transaction.old_manifest,
    )?;
    verify_manifest_files(
        &transaction.paths.candidate_files_root(),
        &transaction.new_manifest,
    )?;
    Ok(transaction)
}

pub fn inspect_transaction(
    paths: &TransactionPaths,
    trust: &ReleaseTrust,
) -> Result<LoadedTransaction, UpdaterError> {
    load_transaction_metadata(paths, trust)
}

pub fn load_recovery_transaction(
    paths: &TransactionPaths,
    trust: &ReleaseTrust,
) -> Result<LoadedTransaction, UpdaterError> {
    let transaction = load_transaction_metadata(paths, trust)?;
    verify_manifest_files(
        &transaction.paths.backup_files_root(),
        &transaction.old_manifest,
    )?;
    Ok(transaction)
}

fn load_transaction_metadata(
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
    validate_transaction_location(paths, &plan)?;

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
    verify_add_destinations_absent(
        &paths.installation_root,
        &transaction.plan.operations,
        &transaction.new_manifest,
    )?;
    Ok(transaction)
}

pub fn activate_transaction(
    paths: &TransactionPaths,
    trust: &ReleaseTrust,
) -> Result<LoadedTransaction, UpdaterError> {
    let mut injector = NoFailureInjection;
    match activate_transaction_with_injector(paths, trust, &mut injector) {
        Ok(transaction) => Ok(transaction),
        Err(TransactionRunError::Interrupted(_)) => {
            unreachable!("NoFailureInjection never interrupts")
        }
        Err(TransactionRunError::Failed(activation_error)) => {
            let rollback_required = load_transaction_metadata(paths, trust)
                .map(|transaction| state_requires_rollback(transaction.journal.state))
                .unwrap_or(false);
            if rollback_required && rollback_transaction(paths, trust).is_err() {
                return Err(UpdaterError::new(
                    UpdaterErrorCode::Rollback,
                    "activation failed and automatic rollback did not complete",
                ));
            }
            Err(activation_error)
        }
    }
}

pub fn activate_transaction_with_injector(
    paths: &TransactionPaths,
    trust: &ReleaseTrust,
    injector: &mut impl FailureInjector,
) -> Result<LoadedTransaction, TransactionRunError> {
    let transaction = load_prepared_transaction(paths, trust)?;
    let mut journal = transaction.journal.clone();
    advance_journal_with(
        paths,
        &mut journal,
        TransactionStateV1::ActivationStarted,
        injector,
    )?;
    for (index, operation) in transaction.plan.operations.iter().enumerate() {
        mutate_with_checkpoint(
            injector,
            FileMutationPhase::ActivateManaged,
            index,
            operation.path(),
            || apply_activation_operation(&transaction, operation, index),
        )?;
    }
    verify_manifest_files(&paths.installation_root, &transaction.new_manifest)?;
    advance_journal_with(
        paths,
        &mut journal,
        TransactionStateV1::CandidateActivated,
        injector,
    )?;
    publish_candidate_record(&transaction, injector)?;
    verify_active_new_release(&transaction, trust)?;
    advance_journal_with(
        paths,
        &mut journal,
        TransactionStateV1::PermanentVerified,
        injector,
    )?;
    load_transaction(paths, trust).map_err(Into::into)
}

pub fn commit_transaction(
    paths: &TransactionPaths,
    trust: &ReleaseTrust,
) -> Result<LoadedTransaction, UpdaterError> {
    let mut injector = NoFailureInjection;
    commit_transaction_with_injector(paths, trust, &mut injector)
        .map_err(TransactionRunError::into_updater_error)
}

pub fn commit_transaction_with_injector(
    paths: &TransactionPaths,
    trust: &ReleaseTrust,
    injector: &mut impl FailureInjector,
) -> Result<LoadedTransaction, TransactionRunError> {
    let transaction = load_transaction(paths, trust)?;
    if transaction.journal.state != TransactionStateV1::PermanentVerified {
        return Err(transaction_error(
            "durable transaction is not ready to commit",
        ));
    }
    verify_active_new_release(&transaction, trust)?;
    let mut journal = transaction.journal.clone();
    advance_journal_with(
        paths,
        &mut journal,
        TransactionStateV1::CommitStarted,
        injector,
    )?;
    advance_journal_with(paths, &mut journal, TransactionStateV1::Committed, injector)?;
    load_transaction(paths, trust).map_err(Into::into)
}

pub fn rollback_transaction(
    paths: &TransactionPaths,
    trust: &ReleaseTrust,
) -> Result<LoadedTransaction, UpdaterError> {
    let mut injector = NoFailureInjection;
    rollback_transaction_with_injector(paths, trust, &mut injector)
        .map_err(TransactionRunError::into_updater_error)
}

pub fn rollback_transaction_with_injector(
    paths: &TransactionPaths,
    trust: &ReleaseTrust,
    injector: &mut impl FailureInjector,
) -> Result<LoadedTransaction, TransactionRunError> {
    let transaction = load_transaction_metadata(paths, trust)?;
    verify_manifest_files(
        &transaction.paths.backup_files_root(),
        &transaction.old_manifest,
    )?;
    if !state_requires_rollback(transaction.journal.state)
        && transaction.journal.state != TransactionStateV1::RollbackStarted
    {
        return Err(rollback_error(
            "durable transaction state cannot enter rollback",
        ));
    }
    let mut journal = transaction.journal.clone();
    if journal.state != TransactionStateV1::RollbackStarted {
        advance_journal_with(
            paths,
            &mut journal,
            TransactionStateV1::RollbackStarted,
            injector,
        )?;
    }
    for (index, operation) in transaction.plan.operations.iter().enumerate().rev() {
        mutate_with_checkpoint(
            injector,
            FileMutationPhase::RollbackManaged,
            index,
            operation.path(),
            || apply_rollback_operation(&transaction, operation, index),
        )?;
    }
    restore_backup_record(&transaction, injector)?;
    verify_active_old_release(&transaction, trust)?;
    advance_journal_with(
        paths,
        &mut journal,
        TransactionStateV1::RolledBack,
        injector,
    )?;
    load_transaction_metadata(paths, trust).map_err(Into::into)
}

pub fn recover_transaction(
    paths: &TransactionPaths,
    trust: &ReleaseTrust,
) -> Result<LoadedTransaction, UpdaterError> {
    let transaction = load_transaction_metadata(paths, trust)?;
    match transaction.journal.state {
        TransactionStateV1::Planned | TransactionStateV1::BackupPrepared => {
            verify_active_old_release(&transaction, trust)?;
            Ok(transaction)
        }
        TransactionStateV1::ActivationStarted
        | TransactionStateV1::CandidateActivated
        | TransactionStateV1::PermanentVerified
        | TransactionStateV1::CommitStarted
        | TransactionStateV1::RollbackStarted => rollback_transaction(paths, trust),
        TransactionStateV1::Committed => {
            verify_active_new_release(&transaction, trust)?;
            Ok(transaction)
        }
        TransactionStateV1::RolledBack => {
            verify_active_old_release(&transaction, trust)?;
            Ok(transaction)
        }
    }
}

pub fn remove_completed_transaction(
    paths: &TransactionPaths,
    trust: &ReleaseTrust,
) -> Result<TransactionStateV1, UpdaterError> {
    let transaction = load_transaction_metadata(paths, trust)?;
    match transaction.journal.state {
        TransactionStateV1::Planned => {
            verify_active_old_release(&transaction, trust)?;
        }
        TransactionStateV1::BackupPrepared | TransactionStateV1::RolledBack => {
            verify_manifest_files(&paths.backup_files_root(), &transaction.old_manifest)?;
            verify_active_old_release(&transaction, trust)?;
        }
        TransactionStateV1::Committed => {
            verify_active_new_release(&transaction, trust)?;
        }
        TransactionStateV1::ActivationStarted
        | TransactionStateV1::CandidateActivated
        | TransactionStateV1::PermanentVerified
        | TransactionStateV1::CommitStarted
        | TransactionStateV1::RollbackStarted => {
            return Err(recovery(
                "unfinished transaction cannot be removed before recovery",
            ));
        }
    }
    let state = transaction.journal.state;
    fs::remove_dir_all(&paths.transaction_root)
        .map_err(|_| recovery("failed to remove completed private transaction state"))?;
    sync_parent(&paths.transaction_root)
        .map_err(|_| recovery("failed to sync completed transaction cleanup"))?;
    Ok(state)
}

pub fn load_permanent_backup(
    installation_root: &Path,
    installation_state_root: &Path,
    installation_id: Uuid,
    trust: &ReleaseTrust,
) -> Result<PermanentBackup, UpdaterError> {
    let root = permanent_backup_parent(installation_state_root, installation_id)?
        .join(installation_id.to_string());
    load_permanent_backup_at(&root, installation_root, installation_id, trust)
}

pub fn finalize_completed_transaction(
    paths: &TransactionPaths,
    trust: &ReleaseTrust,
) -> Result<TransactionStateV1, UpdaterError> {
    let transaction = load_transaction(paths, trust)?;
    match transaction.journal.state {
        TransactionStateV1::Committed => match transaction.plan.operation {
            UpdateOperation::Upgrade | UpdateOperation::Version => {
                promote_permanent_backup(&transaction, trust)?;
            }
            UpdateOperation::Rollback => {
                remove_permanent_backup(&transaction, trust)?;
            }
            UpdateOperation::Check | UpdateOperation::Recovery => {
                return Err(recovery("completed transaction operation is invalid"));
            }
        },
        TransactionStateV1::BackupPrepared | TransactionStateV1::RolledBack => {}
        _ => {
            return Err(recovery(
                "transaction cannot be finalized before reaching a safe terminal state",
            ));
        }
    }
    remove_completed_transaction(paths, trust)
}

fn promote_permanent_backup(
    transaction: &LoadedTransaction,
    trust: &ReleaseTrust,
) -> Result<(), UpdaterError> {
    let installation_id = transaction.plan.installation_id;
    let parent =
        permanent_backup_parent(transaction.paths.installation_state_root(), installation_id)?;
    ensure_directory_exists(&parent)?;
    let active = parent.join(installation_id.to_string());
    let staged = parent.join(format!("{installation_id}.new"));
    let previous = parent.join(format!("{installation_id}.old"));

    if path_exists(&active)? && permanent_backup_matches(&active, transaction, trust)? {
        remove_optional_known_directory(&staged)?;
        remove_optional_known_directory(&previous)?;
        return Ok(());
    }

    if path_exists(&previous)? {
        if path_exists(&active)? || !path_exists(&staged)? {
            return Err(recovery("permanent backup rotation state is ambiguous"));
        }
        fs::rename(&staged, &active)
            .map_err(|_| recovery("failed to complete permanent backup rotation"))?;
        sync_parent(&active).map_err(|_| recovery("failed to sync permanent backup rotation"))?;
        remove_known_directory(&previous)?;
        return verify_promoted_backup(&active, transaction, trust);
    }

    prepare_permanent_backup_stage(&staged, transaction, trust)?;
    if path_exists(&active)? {
        load_permanent_backup_at(
            &active,
            transaction.paths.installation_root(),
            installation_id,
            trust,
        )?;
        fs::rename(&active, &previous)
            .map_err(|_| recovery("failed to preserve the previous permanent backup"))?;
        sync_parent(&previous).map_err(|_| recovery("failed to sync permanent backup rotation"))?;
    }
    if let Err(error) = fs::rename(&staged, &active) {
        if path_exists(&previous)? {
            let _ = fs::rename(&previous, &active);
            let _ = sync_parent(&active);
        }
        let _ = error;
        return Err(recovery("failed to publish the new permanent backup"));
    }
    sync_parent(&active).map_err(|_| recovery("failed to sync the new permanent backup"))?;
    verify_promoted_backup(&active, transaction, trust)?;
    remove_optional_known_directory(&previous)
}

fn prepare_permanent_backup_stage(
    staged: &Path,
    transaction: &LoadedTransaction,
    trust: &ReleaseTrust,
) -> Result<(), UpdaterError> {
    if path_exists(staged)? {
        if permanent_backup_matches(staged, transaction, trust).unwrap_or(false) {
            return Ok(());
        }
        remove_known_directory(staged)?;
    }
    create_synced_directory(staged)?;
    create_synced_directory(&staged.join(FILES_DIRECTORY))?;
    let source = transaction.paths.backup_root();
    for name in [
        IDENTITY_FILE,
        INSTALLED_MANIFEST_FILE,
        INSTALLED_SIGNATURE_FILE,
    ] {
        let limit = match name {
            IDENTITY_FILE => MAX_IDENTITY_BYTES,
            INSTALLED_MANIFEST_FILE => ah_updater_core::MAX_MANIFEST_BYTES,
            INSTALLED_SIGNATURE_FILE => ah_updater_core::DETACHED_SIGNATURE_BYTES,
            _ => unreachable!(),
        };
        let bytes = read_bounded(
            &source.join(name),
            limit,
            "failed to read transaction backup",
        )?;
        write_new_synced(&staged.join(name), &bytes)?;
    }
    let mut injector = NoFailureInjection;
    copy_manifest_files_with_injector(
        &mut injector,
        FileMutationPhase::BackupManaged,
        &transaction.paths.backup_files_root(),
        &staged.join(FILES_DIRECTORY),
        &transaction.old_manifest,
    )
    .map_err(TransactionRunError::into_updater_error)?;
    verify_promoted_backup(staged, transaction, trust)
}

fn verify_promoted_backup(
    root: &Path,
    transaction: &LoadedTransaction,
    trust: &ReleaseTrust,
) -> Result<(), UpdaterError> {
    let backup = load_permanent_backup_at(
        root,
        transaction.paths.installation_root(),
        transaction.plan.installation_id,
        trust,
    )?;
    if backup.identity != transaction.identity || backup.manifest != transaction.old_manifest {
        return Err(recovery(
            "permanent backup does not match the committed transaction",
        ));
    }
    Ok(())
}

fn permanent_backup_matches(
    root: &Path,
    transaction: &LoadedTransaction,
    trust: &ReleaseTrust,
) -> Result<bool, UpdaterError> {
    let backup = load_permanent_backup_at(
        root,
        transaction.paths.installation_root(),
        transaction.plan.installation_id,
        trust,
    )?;
    Ok(backup.identity == transaction.identity && backup.manifest == transaction.old_manifest)
}

fn remove_permanent_backup(
    transaction: &LoadedTransaction,
    trust: &ReleaseTrust,
) -> Result<(), UpdaterError> {
    let root = permanent_backup_parent(
        transaction.paths.installation_state_root(),
        transaction.plan.installation_id,
    )?
    .join(transaction.plan.installation_id.to_string());
    if !path_exists(&root)? {
        return Ok(());
    }
    let backup = load_permanent_backup_at(
        &root,
        transaction.paths.installation_root(),
        transaction.plan.installation_id,
        trust,
    )?;
    if backup.manifest != transaction.new_manifest {
        return Err(recovery(
            "consumed permanent backup does not match the rollback target",
        ));
    }
    remove_known_directory(backup.root())
}

fn load_permanent_backup_at(
    root: &Path,
    installation_root: &Path,
    installation_id: Uuid,
    trust: &ReleaseTrust,
) -> Result<PermanentBackup, UpdaterError> {
    ensure_direct_directory(root)?;
    ensure_direct_directory(&root.join(FILES_DIRECTORY))?;
    let identity_bytes = read_bounded(
        &root.join(IDENTITY_FILE),
        MAX_IDENTITY_BYTES,
        "failed to read permanent backup identity",
    )?;
    let identity = decode_identity(&identity_bytes)?;
    let record = read_release_record(root, trust)?;
    verify_manifest_files(&root.join(FILES_DIRECTORY), &record.manifest)?;
    if identity.installation_id != installation_id {
        return Err(recovery("permanent backup belongs to another installation"));
    }
    let executable = Path::new(&identity.executable_path);
    if !record.manifest.files.iter().any(|file| {
        paths_equal(
            executable,
            &installation_root.join(path_from_slashes(&file.path)),
        )
    }) {
        return Err(recovery(
            "permanent backup identity does not match the installation",
        ));
    }
    Ok(PermanentBackup {
        root: root.to_path_buf(),
        identity,
        manifest: record.manifest,
        manifest_bytes: record.manifest_bytes,
        signature_bytes: record.signature_bytes,
    })
}

fn permanent_backup_parent(
    installation_state_root: &Path,
    installation_id: Uuid,
) -> Result<PathBuf, UpdaterError> {
    if installation_state_root
        .file_name()
        .and_then(|name| name.to_str())
        != Some(installation_id.to_string().as_str())
    {
        return Err(recovery("installation state root identity is invalid"));
    }
    let installations = installation_state_root
        .parent()
        .ok_or_else(|| recovery("installation state root has no parent"))?;
    let updater = installations
        .parent()
        .ok_or_else(|| recovery("installation state root is incomplete"))?;
    let product = updater
        .parent()
        .ok_or_else(|| recovery("installation state root is incomplete"))?;
    if installations.file_name().and_then(|name| name.to_str()) != Some("installations")
        || updater.file_name().and_then(|name| name.to_str()) != Some("updater")
    {
        return Err(recovery("installation state root layout is invalid"));
    }
    for directory in [product, updater, installations, installation_state_root] {
        ensure_direct_directory(directory)?;
    }
    Ok(product.join(PERMANENT_BACKUP_DIRECTORY))
}

fn ensure_directory_exists(path: &Path) -> Result<(), UpdaterError> {
    match fs::symlink_metadata(path) {
        Ok(_) => ensure_direct_directory(path),
        Err(error) if error.kind() == io::ErrorKind::NotFound => create_synced_directory(path),
        Err(_) => Err(recovery("failed to inspect permanent backup directory")),
    }
}

fn path_exists(path: &Path) -> Result<bool, UpdaterError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            ensure_direct_directory_metadata(&metadata)?;
            Ok(true)
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(_) => Err(recovery("failed to inspect permanent backup state")),
    }
}

fn remove_optional_known_directory(path: &Path) -> Result<(), UpdaterError> {
    if path_exists(path)? {
        remove_known_directory(path)?;
    }
    Ok(())
}

fn remove_known_directory(path: &Path) -> Result<(), UpdaterError> {
    ensure_direct_directory(path)?;
    fs::remove_dir_all(path).map_err(|_| recovery("failed to remove permanent backup state"))?;
    sync_parent(path).map_err(|_| recovery("failed to sync permanent backup cleanup"))
}

fn state_requires_rollback(state: TransactionStateV1) -> bool {
    matches!(
        state,
        TransactionStateV1::ActivationStarted
            | TransactionStateV1::CandidateActivated
            | TransactionStateV1::PermanentVerified
            | TransactionStateV1::CommitStarted
    )
}

fn verify_active_old_release(
    transaction: &LoadedTransaction,
    trust: &ReleaseTrust,
) -> Result<(), UpdaterError> {
    let paths = &transaction.paths;
    let active = read_release_record(&paths.installation_state_root, trust)?;
    let backup = read_release_record(&paths.backup_root(), trust)?;
    if active.manifest_bytes != backup.manifest_bytes
        || active.signature_bytes != backup.signature_bytes
    {
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
    verify_manifest_files(&paths.installation_root, &transaction.old_manifest)
}

fn verify_active_new_release(
    transaction: &LoadedTransaction,
    trust: &ReleaseTrust,
) -> Result<(), UpdaterError> {
    let paths = &transaction.paths;
    let active = read_release_record(&paths.installation_state_root, trust)?;
    let candidate = read_release_record(&paths.candidate_root(), trust)?;
    if active.manifest_bytes != candidate.manifest_bytes
        || active.signature_bytes != candidate.signature_bytes
    {
        return Err(recovery(
            "active release record does not match the transaction candidate",
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
    verify_manifest_files(&paths.installation_root, &transaction.new_manifest)
}

fn advance_journal_with(
    paths: &TransactionPaths,
    journal: &mut TransactionJournalV1,
    next: TransactionStateV1,
    injector: &mut impl FailureInjector,
) -> Result<(), TransactionRunError> {
    let sequence = journal
        .transition_sequence
        .checked_add(1)
        .ok_or_else(|| transaction("transaction transition sequence overflow"))?;
    checkpoint(
        injector,
        FailurePoint::BeforeJournalWrite {
            state: next,
            sequence,
        },
    )?;
    journal.advance(next)?;
    replace_journal(paths, journal)?;
    checkpoint(
        injector,
        FailurePoint::AfterJournalWrite {
            state: next,
            sequence,
        },
    )
}

fn mutate_with_checkpoint(
    injector: &mut impl FailureInjector,
    phase: FileMutationPhase,
    index: usize,
    relative_path: &str,
    mutate: impl FnOnce() -> Result<(), UpdaterError>,
) -> Result<(), TransactionRunError> {
    checkpoint(
        injector,
        FailurePoint::BeforeFileMutation {
            phase,
            index,
            relative_path: relative_path.to_owned(),
        },
    )?;
    mutate()?;
    checkpoint(
        injector,
        FailurePoint::AfterFileMutation {
            phase,
            index,
            relative_path: relative_path.to_owned(),
        },
    )
}

fn checkpoint(
    injector: &mut impl FailureInjector,
    point: FailurePoint,
) -> Result<(), TransactionRunError> {
    if injector.interrupt(&point) {
        Err(TransactionRunError::Interrupted(point))
    } else {
        Ok(())
    }
}

fn apply_activation_operation(
    transaction: &LoadedTransaction,
    operation: &ManagedFileOperationV1,
    index: usize,
) -> Result<(), UpdaterError> {
    let root = &transaction.paths.installation_root;
    match operation {
        ManagedFileOperationV1::Add { new } => {
            let destination = optional_managed_file(root, &new.path)?;
            if let Some(destination) = destination {
                if file_matches(&destination, new)? {
                    return Ok(());
                }
                return Err(activation(
                    "transaction add destination contains unknown file content",
                ));
            }
            install_managed_file(
                transaction,
                &transaction.paths.candidate_files_root(),
                new,
                FileMutationPhase::ActivateManaged,
                index,
            )
        }
        ManagedFileOperationV1::Replace { old, new } => {
            let destination = optional_managed_file(root, &new.path)?
                .ok_or_else(|| activation("transaction replacement destination is missing"))?;
            if file_matches(&destination, new)? {
                return Ok(());
            }
            if !file_matches(&destination, old)? {
                return Err(activation(
                    "transaction replacement destination contains unknown file content",
                ));
            }
            install_managed_file(
                transaction,
                &transaction.paths.candidate_files_root(),
                new,
                FileMutationPhase::ActivateManaged,
                index,
            )
        }
        ManagedFileOperationV1::Remove { old } => {
            let Some(destination) = optional_managed_file(root, &old.path)? else {
                return Ok(());
            };
            if !file_matches(&destination, old)? {
                return Err(activation(
                    "transaction removal destination contains unknown file content",
                ));
            }
            remove_file_synced(&destination)
                .map_err(|_| activation("failed to remove obsolete managed file"))
        }
    }
}

fn apply_rollback_operation(
    transaction: &LoadedTransaction,
    operation: &ManagedFileOperationV1,
    index: usize,
) -> Result<(), UpdaterError> {
    remove_managed_operation_temp(
        transaction,
        operation,
        FileMutationPhase::ActivateManaged,
        index,
    )?;
    let root = &transaction.paths.installation_root;
    match operation {
        ManagedFileOperationV1::Add { new } => {
            let Some(destination) = optional_managed_file(root, &new.path)? else {
                return Ok(());
            };
            if file_matches(&destination, new)? {
                remove_file_synced(&destination)
                    .map_err(|_| rollback("failed to remove added managed file during rollback"))?;
            }
            Ok(())
        }
        ManagedFileOperationV1::Replace { old, new } => {
            if let Some(destination) = optional_managed_file(root, &old.path)? {
                if file_matches(&destination, old)? {
                    return Ok(());
                }
                if !file_matches(&destination, new)? {
                    return Err(rollback(
                        "rollback replacement destination contains unknown file content",
                    ));
                }
            }
            install_managed_file(
                transaction,
                &transaction.paths.backup_files_root(),
                old,
                FileMutationPhase::RollbackManaged,
                index,
            )
        }
        ManagedFileOperationV1::Remove { old } => {
            if let Some(destination) = optional_managed_file(root, &old.path)? {
                if file_matches(&destination, old)? {
                    return Ok(());
                }
                return Err(rollback(
                    "rollback removal destination contains unknown file content",
                ));
            }
            install_managed_file(
                transaction,
                &transaction.paths.backup_files_root(),
                old,
                FileMutationPhase::RollbackManaged,
                index,
            )
        }
    }
}

fn install_managed_file(
    loaded: &LoadedTransaction,
    source_root: &Path,
    expected: &ManagedFile,
    phase: FileMutationPhase,
    index: usize,
) -> Result<(), UpdaterError> {
    let source = existing_managed_file(source_root, &expected.path)?;
    verify_file(&source, expected)?;
    let destination = create_managed_destination(&loaded.paths.installation_root, &expected.path)?;
    let parent = destination
        .parent()
        .ok_or_else(|| transaction("managed destination has no parent directory"))?;
    let temporary = parent.join(operation_temp_name(loaded, phase, index));
    prepare_managed_temp(&source, &temporary, expected)?;
    atomic_replace(&temporary, &destination)
        .map_err(|_| mutation_error(phase, "failed to atomically replace managed file"))?;
    verify_file(&destination, expected)
        .map_err(|_| mutation_error(phase, "replaced managed file failed verification"))
}

fn prepare_managed_temp(
    source: &Path,
    temporary: &Path,
    expected: &ManagedFile,
) -> Result<(), UpdaterError> {
    match fs::symlink_metadata(temporary) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            copy_verified_file(source, temporary, expected)
        }
        Ok(_) => {
            ensure_direct_file(temporary)?;
            if !file_matches(temporary, expected)? {
                return Err(recovery(
                    "managed operation temporary file contains unknown content",
                ));
            }
            Ok(())
        }
        Err(_) => Err(recovery(
            "failed to inspect managed operation temporary file",
        )),
    }
}

fn remove_managed_operation_temp(
    transaction: &LoadedTransaction,
    operation: &ManagedFileOperationV1,
    phase: FileMutationPhase,
    index: usize,
) -> Result<(), UpdaterError> {
    let expected = match operation {
        ManagedFileOperationV1::Add { new } | ManagedFileOperationV1::Replace { new, .. } => {
            Some(new)
        }
        ManagedFileOperationV1::Remove { .. } => None,
    };
    let Some(expected) = expected else {
        return Ok(());
    };
    let destination =
        prospective_managed_file(&transaction.paths.installation_root, &expected.path)?;
    let parent = destination
        .parent()
        .ok_or_else(|| recovery("managed destination has no parent directory"))?;
    let temporary = parent.join(operation_temp_name(transaction, phase, index));
    remove_known_managed_temp(&temporary, expected)
}

fn remove_known_managed_temp(temporary: &Path, expected: &ManagedFile) -> Result<(), UpdaterError> {
    match fs::symlink_metadata(temporary) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Ok(_) => {
            ensure_direct_file(temporary)?;
            if !file_matches(temporary, expected)? {
                return Err(recovery(
                    "managed operation temporary file contains unknown content",
                ));
            }
            remove_file_synced(temporary)
                .map_err(|_| recovery("failed to remove managed operation temporary file"))
        }
        Err(_) => Err(recovery(
            "failed to inspect managed operation temporary file",
        )),
    }
}

fn operation_temp_name(
    transaction: &LoadedTransaction,
    phase: FileMutationPhase,
    index: usize,
) -> String {
    format!(
        ".ah-update-{}-{}-{index}.tmp",
        transaction.plan.transaction_id,
        phase_name(phase)
    )
}

fn phase_name(phase: FileMutationPhase) -> &'static str {
    match phase {
        FileMutationPhase::StageCandidate => "stage",
        FileMutationPhase::BackupManaged => "backup",
        FileMutationPhase::ActivateManaged => "activate",
        FileMutationPhase::PublishInstalledRecord => "publish",
        FileMutationPhase::RollbackManaged => "rollback",
        FileMutationPhase::RestoreInstalledRecord => "restore",
    }
}

fn mutation_error(phase: FileMutationPhase, detail: &'static str) -> UpdaterError {
    match phase {
        FileMutationPhase::StageCandidate | FileMutationPhase::BackupManaged => transaction(detail),
        FileMutationPhase::ActivateManaged | FileMutationPhase::PublishInstalledRecord => {
            activation(detail)
        }
        FileMutationPhase::RollbackManaged | FileMutationPhase::RestoreInstalledRecord => {
            rollback(detail)
        }
    }
}

fn optional_managed_file(root: &Path, relative: &str) -> Result<Option<PathBuf>, UpdaterError> {
    let path = prospective_managed_file(root, relative)?;
    match fs::symlink_metadata(&path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Ok(_) => {
            ensure_direct_file(&path)?;
            Ok(Some(path))
        }
        Err(_) => Err(transaction("failed to inspect managed destination file")),
    }
}

fn file_matches(path: &Path, expected: &ManagedFile) -> Result<bool, UpdaterError> {
    let metadata = fs::metadata(path)
        .map_err(|_| transaction("failed to inspect managed destination file"))?;
    if metadata.len() != expected.size {
        return Ok(false);
    }
    let mut input =
        File::open(path).map_err(|_| transaction("failed to open managed destination file"))?;
    Ok(hash_exact(&mut input, expected.size)? == expected.sha256)
}

fn publish_candidate_record(
    transaction: &LoadedTransaction,
    injector: &mut impl FailureInjector,
) -> Result<(), TransactionRunError> {
    for (index, relative) in [INSTALLED_MANIFEST_FILE, INSTALLED_SIGNATURE_FILE]
        .into_iter()
        .enumerate()
    {
        mutate_with_checkpoint(
            injector,
            FileMutationPhase::PublishInstalledRecord,
            index,
            relative,
            || {
                replace_release_record_file(
                    transaction,
                    &transaction.paths.candidate_root().join(relative),
                    &transaction.paths.backup_root().join(relative),
                    &transaction.paths.installation_state_root.join(relative),
                    FileMutationPhase::PublishInstalledRecord,
                    index,
                    false,
                )
            },
        )?;
    }
    Ok(())
}

fn restore_backup_record(
    transaction: &LoadedTransaction,
    injector: &mut impl FailureInjector,
) -> Result<(), TransactionRunError> {
    for (index, relative) in [
        IDENTITY_FILE,
        INSTALLED_MANIFEST_FILE,
        INSTALLED_SIGNATURE_FILE,
    ]
    .into_iter()
    .enumerate()
    {
        mutate_with_checkpoint(
            injector,
            FileMutationPhase::RestoreInstalledRecord,
            index,
            relative,
            || {
                if relative != IDENTITY_FILE {
                    remove_record_temp(
                        transaction,
                        &transaction.paths.candidate_root().join(relative),
                        FileMutationPhase::PublishInstalledRecord,
                        index - 1,
                    )?;
                }
                replace_release_record_file(
                    transaction,
                    &transaction.paths.backup_root().join(relative),
                    &transaction.paths.candidate_root().join(relative),
                    &transaction.paths.installation_state_root.join(relative),
                    FileMutationPhase::RestoreInstalledRecord,
                    index,
                    true,
                )
            },
        )?;
    }
    Ok(())
}

fn replace_release_record_file(
    loaded: &LoadedTransaction,
    desired_source: &Path,
    alternate_source: &Path,
    destination: &Path,
    phase: FileMutationPhase,
    index: usize,
    allow_missing: bool,
) -> Result<(), UpdaterError> {
    let maximum = record_limit(
        destination
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| transaction("release record destination name is invalid"))?,
    );
    let desired = read_bounded(
        desired_source,
        maximum,
        "failed to read durable release record source",
    )?;
    let alternate = if alternate_source.exists() {
        Some(read_bounded(
            alternate_source,
            maximum,
            "failed to read durable release record alternate",
        )?)
    } else {
        None
    };
    match read_optional_bounded(destination, maximum)? {
        Some(current) if current == desired => return Ok(()),
        Some(current) if alternate.as_ref().is_some_and(|bytes| *bytes == current) => {}
        Some(_) => {
            return Err(recovery(
                "active release record contains unknown durable content",
            ));
        }
        None if allow_missing => {}
        None => return Err(recovery("active release record is unexpectedly missing")),
    }
    let parent = destination
        .parent()
        .ok_or_else(|| transaction("release record destination has no parent"))?;
    ensure_direct_directory(parent)?;
    let temporary = parent.join(operation_temp_name(loaded, phase, index));
    prepare_record_temp(&temporary, &desired)?;
    atomic_replace(&temporary, destination)
        .map_err(|_| transaction("failed to atomically replace active release record"))
}

fn remove_record_temp(
    transaction: &LoadedTransaction,
    expected_source: &Path,
    phase: FileMutationPhase,
    index: usize,
) -> Result<(), UpdaterError> {
    let parent = transaction.paths.installation_state_root();
    let temporary = parent.join(operation_temp_name(transaction, phase, index));
    let expected = read_bounded(
        expected_source,
        record_limit(
            expected_source
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or_else(|| recovery("release record source name is invalid"))?,
        ),
        "failed to read release record temporary expectation",
    )?;
    match read_optional_bounded(&temporary, expected.len())? {
        None => Ok(()),
        Some(bytes) if bytes == expected => remove_file_synced(&temporary)
            .map_err(|_| recovery("failed to remove release record temporary file")),
        Some(_) => Err(recovery(
            "release record temporary file contains unknown content",
        )),
    }
}

fn prepare_record_temp(temporary: &Path, expected: &[u8]) -> Result<(), UpdaterError> {
    match read_optional_bounded(temporary, expected.len())? {
        None => write_new_synced(temporary, expected),
        Some(bytes) if bytes == expected => Ok(()),
        Some(_) => Err(recovery(
            "release record temporary file contains unknown content",
        )),
    }
}

fn read_optional_bounded(path: &Path, maximum: usize) -> Result<Option<Vec<u8>>, UpdaterError> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Ok(_) => read_bounded(path, maximum, "failed to read active release record").map(Some),
        Err(_) => Err(recovery("failed to inspect active release record")),
    }
}

fn record_limit(file_name: &str) -> usize {
    match file_name {
        IDENTITY_FILE => MAX_IDENTITY_BYTES,
        INSTALLED_MANIFEST_FILE => ah_updater_core::MAX_MANIFEST_BYTES,
        INSTALLED_SIGNATURE_FILE => ah_updater_core::DETACHED_SIGNATURE_BYTES,
        _ => 0,
    }
}

fn remove_file_synced(path: &Path) -> io::Result<()> {
    fs::remove_file(path)?;
    sync_parent(path)
}

fn transaction_error(detail: &'static str) -> TransactionRunError {
    TransactionRunError::Failed(transaction(detail))
}

fn rollback_error(detail: &'static str) -> TransactionRunError {
    TransactionRunError::Failed(rollback(detail))
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

fn validate_transaction_location(
    paths: &TransactionPaths,
    plan: &TransactionPlanV1,
) -> Result<(), UpdaterError> {
    let expected = paths
        .installation_state_root
        .join("transactions")
        .join(plan.transaction_id.to_string());
    if !paths_equal(&paths.transaction_root, &expected) {
        return Err(recovery(
            "durable transaction root is not bound to its installation state",
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
    let encoded = encode_journal(journal)?;
    match read_optional_bounded(&temporary, MAX_JOURNAL_BYTES)? {
        None => write_new_synced(&temporary, &encoded)?,
        Some(existing) if existing == encoded => {}
        Some(_) => {
            return Err(recovery(
                "transaction journal temporary file contains unknown content",
            ));
        }
    }
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

fn copy_manifest_files_with_injector(
    injector: &mut impl FailureInjector,
    phase: FileMutationPhase,
    source_root: &Path,
    destination_root: &Path,
    manifest: &ReleaseManifest,
) -> Result<(), TransactionRunError> {
    validate_inventory_bounds(manifest)?;
    for (index, managed) in manifest.files.iter().enumerate() {
        mutate_with_checkpoint(injector, phase, index, &managed.path, || {
            let source = existing_managed_file(source_root, &managed.path)?;
            let destination = create_managed_destination(destination_root, &managed.path)?;
            copy_verified_file(&source, &destination, managed)
        })?;
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

fn activation(detail: impl Into<String>) -> UpdaterError {
    UpdaterError::new(UpdaterErrorCode::Activation, detail)
}

fn rollback(detail: impl Into<String>) -> UpdaterError {
    UpdaterError::new(UpdaterErrorCode::Rollback, detail)
}
