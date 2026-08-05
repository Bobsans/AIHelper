use std::{ffi::OsString, path::PathBuf};

use clap::{Arg, ArgAction, ArgMatches, Command, ValueHint, error::ErrorKind, value_parser};
use semver::Version;

use crate::{cli::GlobalOptions, error::AppError, output::OutputMode};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpgradeRequest {
    Check,
    Upgrade,
    Version(Version),
}

#[derive(Debug)]
pub enum EarlyUpgradeRoute {
    NotUpgrade,
    ExitSuccess,
    Execute {
        request: UpgradeRequest,
        options: GlobalOptions,
    },
}

pub fn route(raw_args: &[OsString]) -> Result<EarlyUpgradeRoute, AppError> {
    if command_name(raw_args)?.as_deref() != Some("upgrade") {
        return Ok(EarlyUpgradeRoute::NotUpgrade);
    }
    let matches = match build_early_command().try_get_matches_from(raw_args.iter().cloned()) {
        Ok(matches) => matches,
        Err(error) => match error.kind() {
            ErrorKind::DisplayHelp | ErrorKind::DisplayVersion => {
                error
                    .print()
                    .map_err(|source| AppError::invalid_argument(source.to_string()))?;
                return Ok(EarlyUpgradeRoute::ExitSuccess);
            }
            _ => return Err(AppError::invalid_argument(error.to_string())),
        },
    };
    let options = global_options(&matches)?;
    let Some(("upgrade", upgrade)) = matches.subcommand() else {
        return Ok(EarlyUpgradeRoute::NotUpgrade);
    };
    Ok(EarlyUpgradeRoute::Execute {
        request: request_from_matches(upgrade)?,
        options,
    })
}

pub fn build_help_command() -> Command {
    Command::new("upgrade")
        .about("Check for and install trusted AIHelper releases")
        .arg(
            Arg::new("check")
                .long("check")
                .action(ArgAction::SetTrue)
                .conflicts_with("version")
                .help("Check the highest stable signed release without mutation"),
        )
        .arg(
            Arg::new("version")
                .long("version")
                .value_name("VERSION")
                .value_parser(parse_stable_version)
                .help("Install one exact stable release without downgrade"),
        )
}

pub fn request_from_matches(matches: &ArgMatches) -> Result<UpgradeRequest, AppError> {
    if matches.get_flag("check") {
        Ok(UpgradeRequest::Check)
    } else if let Some(version) = matches.get_one::<Version>("version") {
        Ok(UpgradeRequest::Version(version.clone()))
    } else {
        Ok(UpgradeRequest::Upgrade)
    }
}

fn parse_stable_version(value: &str) -> Result<Version, String> {
    let version = Version::parse(value).map_err(|_| "VERSION must be canonical SemVer")?;
    if !version.pre.is_empty() || !version.build.is_empty() || version.to_string() != value {
        return Err("VERSION must be canonical stable SemVer".to_owned());
    }
    Ok(version)
}

fn build_early_command() -> Command {
    Command::new("ah")
        .disable_version_flag(true)
        .arg(
            Arg::new("json")
                .long("json")
                .action(ArgAction::SetTrue)
                .global(true),
        )
        .arg(
            Arg::new("quiet")
                .long("quiet")
                .action(ArgAction::SetTrue)
                .global(true),
        )
        .arg(
            Arg::new("cwd")
                .long("cwd")
                .value_name("PATH")
                .value_hint(ValueHint::DirPath)
                .value_parser(value_parser!(PathBuf))
                .global(true),
        )
        .arg(
            Arg::new("limit")
                .long("limit")
                .value_name("N")
                .value_parser(value_parser!(usize))
                .global(true),
        )
        .subcommand(build_help_command())
}

fn global_options(matches: &ArgMatches) -> Result<GlobalOptions, AppError> {
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

fn command_name(raw_args: &[OsString]) -> Result<Option<String>, AppError> {
    let mut index = 1;
    while index < raw_args.len() {
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
        return Ok(Some(value.to_owned()));
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routes_check_with_globals_in_any_supported_position() {
        let route = route(&[
            OsString::from("ah"),
            OsString::from("--json"),
            OsString::from("upgrade"),
            OsString::from("--check"),
            OsString::from("--quiet"),
        ])
        .unwrap();
        let EarlyUpgradeRoute::Execute { request, options } = route else {
            panic!("unexpected route")
        };
        assert_eq!(request, UpgradeRequest::Check);
        assert_eq!(options.output, OutputMode::Json);
        assert!(options.quiet);
    }

    #[test]
    fn routes_stable_update_and_rejects_unknown_operation() {
        let EarlyUpgradeRoute::Execute { request, .. } =
            route(&[OsString::from("ah"), OsString::from("upgrade")]).unwrap()
        else {
            panic!("unexpected route")
        };
        assert_eq!(request, UpgradeRequest::Upgrade);

        let EarlyUpgradeRoute::Execute { request, .. } = route(&[
            OsString::from("ah"),
            OsString::from("upgrade"),
            OsString::from("--version"),
            OsString::from("1.2.3"),
        ])
        .unwrap() else {
            panic!("unexpected route")
        };
        assert_eq!(request, UpgradeRequest::Version(Version::new(1, 2, 3)));

        assert!(
            route(&[
                OsString::from("ah"),
                OsString::from("upgrade"),
                OsString::from("--rollback"),
            ])
            .is_err()
        );
    }
}
