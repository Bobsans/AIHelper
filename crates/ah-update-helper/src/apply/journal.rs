//! The journal: the only record of how far a transaction got.
//!
//! Every state change goes through [`mutate_with_checkpoint`], so a crash
//! leaves a state the recovery path can name.

use ah_updater_core::{TransactionJournalV1, TransactionStateV1, UpdaterError};

use super::fsverify::{atomic_replace, read_optional_bounded, write_new_synced};
use super::layout::{JOURNAL_FILE, MAX_JOURNAL_BYTES, TransactionPaths};
use super::managed::FileMutationPhase;
use super::{FailureInjector, FailurePoint, TransactionRunError, recovery, transaction};

pub(super) fn state_requires_rollback(state: TransactionStateV1) -> bool {
    matches!(
        state,
        TransactionStateV1::ActivationStarted
            | TransactionStateV1::CandidateActivated
            | TransactionStateV1::PermanentVerified
            | TransactionStateV1::CommitStarted
    )
}

pub(super) fn advance_journal_with(
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

pub(super) fn mutate_with_checkpoint(
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

pub(super) fn checkpoint(
    injector: &mut impl FailureInjector,
    point: FailurePoint,
) -> Result<(), TransactionRunError> {
    if injector.interrupt(&point) {
        Err(TransactionRunError::Interrupted(point))
    } else {
        Ok(())
    }
}

pub(super) fn persist_initial_journal(
    paths: &TransactionPaths,
    journal: &TransactionJournalV1,
) -> Result<(), UpdaterError> {
    write_new_synced(
        &paths.transaction_root.join(JOURNAL_FILE),
        &encode_journal(journal)?,
    )
}

pub(super) fn replace_journal(
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

pub(super) fn encode_journal(journal: &TransactionJournalV1) -> Result<Vec<u8>, UpdaterError> {
    serde_json::to_vec(journal)
        .map_err(|_| transaction("failed to serialize durable transaction journal"))
}
