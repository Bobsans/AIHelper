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

use std::ffi::OsString;

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
}

/// # Errors
///
/// [`AppError`] when `--limit` is zero, which no command can honour.
pub(crate) fn global_options(matches: &ArgMatches) -> Result<GlobalOptions, AppError> {
    let options = GlobalOptions {
        output: if matches.get_flag("json") {
            OutputMode::Json
        } else {
            OutputMode::Text
        },
        quiet: matches.get_flag("quiet"),
        limit: matches.get_one::<usize>("limit").copied(),
    };
    if options.limit == Some(0) {
        return Err(AppError::invalid_argument("--limit must be >= 1"));
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(values: &[&str]) -> Vec<OsString> {
        values.iter().map(OsString::from).collect()
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
