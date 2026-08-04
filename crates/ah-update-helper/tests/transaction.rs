use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use ah_release_manifest::{
    ArchiveMetadata, FilePurpose, ManagedFile, ReleaseManifest, ReleaseMetadata, RequiredFiles,
    SCHEMA_VERSION, SIGNING_ALGORITHM, SIGNING_DOMAIN, SignatureAlgorithm, SigningMetadata,
    TrustedKey, key_id_for_public_key,
};
use ah_update_helper::transaction::{
    FailureInjector, FailurePoint, LoadedTransaction, TransactionPaths, TransactionRunError,
    activate_transaction, activate_transaction_with_injector, commit_transaction,
    commit_transaction_with_injector, load_prepared_transaction, prepare_transaction,
    prepare_transaction_with_injector, recover_transaction, remove_completed_transaction,
    rollback_transaction, rollback_transaction_with_injector,
};
use ah_updater_core::{
    InstallationIdentityV1, ReleaseTrust, TransactionPlanV1, TransactionStateV1, UpdaterErrorCode,
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use ed25519_dalek::{Signer as _, SigningKey};
use sha2::{Digest, Sha256};
use tempfile::TempDir;
use uuid::Uuid;

#[test]
fn prepares_complete_verified_backup_and_candidate_staging() {
    let fixture = Fixture::new();
    let user_file = fixture.installation_root.join("user-owned.txt");
    fs::write(&user_file, b"preserve me").unwrap();
    fs::write(
        fixture.candidate_source.join("unsigned-extra.txt"),
        b"ignore me",
    )
    .unwrap();

    let transaction = fixture.prepare().unwrap();

    assert_eq!(
        transaction.journal().state,
        TransactionStateV1::BackupPrepared
    );
    assert_eq!(transaction.plan(), &fixture.plan);
    assert_eq!(transaction.identity(), &fixture.identity);
    assert_eq!(fs::read(&user_file).unwrap(), b"preserve me");
    assert!(
        !transaction
            .paths()
            .backup_files_root()
            .join("user-owned.txt")
            .exists()
    );
    assert!(
        !transaction
            .paths()
            .candidate_files_root()
            .join("unsigned-extra.txt")
            .exists()
    );
    assert_tree_matches(&transaction.paths().backup_files_root(), &fixture.old_files);
    assert_tree_matches(
        &transaction.paths().candidate_files_root(),
        &fixture.new_files,
    );
    assert_eq!(
        fs::read(
            fixture
                .paths
                .transaction_root()
                .join("backup/identity.json")
        )
        .unwrap(),
        fixture.identity_bytes
    );
    assert_eq!(
        fs::read(
            fixture
                .paths
                .transaction_root()
                .join("candidate/installed.manifest.json")
        )
        .unwrap(),
        fixture.new_manifest_bytes
    );

    let reloaded = load_prepared_transaction(&fixture.paths, &fixture.trust).unwrap();
    assert_eq!(reloaded.plan(), &fixture.plan);
    let journal_bytes = fs::read(fixture.paths.transaction_root().join("journal.json")).unwrap();
    assert_eq!(
        journal_bytes,
        serde_json::to_vec(reloaded.journal()).unwrap()
    );
}

#[test]
fn refuses_modified_candidate_without_creating_transaction_state() {
    let fixture = Fixture::new();
    fs::write(fixture.candidate_source.join("ah.exe"), b"bad-ah").unwrap();

    let error = fixture.prepare().unwrap_err();

    assert_eq!(error.code(), UpdaterErrorCode::Transaction);
    assert!(!fixture.paths.transaction_root().exists());
    assert_tree_matches(&fixture.installation_root, &fixture.old_files);
}

#[test]
fn refuses_add_collision_without_changing_user_file() {
    let fixture = Fixture::new();
    let collision = fixture.installation_root.join("plugins/new.dll");
    fs::create_dir_all(collision.parent().unwrap()).unwrap();
    fs::write(&collision, b"user data").unwrap();

    let error = fixture.prepare().unwrap_err();

    assert_eq!(error.code(), UpdaterErrorCode::Transaction);
    assert_eq!(fs::read(collision).unwrap(), b"user data");
    assert!(!fixture.paths.transaction_root().exists());
}

#[test]
fn detects_staging_or_backup_tampering_at_helper_boundary() {
    for relative in ["candidate/files/ah.exe", "backup/files/ah.exe"] {
        let fixture = Fixture::new();
        fixture.prepare().unwrap();
        fs::write(fixture.paths.transaction_root().join(relative), b"tamper").unwrap();

        let error = load_prepared_transaction(&fixture.paths, &fixture.trust).unwrap_err();

        assert!(matches!(
            error.code(),
            UpdaterErrorCode::Transaction | UpdaterErrorCode::Recovery
        ));
        assert_tree_matches(&fixture.installation_root, &fixture.old_files);
    }
}

#[test]
fn activates_commits_and_recovers_idempotently_without_touching_user_files() {
    let fixture = Fixture::new();
    fixture.prepare().unwrap();

    let activated = activate_transaction(&fixture.paths, &fixture.trust).unwrap();
    assert_eq!(
        activated.journal().state,
        TransactionStateV1::PermanentVerified
    );
    fixture.assert_new_active();

    let committed = commit_transaction(&fixture.paths, &fixture.trust).unwrap();
    assert_eq!(committed.journal().state, TransactionStateV1::Committed);
    fixture.assert_new_active();
    let recovered = recover_transaction(&fixture.paths, &fixture.trust).unwrap();
    assert_eq!(recovered.journal().state, TransactionStateV1::Committed);
    fixture.assert_new_active();
}

#[test]
fn explicit_rollback_restores_complete_old_release_and_is_recoverable() {
    let fixture = Fixture::new();
    fixture.prepare().unwrap();
    activate_transaction(&fixture.paths, &fixture.trust).unwrap();

    let rolled_back = rollback_transaction(&fixture.paths, &fixture.trust).unwrap();

    assert_eq!(rolled_back.journal().state, TransactionStateV1::RolledBack);
    fixture.assert_old_active();
    let recovered = recover_transaction(&fixture.paths, &fixture.trust).unwrap();
    assert_eq!(recovered.journal().state, TransactionStateV1::RolledBack);
    fixture.assert_old_active();
}

#[test]
fn every_activation_and_commit_checkpoint_recovers_deterministically() {
    let discovery = Fixture::new();
    discovery.prepare().unwrap();
    let mut recorder = RecordingInjector::default();
    activate_transaction_with_injector(&discovery.paths, &discovery.trust, &mut recorder).unwrap();
    commit_transaction_with_injector(&discovery.paths, &discovery.trust, &mut recorder).unwrap();
    assert_unique_checkpoint_count(&recorder.points, 20);

    for target in recorder.points {
        let fixture = Fixture::new();
        fixture.prepare().unwrap();
        let mut injector = InterruptAt::new(target.clone());
        let activation =
            activate_transaction_with_injector(&fixture.paths, &fixture.trust, &mut injector);
        let result = match activation {
            Ok(_) => {
                commit_transaction_with_injector(&fixture.paths, &fixture.trust, &mut injector)
            }
            Err(error) => Err(error),
        };
        assert_eq!(
            result.unwrap_err(),
            TransactionRunError::Interrupted(target.clone())
        );
        assert!(injector.hit);

        let recovered = recover_transaction(&fixture.paths, &fixture.trust).unwrap();
        if matches!(
            target,
            FailurePoint::AfterJournalWrite {
                state: TransactionStateV1::Committed,
                ..
            }
        ) {
            assert_eq!(recovered.journal().state, TransactionStateV1::Committed);
            fixture.assert_new_active();
        } else {
            assert!(matches!(
                recovered.journal().state,
                TransactionStateV1::BackupPrepared | TransactionStateV1::RolledBack
            ));
            fixture.assert_old_active();
        }
    }
}

#[test]
fn every_staging_and_backup_checkpoint_leaves_recoverable_old_release() {
    let discovery = Fixture::new();
    let mut recorder = RecordingInjector::default();
    prepare_transaction_with_injector(
        &discovery.paths,
        &discovery.plan,
        &discovery.candidate_source,
        &discovery.new_manifest_bytes,
        &discovery.new_signature_bytes,
        &discovery.trust,
        &mut recorder,
    )
    .unwrap();
    assert_unique_checkpoint_count(&recorder.points, 14);

    for target in recorder.points {
        let fixture = Fixture::new();
        let mut injector = InterruptAt::new(target.clone());
        let result = prepare_transaction_with_injector(
            &fixture.paths,
            &fixture.plan,
            &fixture.candidate_source,
            &fixture.new_manifest_bytes,
            &fixture.new_signature_bytes,
            &fixture.trust,
            &mut injector,
        );

        assert_eq!(
            result.unwrap_err(),
            TransactionRunError::Interrupted(target)
        );
        assert!(injector.hit);
        assert!(fixture.paths.transaction_root().exists());
        let recovered = recover_transaction(&fixture.paths, &fixture.trust).unwrap();
        assert!(matches!(
            recovered.journal().state,
            TransactionStateV1::Planned | TransactionStateV1::BackupPrepared
        ));
        fixture.assert_old_active();
    }
}

#[test]
fn every_rollback_checkpoint_resumes_to_verified_old_release() {
    let discovery = Fixture::new();
    discovery.prepare().unwrap();
    activate_transaction(&discovery.paths, &discovery.trust).unwrap();
    let mut recorder = RecordingInjector::default();
    rollback_transaction_with_injector(&discovery.paths, &discovery.trust, &mut recorder).unwrap();
    assert_unique_checkpoint_count(&recorder.points, 16);

    for target in recorder.points {
        let fixture = Fixture::new();
        fixture.prepare().unwrap();
        activate_transaction(&fixture.paths, &fixture.trust).unwrap();
        let mut injector = InterruptAt::new(target.clone());

        let result =
            rollback_transaction_with_injector(&fixture.paths, &fixture.trust, &mut injector);

        assert_eq!(
            result.unwrap_err(),
            TransactionRunError::Interrupted(target)
        );
        assert!(injector.hit);
        let recovered = recover_transaction(&fixture.paths, &fixture.trust).unwrap();
        assert_eq!(recovered.journal().state, TransactionStateV1::RolledBack);
        fixture.assert_old_active();
    }
}

#[test]
fn rollback_uses_verified_backup_even_when_candidate_files_are_lost() {
    let fixture = Fixture::new();
    fixture.prepare().unwrap();
    activate_transaction(&fixture.paths, &fixture.trust).unwrap();
    fs::write(
        fixture.paths.candidate_files_root().join("plugins/new.dll"),
        b"lost-plugin",
    )
    .unwrap();

    let recovered = recover_transaction(&fixture.paths, &fixture.trust).unwrap();

    assert_eq!(recovered.journal().state, TransactionStateV1::RolledBack);
    fixture.assert_old_active();
}

#[test]
fn activation_boundary_preserves_foreign_add_path_before_any_mutation() {
    let fixture = Fixture::new();
    fixture.prepare().unwrap();
    let foreign = fixture.installation_root.join("plugins/new.dll");
    fs::write(&foreign, b"foreign-user-file").unwrap();

    let error = activate_transaction(&fixture.paths, &fixture.trust).unwrap_err();

    assert_eq!(error.code(), UpdaterErrorCode::Transaction);
    assert_eq!(fs::read(&foreign).unwrap(), b"foreign-user-file");
    assert_tree_matches(&fixture.installation_root, &fixture.old_files);
    assert_eq!(
        fs::read(fixture.installation_root.join("user-owned.txt")).unwrap(),
        b"preserve me"
    );
    let recovered = recover_transaction(&fixture.paths, &fixture.trust).unwrap();
    assert_eq!(
        recovered.journal().state,
        TransactionStateV1::BackupPrepared
    );
}

#[test]
fn recovery_preserves_foreign_add_race_after_partial_activation() {
    let fixture = Fixture::new();
    fixture.prepare().unwrap();
    let foreign = fixture.installation_root.join("plugins/new.dll");
    let mut injector = CreateForeignAfterFirstMutation {
        path: foreign.clone(),
        created: false,
    };

    let error = activate_transaction_with_injector(&fixture.paths, &fixture.trust, &mut injector)
        .unwrap_err();

    assert!(injector.created);
    assert!(matches!(
        error,
        TransactionRunError::Failed(ref error) if error.code() == UpdaterErrorCode::Activation
    ));
    let recovered = recover_transaction(&fixture.paths, &fixture.trust).unwrap();
    assert_eq!(recovered.journal().state, TransactionStateV1::RolledBack);
    assert_tree_matches(&fixture.installation_root, &fixture.old_files);
    assert_eq!(fs::read(foreign).unwrap(), b"foreign-user-file");
}

#[test]
fn recovery_fails_closed_and_retains_state_for_corrupt_backup() {
    let fixture = Fixture::new();
    fixture.prepare().unwrap();
    activate_transaction(&fixture.paths, &fixture.trust).unwrap();
    fs::write(fixture.paths.backup_files_root().join("ah.exe"), b"broken").unwrap();

    let error = recover_transaction(&fixture.paths, &fixture.trust).unwrap_err();

    assert_eq!(error.code(), UpdaterErrorCode::Transaction);
    let journal: ah_updater_core::TransactionJournalV1 = serde_json::from_slice(
        &fs::read(fixture.paths.transaction_root().join("journal.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(journal.state, TransactionStateV1::PermanentVerified);
    assert!(fixture.paths.transaction_root().exists());
    fixture.assert_new_active();
}

#[test]
fn cleanup_removes_only_verified_terminal_transaction_state() {
    let prepared = Fixture::new();
    prepared.prepare().unwrap();
    let state = remove_completed_transaction(&prepared.paths, &prepared.trust).unwrap();
    assert_eq!(state, TransactionStateV1::BackupPrepared);
    assert!(!prepared.paths.transaction_root().exists());
    prepared.assert_old_active();

    let active = Fixture::new();
    active.prepare().unwrap();
    activate_transaction(&active.paths, &active.trust).unwrap();
    assert!(remove_completed_transaction(&active.paths, &active.trust).is_err());
    assert!(active.paths.transaction_root().exists());
    rollback_transaction(&active.paths, &active.trust).unwrap();
    let state = remove_completed_transaction(&active.paths, &active.trust).unwrap();
    assert_eq!(state, TransactionStateV1::RolledBack);
    assert!(!active.paths.transaction_root().exists());
    active.assert_old_active();
}

#[derive(Default)]
struct RecordingInjector {
    points: Vec<FailurePoint>,
}

impl FailureInjector for RecordingInjector {
    fn interrupt(&mut self, point: &FailurePoint) -> bool {
        self.points.push(point.clone());
        false
    }
}

struct InterruptAt {
    target: FailurePoint,
    hit: bool,
}

impl InterruptAt {
    fn new(target: FailurePoint) -> Self {
        Self { target, hit: false }
    }
}

impl FailureInjector for InterruptAt {
    fn interrupt(&mut self, point: &FailurePoint) -> bool {
        if !self.hit && point == &self.target {
            self.hit = true;
            true
        } else {
            false
        }
    }
}

struct CreateForeignAfterFirstMutation {
    path: PathBuf,
    created: bool,
}

impl FailureInjector for CreateForeignAfterFirstMutation {
    fn interrupt(&mut self, point: &FailurePoint) -> bool {
        if !self.created
            && matches!(
                point,
                FailurePoint::AfterFileMutation {
                    phase: ah_update_helper::transaction::FileMutationPhase::ActivateManaged,
                    index: 0,
                    ..
                }
            )
        {
            fs::write(&self.path, b"foreign-user-file").unwrap();
            self.created = true;
        }
        false
    }
}

fn assert_unique_checkpoint_count(points: &[FailurePoint], expected: usize) {
    assert_eq!(points.len(), expected);
    for (index, point) in points.iter().enumerate() {
        assert!(
            !points[..index].contains(point),
            "duplicate failure checkpoint: {point:?}"
        );
    }
}

struct Fixture {
    _temporary: TempDir,
    installation_root: PathBuf,
    candidate_source: PathBuf,
    paths: TransactionPaths,
    trust: ReleaseTrust,
    plan: TransactionPlanV1,
    identity: InstallationIdentityV1,
    identity_bytes: Vec<u8>,
    old_manifest_bytes: Vec<u8>,
    old_signature_bytes: Vec<u8>,
    new_manifest_bytes: Vec<u8>,
    new_signature_bytes: Vec<u8>,
    old_files: BTreeMap<String, Vec<u8>>,
    new_files: BTreeMap<String, Vec<u8>>,
}

impl Fixture {
    fn new() -> Self {
        let temporary = TempDir::new().unwrap();
        let installation_root = temporary.path().join("installation");
        let candidate_source = temporary.path().join("candidate-source");
        let state_root = temporary.path().join("state");
        let transactions = state_root.join("transactions");
        fs::create_dir_all(&installation_root).unwrap();
        fs::create_dir_all(&candidate_source).unwrap();
        fs::create_dir_all(&transactions).unwrap();

        let old_files = BTreeMap::from([
            ("ah.exe".to_owned(), b"old-ah".to_vec()),
            ("plugins/old.dll".to_owned(), b"old-plugin".to_vec()),
            ("shared.dat".to_owned(), b"shared".to_vec()),
        ]);
        let new_files = BTreeMap::from([
            ("ah.exe".to_owned(), b"new-ah".to_vec()),
            ("plugins/new.dll".to_owned(), b"new-plugin".to_vec()),
            ("shared.dat".to_owned(), b"shared".to_vec()),
        ]);
        write_tree(&installation_root, &old_files);
        write_tree(&candidate_source, &new_files);
        fs::write(installation_root.join("user-owned.txt"), b"preserve me").unwrap();

        let signing_key = SigningKey::from_bytes(&[23_u8; 32]);
        let trusted_key = trusted_key(&signing_key);
        let trust = ReleaseTrust::from_keys(vec![trusted_key.clone()]).unwrap();
        let (old_manifest, old_manifest_bytes, old_signature_bytes) =
            signed_manifest("1.1.0", &old_files, &trusted_key.key_id, &signing_key);
        let (new_manifest, new_manifest_bytes, new_signature_bytes) =
            signed_manifest("1.2.0", &new_files, &trusted_key.key_id, &signing_key);
        let installation_id = Uuid::new_v4();
        let transaction_id = Uuid::new_v4();
        let identity = InstallationIdentityV1::new(
            installation_id,
            installation_root
                .join("ah.exe")
                .to_str()
                .unwrap()
                .to_owned(),
        )
        .unwrap();
        let mut identity_bytes = serde_json::to_vec_pretty(&identity).unwrap();
        identity_bytes.push(b'\n');
        fs::write(state_root.join("identity.json"), &identity_bytes).unwrap();
        fs::write(
            state_root.join("installed.manifest.json"),
            &old_manifest_bytes,
        )
        .unwrap();
        fs::write(
            state_root.join("installed.manifest.sig"),
            &old_signature_bytes,
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
            transactions.join(transaction_id.to_string()),
        );

        Self {
            _temporary: temporary,
            installation_root,
            candidate_source,
            paths,
            trust,
            plan,
            identity,
            identity_bytes,
            old_manifest_bytes,
            old_signature_bytes,
            new_manifest_bytes,
            new_signature_bytes,
            old_files,
            new_files,
        }
    }

    fn prepare(&self) -> Result<LoadedTransaction, ah_updater_core::UpdaterError> {
        prepare_transaction(
            &self.paths,
            &self.plan,
            &self.candidate_source,
            &self.new_manifest_bytes,
            &self.new_signature_bytes,
            &self.trust,
        )
    }

    fn assert_old_active(&self) {
        assert_tree_matches(&self.installation_root, &self.old_files);
        assert!(!self.installation_root.join("plugins/new.dll").exists());
        assert_eq!(
            fs::read(
                self.paths
                    .installation_state_root()
                    .join("installed.manifest.json")
            )
            .unwrap(),
            self.old_manifest_bytes
        );
        assert_eq!(
            fs::read(
                self.paths
                    .installation_state_root()
                    .join("installed.manifest.sig")
            )
            .unwrap(),
            self.old_signature_bytes
        );
        self.assert_identity_and_user_file();
    }

    fn assert_new_active(&self) {
        assert_tree_matches(&self.installation_root, &self.new_files);
        assert!(!self.installation_root.join("plugins/old.dll").exists());
        assert_eq!(
            fs::read(
                self.paths
                    .installation_state_root()
                    .join("installed.manifest.json")
            )
            .unwrap(),
            self.new_manifest_bytes
        );
        assert_eq!(
            fs::read(
                self.paths
                    .installation_state_root()
                    .join("installed.manifest.sig")
            )
            .unwrap(),
            self.new_signature_bytes
        );
        self.assert_identity_and_user_file();
    }

    fn assert_identity_and_user_file(&self) {
        assert_eq!(
            fs::read(self.paths.installation_state_root().join("identity.json")).unwrap(),
            self.identity_bytes
        );
        assert_eq!(
            fs::read(self.installation_root.join("user-owned.txt")).unwrap(),
            b"preserve me"
        );
    }
}

fn signed_manifest(
    version: &str,
    files: &BTreeMap<String, Vec<u8>>,
    key_id: &str,
    signing_key: &SigningKey,
) -> (ReleaseManifest, Vec<u8>, Vec<u8>) {
    let managed = files
        .iter()
        .map(|(path, bytes)| ManagedFile {
            path: path.clone(),
            size: bytes.len() as u64,
            sha256: encode_digest(Sha256::digest(bytes)),
            purpose: if path == "ah.exe" {
                FilePurpose::Executable
            } else if path.starts_with("plugins/") {
                FilePurpose::Plugin
            } else {
                FilePurpose::Support
            },
        })
        .collect::<Vec<_>>();
    let plugins = managed
        .iter()
        .filter(|file| file.purpose == FilePurpose::Plugin)
        .map(|file| file.path.clone())
        .collect();
    let manifest = ReleaseManifest {
        schema_version: SCHEMA_VERSION,
        release: ReleaseMetadata {
            version: version.to_owned(),
            target: "x86_64-pc-windows-msvc".to_owned(),
            architecture: "x86_64".to_owned(),
        },
        archive: ArchiveMetadata {
            url: format!("https://example.test/download/v{version}/ah-windows-x64.zip"),
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
            executables: vec!["ah.exe".to_owned()],
            plugins,
        },
    };
    let manifest_bytes = manifest.to_canonical_bytes().unwrap();
    let mut preimage = Vec::from(SIGNING_DOMAIN);
    preimage.extend_from_slice(&manifest_bytes);
    let signature_bytes = URL_SAFE_NO_PAD
        .encode(signing_key.sign(&preimage).to_bytes())
        .into_bytes();
    (manifest, manifest_bytes, signature_bytes)
}

fn trusted_key(signing_key: &SigningKey) -> TrustedKey {
    let public_key = signing_key.verifying_key().to_bytes();
    TrustedKey {
        key_id: key_id_for_public_key(&public_key),
        algorithm: SIGNING_ALGORITHM.to_owned(),
        public_key,
    }
}

fn write_tree(root: &Path, files: &BTreeMap<String, Vec<u8>>) {
    for (relative, bytes) in files {
        let path = root.join(relative.split('/').collect::<PathBuf>());
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }
}

fn assert_tree_matches(root: &Path, files: &BTreeMap<String, Vec<u8>>) {
    for (relative, bytes) in files {
        let path = root.join(relative.split('/').collect::<PathBuf>());
        assert_eq!(fs::read(path).unwrap(), *bytes);
    }
}

fn encode_digest(bytes: impl AsRef<[u8]>) -> String {
    bytes
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}
