//! The command line one release's `ah` uses to drive another release's helper.
//!
//! This is the other half of invariant 4, and the more fragile half: the helper
//! validates its arguments *by position*, so the contract is the exact argv
//! below and not just the set of flag names. An installed v1.4 helper may be
//! handed off to by a newly installed `ah`, and a v1.5 helper may be launched
//! by the `ah` that is being replaced, so both ends have to keep passing and
//! accepting precisely this.
//!
//! Two directions, two kinds of test. *An older `ah` drives today's helper* is
//! real: the frozen argv below is parsed by today's parser. *Today's `ah` drives
//! an older helper* cannot run the old parser, so instead the command line `ah`
//! builds is checked against the one this helper accepts - both come from
//! `ah_updater_core`'s two builders now, and the round-trip below is what proves
//! the emitters and the parser cannot drift apart. What remains untested is the
//! Windows launcher's own handle plumbing, which no in-process test can reach.

use std::ffi::{OsStr, OsString};

use ah_update_helper::recovery_command::{
    RecoveryCommand, parse_activation_arguments, parse_recovery_arguments, parse_rollback_arguments,
};
use ah_updater_core::UpdaterError;

type Parse = fn(&[OsString]) -> Result<RecoveryCommand, UpdaterError>;

/// The flags, in the order the handoff passes them, frozen here so that a
/// change to the shared list has to be made twice - once in the contract and
/// once in this test.
const FROZEN_FLAGS: [&str; 6] = [
    "--installation-root",
    "--installation-state-root",
    "--transaction-root",
    "--lifecycle-lock",
    "--lifecycle-lock-handle",
    "--handoff-event",
];

#[test]
fn the_shared_flag_list_is_the_frozen_one() {
    assert_eq!(ah_updater_core::HANDOFF_FLAGS, FROZEN_FLAGS);
    assert_eq!(ah_updater_core::HANDOFF_ARGUMENT_COUNT, 14);
}

/// The command line `ah` builds is the command line this helper accepts.
///
/// `ah` produces it in two halves, from the two builders in the shared crate:
/// the roots before the launch, and the inherited lease and event during it.
/// Nothing else in this repository can put the halves together, which is why
/// this test lives here rather than with either of them.
#[test]
fn the_command_line_ah_builds_is_the_one_the_helper_accepts() {
    for operation in ["activate", "rollback", "recover"] {
        let paths = ah_updater_core::handoff_paths_arguments(
            OsStr::new(operation),
            OsStr::new(r"C:\Program Files\AIHelper"),
            OsStr::new(r"C:\ProgramData\AIHelper\state"),
            OsStr::new(r"C:\ProgramData\AIHelper\state\transaction"),
        );
        let lease = ah_updater_core::handoff_lease_arguments(
            OsStr::new(r"C:\ProgramData\AIHelper\state\lifecycle.lock"),
            OsStr::new("4242"),
            OsStr::new(EVENT),
            OsStr::new("1234"),
        );
        let built = paths
            .into_iter()
            .chain(lease)
            .map(OsString::from)
            .collect::<Vec<_>>();

        assert_eq!(
            built,
            argv(operation),
            "{operation}: the built command line and the frozen one must agree"
        );
        let parse = match operation {
            "activate" => parse_activation_arguments as Parse,
            "rollback" => parse_rollback_arguments as Parse,
            _ => parse_recovery_arguments as Parse,
        };
        parse(&built).unwrap_or_else(|error| {
            panic!(
                "{operation} should parse what `ah` builds: {}",
                error.code()
            )
        });
    }
}

const EVENT: &str = "Local\\AIHelper.Update.Handoff.11111111-1111-4111-8111-111111111111";

fn argv(operation: &str) -> Vec<OsString> {
    [
        operation,
        FROZEN_FLAGS[0],
        "C:\\Program Files\\AIHelper",
        FROZEN_FLAGS[1],
        "C:\\ProgramData\\AIHelper\\state",
        FROZEN_FLAGS[2],
        "C:\\ProgramData\\AIHelper\\state\\transaction",
        FROZEN_FLAGS[3],
        "C:\\ProgramData\\AIHelper\\state\\lifecycle.lock",
        FROZEN_FLAGS[4],
        "4242",
        FROZEN_FLAGS[5],
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
