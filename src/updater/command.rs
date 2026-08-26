use std::ffi::OsString;

use clap::{Arg, ArgAction, ArgMatches, Command, error::ErrorKind};
use semver::Version;

use crate::{cli::GlobalOptions, error::AppError};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpgradeRequest {
    Check,
    Upgrade,
    Version(Version),
    Rollback,
}

#[derive(Debug)]
pub enum EarlyUpgradeRoute {
    ExitSuccess,
    Execute {
        request: UpgradeRequest,
        options: GlobalOptions,
    },
}

/// Parse an argv that `entry::detect` has already identified as
/// `upgrade`.
///
/// It used to re-scan argv to find that out for itself, which is why there was
/// a `NotUpgrade` variant: the caller asked every parser in turn whether the
/// command was theirs. One scan decides now, so the question cannot be asked
/// twice and cannot be answered two ways.
///
/// # Errors
///
/// [`AppError`] for arguments `upgrade` does not accept.
pub fn parse(raw_args: &[OsString]) -> Result<EarlyUpgradeRoute, AppError> {
    let matches = match crate::entry::early_command()
        .subcommand(build_help_command())
        .try_get_matches_from(raw_args.iter().cloned())
    {
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
    let options = crate::entry::global_options(&matches)?;
    let Some(("upgrade", upgrade)) = matches.subcommand() else {
        return Err(AppError::invalid_argument(
            "upgrade takes no other subcommand",
        ));
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
                .conflicts_with_all(["version", "rollback"])
                .help("Check the highest stable signed release without mutation"),
        )
        .arg(
            Arg::new("version")
                .long("version")
                .value_name("VERSION")
                .value_parser(parse_stable_version)
                .conflicts_with("rollback")
                .help("Install one exact stable release without downgrade"),
        )
        .arg(
            Arg::new("rollback")
                .long("rollback")
                .action(ArgAction::SetTrue)
                .help("Restore and consume the single verified permanent backup"),
        )
}

pub fn request_from_matches(matches: &ArgMatches) -> Result<UpgradeRequest, AppError> {
    if matches.get_flag("check") {
        Ok(UpgradeRequest::Check)
    } else if matches.get_flag("rollback") {
        Ok(UpgradeRequest::Rollback)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::output::OutputMode;

    #[test]
    fn routes_check_with_globals_in_any_supported_position() {
        let route = parse(&[
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
            parse(&[OsString::from("ah"), OsString::from("upgrade")]).unwrap()
        else {
            panic!("unexpected route")
        };
        assert_eq!(request, UpgradeRequest::Upgrade);

        let EarlyUpgradeRoute::Execute { request, .. } = parse(&[
            OsString::from("ah"),
            OsString::from("upgrade"),
            OsString::from("--version"),
            OsString::from("1.2.3"),
        ])
        .unwrap() else {
            panic!("unexpected route")
        };
        assert_eq!(request, UpgradeRequest::Version(Version::new(1, 2, 3)));

        let EarlyUpgradeRoute::Execute { request, .. } = parse(&[
            OsString::from("ah"),
            OsString::from("upgrade"),
            OsString::from("--rollback"),
        ])
        .unwrap() else {
            panic!("unexpected route")
        };
        assert_eq!(request, UpgradeRequest::Rollback);
    }
}
