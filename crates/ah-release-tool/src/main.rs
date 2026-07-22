use std::path::PathBuf;
use std::process::ExitCode;

use ah_release_tool::validate_archive_set;
use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(name = "ah-release-tool", about = "Internal AIHelper release tooling")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Validate the exact supported release ZIP set without signing.
    ValidateArchives {
        #[arg(long)]
        assets_dir: PathBuf,
    },
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Result<(), ah_release_tool::ReleaseToolError> {
    match cli.command {
        Command::ValidateArchives { assets_dir } => {
            let inventories = validate_archive_set(&assets_dir)?;
            println!(
                "validated release archives: {}",
                inventories
                    .iter()
                    .map(|inventory| inventory.profile.asset_name)
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
    }
    Ok(())
}
