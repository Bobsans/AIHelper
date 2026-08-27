//! The command line one release's `ah` uses to drive another release's helper.
//!
//! This is the other half of invariant 4, and the more fragile half: the helper
//! validates its arguments *by position*, so the contract is the exact argv
//! below and not just the set of flag names. An installed v1.4 helper may be
//! handed off to by a newly installed `ah`, and a v1.5 helper may be launched
//! by the `ah` that is being replaced, so both ends have to keep passing and
//! accepting precisely this.
//!
//! What this covers is "an older `ah` drives today's helper", which is a real
//! test because today's parser runs. The other direction - today's `ah` driving
//! an older helper - cannot run the old parser, and the emitters
//! (`ah_updater::activate`, `ah_updater::recovery`, `ah_updater::handoff`)
//! build the argv from literals rather than from a shared list. Until they take
//! it from one place, a change there is caught by nothing but review; that is
//! the remaining gap in this row.

use std::ffi::OsString;

use ah_update_helper::recovery_command::{
    RecoveryCommand, parse_activation_arguments, parse_recovery_arguments, parse_rollback_arguments,
};
use ah_updater_core::UpdaterError;

type Parse = fn(&[OsString]) -> Result<RecoveryCommand, UpdaterError>;

/// The flags, in the order the handoff passes them. Frozen: the helper reads
/// them by index.
const HANDOFF_FLAGS: [&str; 6] = [
    "--installation-root",
    "--installation-state-root",
    "--transaction-root",
    "--lifecycle-lock",
    "--lifecycle-lock-handle",
    "--handoff-event",
];

const EVENT: &str = "Local\\AIHelper.Update.Handoff.11111111-1111-4111-8111-111111111111";

fn argv(operation: &str) -> Vec<OsString> {
    [
        operation,
        HANDOFF_FLAGS[0],
        "C:\\Program Files\\AIHelper",
        HANDOFF_FLAGS[1],
        "C:\\ProgramData\\AIHelper\\state",
        HANDOFF_FLAGS[2],
        "C:\\ProgramData\\AIHelper\\state\\transaction",
        HANDOFF_FLAGS[3],
        "C:\\ProgramData\\AIHelper\\state\\lifecycle.lock",
        HANDOFF_FLAGS[4],
        "4242",
        HANDOFF_FLAGS[5],
        EVENT,
        "1234",
    ]
    .into_iter()
    .map(OsString::from)
    .collect()
}

#[test]
fn the_frozen_handoff_command_line_is_accepted_for_every_operation() {
    for (operation, parse) in [
        ("activate", parse_activation_arguments as Parse),
        ("rollback", parse_rollback_arguments as Parse),
        ("recover", parse_recovery_arguments as Parse),
    ] {
        let command = parse(&argv(operation))
            .unwrap_or_else(|error| panic!("{operation} should parse: {}", error.code()));
        assert_eq!(
            command.paths.installation_root(),
            std::path::Path::new("C:\\Program Files\\AIHelper"),
            "{operation}"
        );
        assert_eq!(
            command.paths.installation_state_root(),
            std::path::Path::new("C:\\ProgramData\\AIHelper\\state"),
            "{operation}"
        );
        assert_eq!(
            command.paths.transaction_root(),
            std::path::Path::new("C:\\ProgramData\\AIHelper\\state\\transaction"),
            "{operation}"
        );
        assert_eq!(
            command.lifecycle_lock,
            std::path::PathBuf::from("C:\\ProgramData\\AIHelper\\state\\lifecycle.lock"),
            "{operation}"
        );
        assert_eq!(command.lifecycle_lock_handle, 4242, "{operation}");
        assert_eq!(command.handoff_event, EVENT, "{operation}");
        assert_eq!(command.parent_pid, 1234, "{operation}");
    }
}

/// The operation is the first argument, and each parser accepts only its own.
#[test]
fn a_parser_accepts_only_its_own_operation() {
    for operation in ["activate", "rollback", "recover"] {
        let arguments = argv(operation);
        let accepted = [
            parse_activation_arguments(&arguments).is_ok(),
            parse_rollback_arguments(&arguments).is_ok(),
            parse_recovery_arguments(&arguments).is_ok(),
        ];
        assert_eq!(
            accepted.iter().filter(|accepted| **accepted).count(),
            1,
            "{operation} should be accepted by exactly one parser"
        );
    }
}

/// Every position is checked, so a command line that drifts is refused rather
/// than misread - which is what protects a helper from an `ah` that changed the
/// order.
#[test]
fn a_command_line_that_drifts_is_refused() {
    let mut short = argv("activate");
    short.pop();
    assert!(parse_activation_arguments(&short).is_err(), "too few");

    let mut long = argv("activate");
    long.push(OsString::from("extra"));
    assert!(parse_activation_arguments(&long).is_err(), "too many");

    for index in [1, 3, 5, 7, 9, 11] {
        let mut renamed = argv("activate");
        renamed[index] = OsString::from("--something-else");
        assert!(
            parse_activation_arguments(&renamed).is_err(),
            "the flag at {index} is part of the contract"
        );
    }

    let mut reordered = argv("activate");
    reordered.swap(1, 3);
    reordered.swap(2, 4);
    assert!(
        parse_activation_arguments(&reordered).is_err(),
        "the order is part of the contract"
    );
}

/// The three values the helper does not simply copy: a handle it will use, an
/// event name it will open, and the pid it waits for.
#[test]
fn the_values_the_helper_acts_on_are_validated() {
    let cases = [
        (10, "0", "a null handle"),
        (10, "not a number", "a handle that is not a number"),
        (12, "Local\\Something.Else", "a foreign event name"),
        (12, "", "an empty event name"),
        (13, "0", "a null parent pid"),
        (13, "not a number", "a pid that is not a number"),
    ];
    for (index, value, description) in cases {
        let mut arguments = argv("activate");
        arguments[index] = OsString::from(value);
        assert!(
            parse_activation_arguments(&arguments).is_err(),
            "{description} must be refused"
        );
    }

    let mut arguments = argv("activate");
    arguments[12] = OsString::from(format!(
        "Local\\AIHelper.Update.Handoff.{}",
        "x".repeat(128)
    ));
    assert!(
        parse_activation_arguments(&arguments).is_err(),
        "an over-long event name must be refused"
    );
}
