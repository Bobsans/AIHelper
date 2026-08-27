//! The on-disk and cross-process forms of an update, frozen.
//!
//! Invariant 4 of the refactoring program: an installed helper from one release
//! may hand off to a newly installed `ah`, and a new helper may have to recover
//! a transaction an older one left behind. Both directions are covered here,
//! and they need different kinds of test:
//!
//! - **Reading what an older release wrote** is a real test: the fixtures next
//!   to this file are the v1 wire form, and today's types parse and validate
//!   them.
//! - **Writing what an older release can read** cannot run the old code, so it
//!   is a byte comparison. Both types carry `deny_unknown_fields`, which means
//!   an older reader *rejects* a document with a field it does not know. So the
//!   rule is strict: a new field must be `skip_serializing_if` so that it is
//!   absent whenever it is unset, and the fixtures below must keep matching
//!   byte for byte.
//!
//! A failure here is not a test to fix. It means the on-disk format changed,
//! and the change needs either a `skip_serializing_if` default or a schema
//! version bump with a compatibility window.

use ah_updater_core::{
    FilePurpose, ManagedFile, ManagedFileOperationV1, TransactionJournalV1, TransactionPlanV1,
    TransactionStateV1, UpdateHelperSelfCheckV1, UpdateOperation,
};
use uuid::Uuid;

const TRANSACTION_ID: &str = "11111111-1111-4111-8111-111111111111";
const INSTALLATION_ID: &str = "22222222-2222-4222-8222-222222222222";
const PREVIOUS_INSTANCE_ID: &str = "33333333-3333-4333-8333-333333333333";

fn managed_file(path: &str, size: u64, byte: u8, purpose: FilePurpose) -> ManagedFile {
    ManagedFile {
        path: path.to_owned(),
        size,
        sha256: std::iter::repeat_n(format!("{byte:02x}"), 32).collect::<String>(),
        purpose,
    }
}

/// The plan an upgrade writes: no optional field set, which is the form an
/// older reader has to be able to parse.
fn upgrade_plan() -> TransactionPlanV1 {
    TransactionPlanV1 {
        schema_version: 1,
        transaction_id: Uuid::parse_str(TRANSACTION_ID).unwrap(),
        installation_id: Uuid::parse_str(INSTALLATION_ID).unwrap(),
        old_version: "1.3.0".to_owned(),
        new_version: "1.4.0".to_owned(),
        target: "x86_64-pc-windows-msvc".to_owned(),
        architecture: "x86_64".to_owned(),
        old_manifest_sha256: std::iter::repeat_n("aa", 32).collect(),
        new_manifest_sha256: std::iter::repeat_n("bb", 32).collect(),
        operation: UpdateOperation::Upgrade,
        managed_mcp_was_running: false,
        managed_mcp_previous_instance_id: None,
        operations: vec![
            ManagedFileOperationV1::Replace {
                old: managed_file("ah.exe", 1024, 0x11, FilePurpose::Executable),
                new: managed_file("ah.exe", 2048, 0x22, FilePurpose::Executable),
            },
            ManagedFileOperationV1::Add {
                new: managed_file("ah_plugin_github.dll", 512, 0x33, FilePurpose::Plugin),
            },
            ManagedFileOperationV1::Remove {
                old: managed_file("ah_plugin_old.dll", 256, 0x44, FilePurpose::Plugin),
            },
        ],
    }
}

/// The plan a rollback writes while the managed service was running, which is
/// the only form where every optional field is present.
fn rollback_plan() -> TransactionPlanV1 {
    TransactionPlanV1 {
        old_version: "1.4.0".to_owned(),
        new_version: "1.3.0".to_owned(),
        operation: UpdateOperation::Rollback,
        managed_mcp_was_running: true,
        managed_mcp_previous_instance_id: Some(Uuid::parse_str(PREVIOUS_INSTANCE_ID).unwrap()),
        ..upgrade_plan()
    }
}

fn journal(
    plan: &TransactionPlanV1,
    state: TransactionStateV1,
    sequence: u32,
) -> TransactionJournalV1 {
    TransactionJournalV1 {
        schema_version: 1,
        transaction_id: plan.transaction_id,
        installation_id: plan.installation_id,
        plan_sha256: plan.sha256().unwrap(),
        state,
        transition_sequence: sequence,
    }
}

/// Compare against the fixture, or write it when `AH_UPDATE_FIXTURES=1`.
///
/// The same escape hatch the golden snapshots use, and for the same reason: a
/// deliberate format change should be reviewable as a diff of the frozen bytes.
fn assert_frozen(name: &str, produced: &str) {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(name);
    if std::env::var_os("AH_UPDATE_FIXTURES").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, format!("{produced}\n")).unwrap();
        return;
    }
    let expected = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("fixture '{}' should be readable: {error}", path.display()));
    assert_eq!(
        produced,
        expected.trim_end_matches('\n'),
        "the wire form of '{name}' changed; an older release reads these bytes"
    );
}

#[test]
fn an_upgrade_plan_serializes_to_the_frozen_wire_form() {
    let produced = String::from_utf8(upgrade_plan().to_canonical_bytes().unwrap()).unwrap();
    assert!(
        !produced.contains("\"operation\":\"upgrade\""),
        "the default operation must stay absent, or an older reader rejects the plan"
    );
    assert!(
        !produced.contains("managed_mcp"),
        "the managed-service fields must stay absent when unset"
    );
    assert_frozen("plan-v1-upgrade.json", &produced);
}

#[test]
fn a_rollback_plan_serializes_to_the_frozen_wire_form() {
    let produced = String::from_utf8(rollback_plan().to_canonical_bytes().unwrap()).unwrap();
    assert_frozen("plan-v1-rollback-managed.json", &produced);
}

#[test]
fn a_journal_serializes_to_the_frozen_wire_form() {
    let plan = upgrade_plan();
    let produced =
        serde_json::to_string(&journal(&plan, TransactionStateV1::CandidateActivated, 3)).unwrap();
    assert_frozen("journal-v1-candidate-activated.json", &produced);
}

#[test]
fn the_helper_self_check_serializes_to_the_frozen_wire_form() {
    let produced = serde_json::to_string(&UpdateHelperSelfCheckV1::new(
        "1.4.0",
        "x86_64-pc-windows-msvc",
        "x86_64",
    ))
    .unwrap();
    assert_frozen("helper-self-check-v1.json", &produced);
}

/// The other direction: today's types read what is on disk, and validate it.
#[test]
fn the_frozen_wire_form_is_read_and_validated_by_todays_types() {
    for (fixture, expected) in [
        ("plan-v1-upgrade.json", upgrade_plan()),
        ("plan-v1-rollback-managed.json", rollback_plan()),
    ] {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("fixtures")
            .join(fixture);
        let bytes = std::fs::read(&path).expect("the fixture should be readable");
        let parsed: TransactionPlanV1 =
            serde_json::from_slice(bytes.trim_ascii_end()).expect("the plan should parse");
        parsed.validate().expect("the plan should validate");
        assert_eq!(parsed, expected, "{fixture}");
    }

    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("journal-v1-candidate-activated.json");
    let bytes = std::fs::read(&path).expect("the fixture should be readable");
    let parsed: TransactionJournalV1 =
        serde_json::from_slice(bytes.trim_ascii_end()).expect("the journal should parse");
    parsed
        .validate_for(&upgrade_plan())
        .expect("the journal should validate against its plan");
    assert_eq!(parsed.state, TransactionStateV1::CandidateActivated);

    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("helper-self-check-v1.json");
    let bytes = std::fs::read(&path).expect("the fixture should be readable");
    let parsed: UpdateHelperSelfCheckV1 =
        serde_json::from_slice(bytes.trim_ascii_end()).expect("the self-check should parse");
    parsed
        .validate("1.4.0", "x86_64-pc-windows-msvc", "x86_64")
        .expect("the self-check should validate");
}

/// Every state the journal can hold has a frozen spelling, because recovery
/// dispatches on it and a renamed discriminant would strand a transaction.
#[test]
fn every_transaction_state_keeps_its_spelling() {
    let spellings = [
        (TransactionStateV1::Planned, "planned"),
        (TransactionStateV1::BackupPrepared, "backup_prepared"),
        (TransactionStateV1::ActivationStarted, "activation_started"),
        (
            TransactionStateV1::CandidateActivated,
            "candidate_activated",
        ),
        (TransactionStateV1::PermanentVerified, "permanent_verified"),
        (TransactionStateV1::CommitStarted, "commit_started"),
        (TransactionStateV1::Committed, "committed"),
        (TransactionStateV1::RollbackStarted, "rollback_started"),
        (TransactionStateV1::RolledBack, "rolled_back"),
    ];
    for (state, spelling) in spellings {
        assert_eq!(
            serde_json::to_string(&state).unwrap(),
            format!("\"{spelling}\""),
            "{state:?}"
        );
        assert_eq!(
            serde_json::from_str::<TransactionStateV1>(&format!("\"{spelling}\"")).unwrap(),
            state
        );
    }
}

/// The operation tag is the discriminant a helper dispatches file work on.
#[test]
fn every_file_operation_keeps_its_tag() {
    for (operation, tag) in [
        (
            ManagedFileOperationV1::Add {
                new: managed_file("a", 1, 0x11, FilePurpose::Executable),
            },
            "add",
        ),
        (
            ManagedFileOperationV1::Replace {
                old: managed_file("a", 1, 0x11, FilePurpose::Executable),
                new: managed_file("a", 2, 0x22, FilePurpose::Executable),
            },
            "replace",
        ),
        (
            ManagedFileOperationV1::Remove {
                old: managed_file("a", 1, 0x11, FilePurpose::Executable),
            },
            "remove",
        ),
    ] {
        let json = serde_json::to_string(&operation).unwrap();
        assert!(
            json.starts_with(&format!("{{\"operation\":\"{tag}\"")),
            "{json}"
        );
    }
}

/// A document from a *newer* release is refused rather than half-read. Both
/// types deny unknown fields, which is what makes the rule above strict.
#[test]
fn a_document_from_a_newer_release_is_refused() {
    let mut plan = serde_json::to_value(upgrade_plan()).unwrap();
    plan["something_new"] = serde_json::json!(true);
    assert!(serde_json::from_value::<TransactionPlanV1>(plan).is_err());

    let plan = upgrade_plan();
    let mut journal = serde_json::to_value(journal(&plan, TransactionStateV1::Planned, 0)).unwrap();
    journal["something_new"] = serde_json::json!(true);
    assert!(serde_json::from_value::<TransactionJournalV1>(journal).is_err());

    let mut plan = serde_json::to_value(upgrade_plan()).unwrap();
    plan["schema_version"] = serde_json::json!(2);
    let parsed: TransactionPlanV1 = serde_json::from_value(plan).expect("the shape still parses");
    assert!(
        parsed.validate().is_err(),
        "a newer schema version must be refused by validation"
    );
}
