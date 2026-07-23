use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use ah_release_manifest::{ManagedFile, ReleaseManifest};
use semver::Version;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::{UpdaterError, UpdaterErrorCode};

pub const TRANSACTION_PLAN_SCHEMA_VERSION: u32 = 1;
pub const TRANSACTION_JOURNAL_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TransactionPlanV1 {
    pub schema_version: u32,
    pub transaction_id: Uuid,
    pub installation_id: Uuid,
    pub old_version: String,
    pub new_version: String,
    pub target: String,
    pub architecture: String,
    pub old_manifest_sha256: String,
    pub new_manifest_sha256: String,
    pub operations: Vec<ManagedFileOperationV1>,
}

impl TransactionPlanV1 {
    pub fn build(
        transaction_id: Uuid,
        installation_id: Uuid,
        old_manifest: &ReleaseManifest,
        new_manifest: &ReleaseManifest,
    ) -> Result<Self, UpdaterError> {
        old_manifest
            .validate()
            .map_err(|_| transaction("installed release manifest is invalid"))?;
        new_manifest
            .validate()
            .map_err(|_| transaction("candidate release manifest is invalid"))?;
        if transaction_id.is_nil() || installation_id.is_nil() {
            return Err(transaction("transaction identity is invalid"));
        }
        if old_manifest.release.target != new_manifest.release.target
            || old_manifest.release.architecture != new_manifest.release.architecture
        {
            return Err(transaction(
                "installed and candidate release targets do not match",
            ));
        }
        let old_version = canonical_version(&old_manifest.release.version)?;
        let new_version = canonical_version(&new_manifest.release.version)?;
        if new_version < old_version {
            return Err(transaction(
                "candidate release version must not be older than the installed version",
            ));
        }

        let old_files = inventory(old_manifest);
        let new_files = inventory(new_manifest);
        let paths = old_files
            .keys()
            .chain(new_files.keys())
            .copied()
            .collect::<BTreeSet<_>>();
        let mut operations = Vec::new();
        for path in paths {
            match (old_files.get(path), new_files.get(path)) {
                (None, Some(new)) => {
                    operations.push(ManagedFileOperationV1::Add {
                        new: (*new).clone(),
                    });
                }
                (Some(old), None) => {
                    operations.push(ManagedFileOperationV1::Remove {
                        old: (*old).clone(),
                    });
                }
                (Some(old), Some(new)) if old != new => {
                    operations.push(ManagedFileOperationV1::Replace {
                        old: (*old).clone(),
                        new: (*new).clone(),
                    });
                }
                (Some(_), Some(_)) => {}
                (None, None) => unreachable!("path union contains at least one inventory entry"),
            }
        }

        let plan = Self {
            schema_version: TRANSACTION_PLAN_SCHEMA_VERSION,
            transaction_id,
            installation_id,
            old_version: old_version.to_string(),
            new_version: new_version.to_string(),
            target: old_manifest.release.target.clone(),
            architecture: old_manifest.release.architecture.clone(),
            old_manifest_sha256: manifest_sha256(old_manifest)?,
            new_manifest_sha256: manifest_sha256(new_manifest)?,
            operations,
        };
        plan.validate()?;
        Ok(plan)
    }

    pub fn validate(&self) -> Result<(), UpdaterError> {
        if self.schema_version != TRANSACTION_PLAN_SCHEMA_VERSION {
            return Err(transaction(
                "transaction plan schema version is unsupported",
            ));
        }
        if self.transaction_id.is_nil() || self.installation_id.is_nil() {
            return Err(transaction("transaction identity is invalid"));
        }
        let old_version = canonical_version(&self.old_version)?;
        let new_version = canonical_version(&self.new_version)?;
        if new_version < old_version {
            return Err(transaction(
                "candidate release version must not be older than the installed version",
            ));
        }
        if self.target.is_empty()
            || self.architecture.is_empty()
            || self.target.contains('\0')
            || self.architecture.contains('\0')
        {
            return Err(transaction("transaction target identity is invalid"));
        }
        validate_digest(&self.old_manifest_sha256)?;
        validate_digest(&self.new_manifest_sha256)?;

        let mut previous_path: Option<&str> = None;
        for operation in &self.operations {
            operation.validate()?;
            let path = operation.path();
            if previous_path.is_some_and(|previous| previous >= path) {
                return Err(transaction(
                    "transaction operations must be strictly sorted and unique",
                ));
            }
            previous_path = Some(path);
        }
        Ok(())
    }

    pub fn validate_for_manifests(
        &self,
        old_manifest: &ReleaseManifest,
        new_manifest: &ReleaseManifest,
    ) -> Result<(), UpdaterError> {
        let expected = Self::build(
            self.transaction_id,
            self.installation_id,
            old_manifest,
            new_manifest,
        )?;
        if self != &expected {
            return Err(transaction(
                "transaction plan does not match its release manifests",
            ));
        }
        Ok(())
    }

    pub fn to_canonical_bytes(&self) -> Result<Vec<u8>, UpdaterError> {
        self.validate()?;
        serde_json::to_vec(self)
            .map_err(|_| transaction("failed to serialize the transaction plan"))
    }

    pub fn sha256(&self) -> Result<String, UpdaterError> {
        Ok(encode_digest(Sha256::digest(self.to_canonical_bytes()?)))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum ManagedFileOperationV1 {
    Add { new: ManagedFile },
    Replace { old: ManagedFile, new: ManagedFile },
    Remove { old: ManagedFile },
}

impl ManagedFileOperationV1 {
    pub fn path(&self) -> &str {
        match self {
            Self::Add { new } | Self::Replace { new, .. } => &new.path,
            Self::Remove { old } => &old.path,
        }
    }

    fn validate(&self) -> Result<(), UpdaterError> {
        match self {
            Self::Add { new } => validate_managed_file(new),
            Self::Remove { old } => validate_managed_file(old),
            Self::Replace { old, new } => {
                validate_managed_file(old)?;
                validate_managed_file(new)?;
                if old.path != new.path || old == new {
                    return Err(transaction(
                        "transaction replacement metadata is inconsistent",
                    ));
                }
                Ok(())
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransactionStateV1 {
    Planned,
    BackupPrepared,
    ActivationStarted,
    CandidateActivated,
    PermanentVerified,
    CommitStarted,
    Committed,
    RollbackStarted,
    RolledBack,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TransactionJournalV1 {
    pub schema_version: u32,
    pub transaction_id: Uuid,
    pub installation_id: Uuid,
    pub plan_sha256: String,
    pub state: TransactionStateV1,
    pub transition_sequence: u32,
}

impl TransactionJournalV1 {
    pub fn new(plan: &TransactionPlanV1) -> Result<Self, UpdaterError> {
        plan.validate()?;
        Ok(Self {
            schema_version: TRANSACTION_JOURNAL_SCHEMA_VERSION,
            transaction_id: plan.transaction_id,
            installation_id: plan.installation_id,
            plan_sha256: plan.sha256()?,
            state: TransactionStateV1::Planned,
            transition_sequence: 0,
        })
    }

    pub fn validate_for(&self, plan: &TransactionPlanV1) -> Result<(), UpdaterError> {
        self.validate()?;
        plan.validate()?;
        if self.transaction_id != plan.transaction_id
            || self.installation_id != plan.installation_id
            || self.plan_sha256 != plan.sha256()?
        {
            return Err(transaction(
                "transaction journal does not match its immutable plan",
            ));
        }
        Ok(())
    }

    pub fn advance(&mut self, next: TransactionStateV1) -> Result<(), UpdaterError> {
        if !allowed_transition(self.state, next) {
            return Err(transaction("transaction state transition is invalid"));
        }
        self.transition_sequence = self
            .transition_sequence
            .checked_add(1)
            .ok_or_else(|| transaction("transaction transition sequence overflow"))?;
        self.state = next;
        Ok(())
    }

    pub fn validate(&self) -> Result<(), UpdaterError> {
        if self.schema_version != TRANSACTION_JOURNAL_SCHEMA_VERSION {
            return Err(transaction(
                "transaction journal schema version is unsupported",
            ));
        }
        if self.transaction_id.is_nil() || self.installation_id.is_nil() {
            return Err(transaction("transaction journal identity is invalid"));
        }
        validate_digest(&self.plan_sha256)?;
        if self.transition_sequence < minimum_sequence(self.state) {
            return Err(transaction(
                "transaction journal sequence is inconsistent with its state",
            ));
        }
        Ok(())
    }
}

fn allowed_transition(current: TransactionStateV1, next: TransactionStateV1) -> bool {
    use TransactionStateV1::{
        ActivationStarted, BackupPrepared, CandidateActivated, CommitStarted, Committed,
        PermanentVerified, Planned, RollbackStarted, RolledBack,
    };

    matches!(
        (current, next),
        (Planned, BackupPrepared)
            | (BackupPrepared, ActivationStarted)
            | (ActivationStarted, CandidateActivated | RollbackStarted)
            | (CandidateActivated, PermanentVerified | RollbackStarted)
            | (PermanentVerified, CommitStarted | RollbackStarted)
            | (CommitStarted, Committed | RollbackStarted)
            | (RollbackStarted, RolledBack)
    )
}

fn minimum_sequence(state: TransactionStateV1) -> u32 {
    use TransactionStateV1::{
        ActivationStarted, BackupPrepared, CandidateActivated, CommitStarted, Committed,
        PermanentVerified, Planned, RollbackStarted, RolledBack,
    };

    match state {
        Planned => 0,
        BackupPrepared => 1,
        ActivationStarted => 2,
        CandidateActivated => 3,
        PermanentVerified => 4,
        CommitStarted => 5,
        Committed => 6,
        RollbackStarted => 3,
        RolledBack => 4,
    }
}

fn inventory(manifest: &ReleaseManifest) -> BTreeMap<&str, &ManagedFile> {
    manifest
        .files
        .iter()
        .map(|file| (file.path.as_str(), file))
        .collect()
}

fn canonical_version(raw: &str) -> Result<Version, UpdaterError> {
    let version =
        Version::parse(raw).map_err(|_| transaction("transaction release version is invalid"))?;
    if version.to_string() != raw {
        return Err(transaction("transaction release version is not canonical"));
    }
    Ok(version)
}

fn validate_digest(digest: &str) -> Result<(), UpdaterError> {
    if digest.len() != 64
        || !digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    {
        return Err(transaction("transaction plan digest is invalid"));
    }
    Ok(())
}

fn encode_digest(bytes: impl AsRef<[u8]>) -> String {
    let bytes = bytes.as_ref();
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        write!(&mut encoded, "{byte:02x}").expect("writing to a String cannot fail");
    }
    encoded
}

fn manifest_sha256(manifest: &ReleaseManifest) -> Result<String, UpdaterError> {
    let bytes = manifest
        .to_canonical_bytes()
        .map_err(|_| transaction("failed to canonicalize a transaction release manifest"))?;
    Ok(encode_digest(Sha256::digest(bytes)))
}

fn validate_managed_file(file: &ManagedFile) -> Result<(), UpdaterError> {
    let path = &file.path;
    if path.is_empty()
        || path.len() > ah_release_manifest::MAX_MANAGED_PATH_BYTES
        || !path.is_ascii()
        || !path
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'/'))
        || path.starts_with('/')
        || path.ends_with('/')
        || path
            .split('/')
            .any(|component| component.is_empty() || matches!(component, "." | ".."))
    {
        return Err(transaction("transaction managed-file path is invalid"));
    }
    validate_digest(&file.sha256)
}

fn transaction(detail: impl Into<String>) -> UpdaterError {
    UpdaterError::new(UpdaterErrorCode::Transaction, detail)
}

#[cfg(test)]
mod tests {
    use ah_release_manifest::{
        ArchiveMetadata, FilePurpose, ManagedFile, ReleaseMetadata, RequiredFiles, SCHEMA_VERSION,
        SignatureAlgorithm, SigningMetadata,
    };

    use super::*;

    #[test]
    fn builds_exact_sorted_add_replace_remove_plan() {
        let old = manifest(
            "1.1.0",
            vec![
                file("ah.exe", "1", FilePurpose::Executable),
                file("plugins/old.dll", "2", FilePurpose::Plugin),
                file("shared.dat", "3", FilePurpose::Support),
            ],
        );
        let new = manifest(
            "1.2.0",
            vec![
                file("ah.exe", "4", FilePurpose::Executable),
                file("plugins/new.dll", "5", FilePurpose::Plugin),
                file("shared.dat", "3", FilePurpose::Support),
            ],
        );

        let plan = TransactionPlanV1::build(Uuid::new_v4(), Uuid::new_v4(), &old, &new).unwrap();

        assert_eq!(plan.operations.len(), 3);
        assert!(matches!(
            &plan.operations[0],
            ManagedFileOperationV1::Replace { old, new }
                if old.path == "ah.exe" && new.path == "ah.exe"
        ));
        assert!(matches!(
            &plan.operations[1],
            ManagedFileOperationV1::Add { new } if new.path == "plugins/new.dll"
        ));
        assert!(matches!(
            &plan.operations[2],
            ManagedFileOperationV1::Remove { old } if old.path == "plugins/old.dll"
        ));
        plan.validate().unwrap();
        plan.validate_for_manifests(&old, &new).unwrap();
        let mut tampered_new = new.clone();
        tampered_new.files[0].sha256 = "6".repeat(64);
        assert!(plan.validate_for_manifests(&old, &tampered_new).is_err());
        assert_eq!(plan.sha256().unwrap().len(), 64);
        assert_eq!(
            serde_json::from_slice::<TransactionPlanV1>(&plan.to_canonical_bytes().unwrap())
                .unwrap(),
            plan
        );
    }

    #[test]
    fn rejects_downgrade_target_mismatch_and_invalid_operation_order() {
        let old = manifest("2.0.0", vec![file("ah.exe", "1", FilePurpose::Executable)]);
        let downgrade = manifest("1.0.0", vec![file("ah.exe", "2", FilePurpose::Executable)]);
        assert_eq!(
            TransactionPlanV1::build(Uuid::new_v4(), Uuid::new_v4(), &old, &downgrade)
                .unwrap_err()
                .code(),
            UpdaterErrorCode::Transaction
        );

        let mut wrong_target = old.clone();
        wrong_target.release.target = "x86_64-unknown-linux-gnu".to_owned();
        assert!(
            TransactionPlanV1::build(Uuid::new_v4(), Uuid::new_v4(), &old, &wrong_target).is_err()
        );

        let mut invalid =
            TransactionPlanV1::build(Uuid::new_v4(), Uuid::new_v4(), &downgrade, &old).unwrap();
        invalid.operations.push(ManagedFileOperationV1::Remove {
            old: file("ah.exe", "1", FilePurpose::Executable),
        });
        assert!(invalid.validate().is_err());
    }

    #[test]
    fn journal_binds_plan_and_allows_only_durable_state_graph() {
        let old = manifest("1.1.0", vec![file("ah.exe", "1", FilePurpose::Executable)]);
        let new = manifest("1.2.0", vec![file("ah.exe", "2", FilePurpose::Executable)]);
        let plan = TransactionPlanV1::build(Uuid::new_v4(), Uuid::new_v4(), &old, &new).unwrap();
        let mut journal = TransactionJournalV1::new(&plan).unwrap();
        journal.validate_for(&plan).unwrap();
        assert!(
            journal
                .advance(TransactionStateV1::CandidateActivated)
                .is_err()
        );

        for state in [
            TransactionStateV1::BackupPrepared,
            TransactionStateV1::ActivationStarted,
            TransactionStateV1::CandidateActivated,
            TransactionStateV1::PermanentVerified,
            TransactionStateV1::CommitStarted,
            TransactionStateV1::Committed,
        ] {
            journal.advance(state).unwrap();
            journal.validate_for(&plan).unwrap();
        }
        assert_eq!(journal.transition_sequence, 6);
        assert!(
            journal
                .advance(TransactionStateV1::RollbackStarted)
                .is_err()
        );
    }

    #[test]
    fn journal_supports_rollback_from_every_post_mutation_boundary() {
        for failure_state in [
            TransactionStateV1::ActivationStarted,
            TransactionStateV1::CandidateActivated,
            TransactionStateV1::PermanentVerified,
            TransactionStateV1::CommitStarted,
        ] {
            let plan = plan();
            let mut journal = TransactionJournalV1::new(&plan).unwrap();
            journal.advance(TransactionStateV1::BackupPrepared).unwrap();
            journal
                .advance(TransactionStateV1::ActivationStarted)
                .unwrap();
            if failure_state != TransactionStateV1::ActivationStarted {
                journal
                    .advance(TransactionStateV1::CandidateActivated)
                    .unwrap();
            }
            if matches!(
                failure_state,
                TransactionStateV1::PermanentVerified | TransactionStateV1::CommitStarted
            ) {
                journal
                    .advance(TransactionStateV1::PermanentVerified)
                    .unwrap();
            }
            if failure_state == TransactionStateV1::CommitStarted {
                journal.advance(TransactionStateV1::CommitStarted).unwrap();
            }
            journal
                .advance(TransactionStateV1::RollbackStarted)
                .unwrap();
            journal.advance(TransactionStateV1::RolledBack).unwrap();
            journal.validate_for(&plan).unwrap();
        }
    }

    #[test]
    fn strict_json_rejects_unknown_plan_and_journal_fields() {
        let plan = plan();
        let mut plan_value = serde_json::to_value(&plan).unwrap();
        plan_value["extra"] = serde_json::json!(true);
        assert!(serde_json::from_value::<TransactionPlanV1>(plan_value).is_err());

        let journal = TransactionJournalV1::new(&plan).unwrap();
        let mut journal_value = serde_json::to_value(journal).unwrap();
        journal_value["extra"] = serde_json::json!(true);
        assert!(serde_json::from_value::<TransactionJournalV1>(journal_value).is_err());
    }

    fn plan() -> TransactionPlanV1 {
        TransactionPlanV1::build(
            Uuid::new_v4(),
            Uuid::new_v4(),
            &manifest("1.1.0", vec![file("ah.exe", "1", FilePurpose::Executable)]),
            &manifest("1.2.0", vec![file("ah.exe", "2", FilePurpose::Executable)]),
        )
        .unwrap()
    }

    fn manifest(version: &str, mut files: Vec<ManagedFile>) -> ReleaseManifest {
        files.sort_by(|left, right| left.path.cmp(&right.path));
        let executables = files
            .iter()
            .filter(|file| {
                matches!(
                    file.purpose,
                    FilePurpose::Executable | FilePurpose::UpdateHelper
                )
            })
            .map(|file| file.path.clone())
            .collect();
        let plugins = files
            .iter()
            .filter(|file| file.purpose == FilePurpose::Plugin)
            .map(|file| file.path.clone())
            .collect();
        ReleaseManifest {
            schema_version: SCHEMA_VERSION,
            release: ReleaseMetadata {
                version: version.to_owned(),
                target: "x86_64-pc-windows-msvc".to_owned(),
                architecture: "x86_64".to_owned(),
            },
            archive: ArchiveMetadata {
                url: "https://example.test/ah-windows-x64.zip".to_owned(),
                size: 1,
                sha256: "0".repeat(64),
            },
            minimum_updater_version: "1.0.0".to_owned(),
            signing: SigningMetadata {
                key_id:
                    "ed25519-sha256-0000000000000000000000000000000000000000000000000000000000000000"
                        .to_owned(),
                algorithm: SignatureAlgorithm::Ed25519,
            },
            required: RequiredFiles {
                executables,
                plugins,
            },
            files,
        }
    }

    fn file(path: &str, digest_digit: &str, purpose: FilePurpose) -> ManagedFile {
        ManagedFile {
            path: path.to_owned(),
            size: 1,
            sha256: digest_digit.repeat(64),
            purpose,
        }
    }
}
