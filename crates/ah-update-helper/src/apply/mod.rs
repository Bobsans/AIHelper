//! Update transaction *execution*: prepare, activate, commit, roll back and
//! recover a plan against the real filesystem.
//!
//! The plan and journal types this module drives live in
//! `ah-updater-core::plan`.
//!
//! This file is the lifecycle and nothing else: each public entry point loads
//! or validates, then drives one of the submodules. The 2 195 lines that used
//! to be here hid the order of those steps, which is the whole safety
//! argument, because the steps and the primitives they call were interleaved.
//! Read the submodules in dependency order:
//!
//! | Module      | Owns                                                          |
//! |-------------|---------------------------------------------------------------|
//! | `layout`  | where a transaction's files live, and the size bounds          |
//! | `fsverify`| the only filesystem primitives this path may use               |
//! | `journal` | the state record every mutation checkpoints through            |
//! | `record`  | the identity/manifest/signature triple an installation reports |
//! | `managed` | staging, verifying and replacing the managed files             |
//! | `backup`  | the single permanent backup and its promotion                  |

mod backup;
mod fsverify;
mod journal;
mod layout;
mod managed;
mod record;

use std::{
    fs::{self},
    path::{Path, PathBuf},
};

use ah_updater_core::{
    InstallationIdentityV1, ReleaseManifest, ReleaseTrust, TransactionJournalV1, TransactionPlanV1,
    TransactionStateV1, UpdaterError, UpdaterErrorCode,
};

pub use backup::{PermanentBackup, finalize_completed_transaction, load_permanent_backup};
use fsverify::{read_bounded, sync_parent, write_new_synced};
use journal::{
    advance_journal_with, encode_journal, mutate_with_checkpoint, persist_initial_journal,
    state_requires_rollback,
};
pub use layout::TransactionPaths;
use layout::{
    BACKUP_DIRECTORY, CANDIDATE_DIRECTORY, FILES_DIRECTORY, IDENTITY_FILE, INSTALLED_MANIFEST_FILE,
    INSTALLED_SIGNATURE_FILE, JOURNAL_FILE, MAX_IDENTITY_BYTES, MAX_JOURNAL_BYTES, MAX_PLAN_BYTES,
    PLAN_FILE, create_transaction_layout, validate_existing_paths, validate_prepare_paths,
    validate_transaction_location,
};
pub use managed::FileMutationPhase;
use managed::{
    apply_activation_operation, apply_rollback_operation, copy_manifest_files_with_injector,
    verify_add_destinations_absent, verify_manifest_files,
};
use record::{
    decode_identity, publish_candidate_record, read_release_record, restore_backup_record,
    validate_identity, verify_active_new_release, verify_active_old_release, verify_release_record,
};

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

fn transaction_error(detail: &'static str) -> TransactionRunError {
    TransactionRunError::Failed(transaction(detail))
}

fn rollback_error(detail: &'static str) -> TransactionRunError {
    TransactionRunError::Failed(rollback(detail))
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
