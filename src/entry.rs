//! The parsing that happens before the plugin catalog is known.
//!
//! The full CLI cannot be the first parse: its shape depends on which plugins
//! loaded, and discovery is expensive enough that `ah mcp service status` and
//! `ah upgrade` are answered before it runs. So a second, static parser reads
//! the entry points the host owns.
//!
//! There were two of those, one per early route, each redeclaring the four
//! global flags and each carrying its own hand-rolled argv walker to find the
//! leading positional. They had already drifted: the updater's copy declared
//! `--json`, `--quiet`, `--cwd` and `--limit` with no help text, so
//! `ah upgrade --help` printed four blank descriptions while the golden CLI
//! snapshot - rendered from the *main* command tree, which no user reaches for
//! that command - showed them filled in.
//!
//! One declaration, used by both.

use std::ffi::{OsStr, OsString};

use clap::{Arg, ArgAction, ArgMatches, Command, ValueHint, value_parser};

use crate::{cli::GlobalOptions, error::AppError, output::OutputMode};

/// The static parser the early routes share.
pub(crate) fn early_command() -> Command {
    with_global_flags(Command::new("ah").disable_version_flag(true))
}

/// The four flags every entry point accepts, declared once.
///
/// Three copies of this list existed - the main CLI, the updater route and the
/// managed-service route - and they had already drifted: two of them carried no
/// help text.
pub(crate) fn with_global_flags(command: Command) -> Command {
    command
        .arg(
            Arg::new("json")
                .long("json")
                .action(ArgAction::SetTrue)
                .global(true)
                .help("Return machine-readable JSON output"),
        )
        .arg(
            Arg::new("quiet")
                .long("quiet")
                .action(ArgAction::SetTrue)
                .global(true)
                .help("Suppress command output"),
        )
        .arg(
            Arg::new("cwd")
                .long("cwd")
                .value_name("PATH")
                .value_hint(ValueHint::DirPath)
                .value_parser(value_parser!(std::path::PathBuf))
                .global(true)
                .help("Set working directory"),
        )
        .arg(
            Arg::new("limit")
                .long("limit")
                .value_name("N")
                .value_parser(value_parser!(usize))
                .global(true)
                .help("Cap output lines/items when supported"),
        )
        .arg(
            Arg::new(HANDOFF_FLAG)
                .long(HANDOFF_FLAG)
                .value_name("KIND")
                .value_parser([
                    Handoff::InstalledSmoke.as_str(),
                    Handoff::ManagedRestore.as_str(),
                ])
                .global(true)
                .hide(true)
                .help("Internal: the updater is driving this run"),
        )
}

/// # Errors
///
/// [`AppError`] when `--limit` is zero, which no command can honour.
pub(crate) fn global_options(matches: &ArgMatches) -> Result<GlobalOptions, AppError> {
    let mut options = GlobalOptions {
        output: if matches.get_flag("json") {
            OutputMode::Json
        } else {
            OutputMode::Text
        },
        quiet: matches.get_flag("quiet"),
        limit: matches.get_one::<usize>("limit").copied(),
        cwd: None,
    };
    if options.limit == Some(0) {
        return Err(AppError::invalid_argument("--limit must be >= 1"));
    }
    options.cwd = matches
        .get_one::<std::path::PathBuf>("cwd")
        .cloned()
        .map(crate::cli::resolve_request_dir)
        .transpose()?;
    Ok(options)
}

/// The first `max` positional arguments, skipping the global flags and their
/// values.
///
/// This exists because the early routes have to know *which* command was asked
/// for before they can decide whether to build a parser for it. It knows the
/// global flag set, which is why that set is declared once above: a flag added
/// there and forgotten here would make the walker treat its value as a command
/// name.
///
/// # Errors
///
/// [`AppError`] for non-Unicode arguments, or a global flag left without its
/// value at the end of argv.
pub(crate) fn leading_positionals(
    raw_args: &[OsString],
    max: usize,
) -> Result<Vec<String>, AppError> {
    let mut found = Vec::new();
    let mut index = 1;
    while index < raw_args.len() && found.len() < max {
        let value = raw_args[index]
            .to_str()
            .ok_or_else(|| AppError::invalid_argument("command arguments must be valid Unicode"))?;
        if matches!(value, "--json" | "--quiet") {
            index += 1;
            continue;
        }
        if matches!(value, "--cwd" | "--limit") {
            if index + 1 >= raw_args.len() {
                return Err(AppError::invalid_argument(format!(
                    "missing value for trailing {value}"
                )));
            }
            index += 2;
            continue;
        }
        if value.starts_with("--cwd=") || value.starts_with("--limit=") {
            index += 1;
            continue;
        }
        if value.starts_with('-') {
            index += 1;
            continue;
        }
        found.push(value.to_owned());
        index += 1;
    }
    Ok(found)
}

/// The hidden flag the updater uses to say a run is an internal handoff.
///
/// Hidden rather than absent: a handoff is not something a person should invoke,
/// but it *is* a contract between two of our binaries, and a contract belongs in
/// the parser rather than in the environment.
pub(crate) const HANDOFF_FLAG: &str = "internal-handoff";

/// Why the updater is running this process rather than a person.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Handoff {
    /// Proving a freshly installed binary starts at all.
    InstalledSmoke,
    /// Reinstating the managed MCP service around an update.
    ManagedRestore,
}

impl Handoff {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::InstalledSmoke => "installed-smoke",
            Self::ManagedRestore => "managed-restore",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value {
            "installed-smoke" => Some(Self::InstalledSmoke),
            "managed-restore" => Some(Self::ManagedRestore),
            _ => None,
        }
    }

    /// The variable this handoff used before the flag existed.
    ///
    /// Kept because the two ends ship separately: a helper from one release may
    /// activate, fail, roll back, and then drive the *previous* `ah`, which
    /// knows only the variable. The flag cannot replace it until every supported
    /// `ah` understands the flag.
    fn legacy_variable(self) -> &'static str {
        match self {
            Self::InstalledSmoke => "AH_UPDATER_INSTALLED_SMOKE",
            Self::ManagedRestore => "AH_UPDATER_MCP_RESTORE",
        }
    }
}

/// What the process must know before it can decide what to build.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Startup {
    /// `--version` or `-V` on their own, answered without loading anything.
    pub(crate) version_only: bool,
    pub(crate) handoff: Option<Handoff>,
    /// A managed `mcp serve`, which crash recovery treats differently.
    pub(crate) managed_serve: bool,
}

/// Read the startup decision out of argv and the environment.
///
/// These were four predicates scattered through `run()`, each scanning raw argv
/// by its own rules. The restore handoff matched `raw_args.len() == 5` and
/// compared positions, so a reordered flag, an added one, or an `=`-style
/// argument silently turned the path off.
///
/// # Errors
///
/// [`AppError`] for argv the walker cannot read, or an unknown handoff kind.
pub(crate) fn detect(raw_args: &[OsString]) -> Result<Startup, AppError> {
    detect_in(raw_args, &|name| std::env::var_os(name))
}

/// The same decision against a supplied environment.
///
/// The lookup is a parameter so a test never sets a process-global variable.
/// Two tests that do race each other - which is the problem finding 3.4
/// describes one level up, about the working directory.
///
/// # Errors
///
/// As [`detect`].
fn detect_in(
    raw_args: &[OsString],
    environment: &dyn Fn(&str) -> Option<OsString>,
) -> Result<Startup, AppError> {
    let positionals = leading_positionals(raw_args, 3)?;
    let version_only = raw_args.len() == 2
        && raw_args[1]
            .to_str()
            .is_some_and(|argument| matches!(argument, "--version" | "-V"));

    Ok(Startup {
        version_only,
        handoff: handoff(raw_args, &positionals, version_only, environment)?,
        managed_serve: leads_with(&positionals, &["mcp", "serve"])
            && has_flag(raw_args, "--managed-config"),
    })
}

fn handoff(
    raw_args: &[OsString],
    positionals: &[String],
    version_only: bool,
    environment: &dyn Fn(&str) -> Option<OsString>,
) -> Result<Option<Handoff>, AppError> {
    if let Some(value) = flag_value(raw_args, &format!("--{HANDOFF_FLAG}"))? {
        return Handoff::parse(&value).map(Some).ok_or_else(|| {
            AppError::invalid_argument(format!("unknown --{HANDOFF_FLAG} value '{value}'"))
        });
    }

    // The variables carry no argument, so each keeps the narrowing its
    // hand-written predicate had: one that leaks into an unrelated invocation
    // must not change what that invocation does. The narrowing is now stated
    // against parsed positionals rather than argv offsets.
    for candidate in [Handoff::InstalledSmoke, Handoff::ManagedRestore] {
        if environment(candidate.legacy_variable()).as_deref() != Some(OsStr::new("1")) {
            continue;
        }
        let shape = match candidate {
            Handoff::InstalledSmoke => version_only,
            Handoff::ManagedRestore => {
                leads_with(positionals, &["mcp", "service"])
                    && matches!(
                        positionals.get(2).map(String::as_str),
                        Some("install" | "start" | "status")
                    )
            }
        };
        if shape {
            return Ok(Some(candidate));
        }
    }
    Ok(None)
}

fn leads_with(positionals: &[String], expected: &[&str]) -> bool {
    positionals.len() >= expected.len()
        && positionals
            .iter()
            .zip(expected)
            .all(|(found, want)| found == want)
}

fn has_flag(raw_args: &[OsString], flag: &str) -> bool {
    let assigned = format!("{flag}=");
    raw_args.iter().any(|argument| {
        argument == flag
            || argument
                .to_str()
                .is_some_and(|value| value.starts_with(&assigned))
    })
}

/// The value of `--flag value` or `--flag=value`, if present.
///
/// # Errors
///
/// [`AppError`] when the flag ends argv with nothing after it.
fn flag_value(raw_args: &[OsString], flag: &str) -> Result<Option<String>, AppError> {
    let assigned = format!("{flag}=");
    for (index, argument) in raw_args.iter().enumerate().skip(1) {
        let Some(value) = argument.to_str() else {
            continue;
        };
        if value == flag {
            return raw_args
                .get(index + 1)
                .and_then(|value| value.to_str())
                .map(str::to_owned)
                .map(Some)
                .ok_or_else(|| {
                    AppError::invalid_argument(format!("missing value for trailing {flag}"))
                });
        }
        if let Some(rest) = value.strip_prefix(&assigned) {
            return Ok(Some(rest.to_owned()));
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(values: &[&str]) -> Vec<OsString> {
        values.iter().map(OsString::from).collect()
    }

    /// An environment with nothing set.
    fn nothing(_: &str) -> Option<OsString> {
        None
    }

    /// An environment with exactly one variable set to `1`.
    fn only(name: &'static str) -> impl Fn(&str) -> Option<OsString> {
        move |asked: &str| (asked == name).then(|| OsString::from("1"))
    }

    #[test]
    fn the_version_shortcut_is_only_a_bare_invocation() {
        for line in [vec!["ah", "--version"], vec!["ah", "-V"]] {
            assert!(
                detect_in(&argv(&line), &nothing).unwrap().version_only,
                "{line:?}"
            );
        }
        for line in [
            vec!["ah", "--version", "x"],
            vec!["ah", "--json", "--version"],
        ] {
            assert!(
                !detect_in(&argv(&line), &nothing).unwrap().version_only,
                "{line:?}"
            );
        }
    }

    #[test]
    fn a_managed_serve_is_recognised_in_either_spelling() {
        for line in [
            vec!["ah", "mcp", "serve", "--managed-config", "d.json"],
            vec!["ah", "mcp", "serve", "--managed-config=d.json"],
            vec!["ah", "--json", "mcp", "serve", "--managed-config=d.json"],
        ] {
            assert!(
                detect_in(&argv(&line), &nothing).unwrap().managed_serve,
                "{line:?}"
            );
        }
        for line in [
            vec!["ah", "mcp", "serve"],
            vec!["ah", "mcp", "service", "status", "--managed-config=d"],
        ] {
            assert!(
                !detect_in(&argv(&line), &nothing).unwrap().managed_serve,
                "{line:?}"
            );
        }
    }

    #[test]
    fn the_flag_names_the_handoff_without_an_environment_variable() {
        assert_eq!(
            detect_in(
                &argv(&["ah", "--internal-handoff", "installed-smoke", "--version"]),
                &nothing,
            )
            .unwrap()
            .handoff,
            Some(Handoff::InstalledSmoke)
        );
        assert_eq!(
            detect_in(
                &argv(&["ah", "--internal-handoff=managed-restore"]),
                &nothing
            )
            .unwrap()
            .handoff,
            Some(Handoff::ManagedRestore)
        );
        assert!(detect_in(&argv(&["ah", "--internal-handoff=nonsense"]), &nothing).is_err());
        assert!(detect_in(&argv(&["ah", "--internal-handoff"]), &nothing).is_err());
    }

    /// The old predicate compared argv positions and required exactly five
    /// arguments, so `--json` written after the command turned the path off.
    /// The helper's own line still works, and so does a reordered one.
    #[test]
    fn the_restore_variable_no_longer_depends_on_argument_order() {
        let restore = only("AH_UPDATER_MCP_RESTORE");
        for line in [
            vec!["ah", "--json", "mcp", "service", "install"],
            vec!["ah", "mcp", "service", "install", "--json"],
            vec!["ah", "mcp", "service", "status"],
        ] {
            assert_eq!(
                detect_in(&argv(&line), &restore).unwrap().handoff,
                Some(Handoff::ManagedRestore),
                "{line:?}"
            );
        }
    }

    /// A variable that leaks into an unrelated invocation must not change it.
    #[test]
    fn a_leaked_variable_captures_nothing() {
        assert_eq!(
            detect_in(
                &argv(&["ah", "--json", "plugins", "list"]),
                &only("AH_UPDATER_MCP_RESTORE"),
            )
            .unwrap()
            .handoff,
            None
        );
        assert_eq!(
            detect_in(
                &argv(&["ah", "--json", "plugins", "list"]),
                &only("AH_UPDATER_INSTALLED_SMOKE"),
            )
            .unwrap()
            .handoff,
            None
        );
        assert_eq!(
            detect_in(
                &argv(&["ah", "--version"]),
                &only("AH_UPDATER_INSTALLED_SMOKE")
            )
            .unwrap()
            .handoff,
            Some(Handoff::InstalledSmoke)
        );
    }

    #[test]
    fn global_flags_are_skipped_in_every_supported_position() {
        assert_eq!(
            leading_positionals(&argv(&["ah", "--json", "mcp", "service", "status"]), 2).unwrap(),
            ["mcp", "service"]
        );
        assert_eq!(
            leading_positionals(&argv(&["ah", "--cwd", "/tmp", "upgrade"]), 1).unwrap(),
            ["upgrade"]
        );
        assert_eq!(
            leading_positionals(&argv(&["ah", "--limit=5", "upgrade"]), 1).unwrap(),
            ["upgrade"]
        );
    }

    #[test]
    fn a_trailing_flag_without_its_value_is_an_error_not_a_command() {
        assert!(leading_positionals(&argv(&["ah", "--cwd"]), 1).is_err());
        assert!(leading_positionals(&argv(&["ah", "--limit"]), 1).is_err());
    }

    /// The walker and the parser have to agree on what a global flag is; the
    /// two used to be declared separately and drifted.
    #[test]
    fn the_walker_knows_every_global_the_parser_declares() {
        let declared: Vec<String> = early_command()
            .get_arguments()
            .filter(|argument| argument.is_global_set())
            .filter_map(|argument| argument.get_long())
            .map(|long| format!("--{long}"))
            .collect();

        for flag in &declared {
            let taking_value = matches!(flag.as_str(), "--cwd" | "--limit");
            let line = if taking_value {
                argv(&["ah", flag, "value", "upgrade"])
            } else {
                argv(&["ah", flag, "upgrade"])
            };
            assert_eq!(
                leading_positionals(&line, 1).unwrap(),
                ["upgrade"],
                "{flag} is not skipped by the walker"
            );
        }
    }
}
