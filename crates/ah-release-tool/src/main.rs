use std::path::PathBuf;
use std::process::ExitCode;

use ah_release_tool::{ReleaseRequest, SigningMaterial, sign_release_set, validate_archive_set};
use clap::{Parser, Subcommand};
use zeroize::Zeroizing;

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
    /// Generate and sign the complete supported release asset set.
    SignRelease {
        #[arg(long)]
        assets_dir: PathBuf,
        #[arg(long)]
        output_dir: PathBuf,
        #[arg(long)]
        repository: String,
        #[arg(long)]
        tag: String,
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
        Command::SignRelease {
            assets_dir,
            output_dir,
            repository,
            tag,
        } => {
            let seed = Zeroizing::new(
                std::env::var("AIHELPER_RELEASE_ED25519_SEED_B64URL").map_err(|_| {
                    ah_release_tool::ReleaseToolError::InvalidSigningConfiguration {
                        detail: "signing seed environment variable is missing".to_owned(),
                    }
                })?,
            );
            let public_key =
                std::env::var("AIHELPER_RELEASE_ED25519_PUBLIC_KEY_B64URL").map_err(|_| {
                    ah_release_tool::ReleaseToolError::InvalidSigningConfiguration {
                        detail: "public key environment variable is missing".to_owned(),
                    }
                })?;
            let signing = SigningMaterial::parse(&seed, &public_key)?;
            let outputs = sign_release_set(
                &ReleaseRequest {
                    assets_dir,
                    output_dir,
                    repository,
                    tag,
                },
                &signing,
            )?;
            println!("prepared signed release assets: {}", outputs.len());
        }
    }
    Ok(())
}
