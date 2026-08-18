//! Command-line interface for upgrading the Dangerzone sandbox image.
//!
//! Corresponds to `dangerzone/updater/cli.py`. The `click` group is translated
//! to a `clap` command tree, and the `requires_container_runtime` decorator
//! runs the machine startup tasks before a wrapped command and stops the
//! machine afterwards.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use dz_core::shutdown::{MachineStopTask, ShutdownLogic};
use dz_core::startup::{
    MachineInitTask, MachineStartTask, MachineStopOthersTask, StartupLogic, Task, WSLInstallTask,
};
use dz_core::util::get_architecture;
use dz_runtime::container_utils::expected_image_name;

use crate::errors::UpdateError;
use crate::{registry, signatures};

/// The default container registry repository of the Dangerzone image.
pub const DEFAULT_REPOSITORY: &str = "freedomofpress/dangerzone";
/// The default branch of the repository that tracks the release notes.
pub const DEFAULT_BRANCH: &str = "main";
/// The name of the Dangerzone container image, derived from the host
/// architecture.
pub fn default_image_name() -> String {
    expected_image_name()
}

/// The `dz-update-image` command-line interface.
#[derive(Debug, Parser)]
#[command(
    name = "dangerzone-image",
    about = "Manage the Dangerzone sandbox image."
)]
struct Cli {
    /// Enable debug logging.
    #[arg(long)]
    debug: bool,

    #[command(subcommand)]
    command: Command,
}

/// The `dz-update-image` subcommands.
#[derive(Debug, Subcommand)]
enum Command {
    /// Upgrade the sandbox to the latest version available.
    ///
    /// To upgrade to a custom sandbox image, use "prepare-archive" and
    /// "load-archive" instead.
    Upgrade,
    /// Retrieve and store the signatures of the remote sandbox.
    StoreSignatures {
        /// The sandbox container registry location.
        #[arg(long, default_value_t = default_image_name())]
        image: String,
    },
    /// Use ARCHIVE_FILENAME as the Dangerzone sandbox image.
    LoadArchive {
        /// The archive file to install.
        #[arg(value_name = "ARCHIVE_FILENAME")]
        archive_filename: PathBuf,
        /// Force the installation, bypassing logindex verification checks.
        #[arg(long)]
        force: bool,
    },
    /// Prepare an archive to upgrade the Dangerzone image (useful for
    /// air-gapped environments).
    PrepareArchive {
        /// The sandbox container registry location.
        #[arg(long, default_value_t = default_image_name())]
        image: String,
        /// The location of the generated archive. '{arch}' will be replaced by
        /// the specified or detected architecture.
        #[arg(long, default_value = "dangerzone-{arch}.tar")]
        output: String,
        /// The architecture to prepare the archive for.
        #[arg(long, default_value_t = get_architecture().to_string())]
        arch: String,
    },
    /// Ensure the local image signature(s) match the embedded public key.
    VerifyLocal {
        /// The name of the image to check signatures for.
        #[arg(long, default_value_t = default_image_name())]
        image: String,
    },
}

/// Failure modes of the `dz-update-image` subcommands.
enum CliFailure {
    /// A container-runtime startup task failed.
    Startup(String),
    /// An update or signature error occurred.
    Update(UpdateError),
    /// The command was aborted after printing an error.
    Aborted,
}

/// Runs the container-runtime startup tasks, executes `f`, and stops the
/// machine afterwards.
///
/// Corresponds to the `requires_container_runtime` decorator: the startup
/// tasks run first (a failure aborts the command), then the command runs, and
/// the machine is always stopped afterwards.
fn requires_container_runtime<T>(
    f: impl FnOnce() -> Result<T, CliFailure>,
) -> Result<T, CliFailure> {
    let tasks: Vec<Box<dyn Task>> = vec![
        Box::new(WSLInstallTask),
        Box::new(MachineStopOthersTask),
        Box::new(MachineInitTask),
        Box::new(MachineStartTask),
    ];
    let result = (|| {
        StartupLogic::new_startup(tasks, true)
            .run()
            .map_err(|error| CliFailure::Startup(error.to_string()))?;
        f()
    })();
    let _ = ShutdownLogic::new_shutdown(vec![Box::new(MachineStopTask)], true).run();
    result
}

/// Upgrades the sandbox to the latest version available.
fn cmd_upgrade() -> Result<(), CliFailure> {
    requires_container_runtime(|| {
        let image = default_image_name();
        let manifest_digest = registry::get_manifest_digest(&image).map_err(CliFailure::Update)?;
        match signatures::upgrade_container_image(&manifest_digest, &image, None) {
            Ok(()) => {
                println!("✅ The local image {image} has been upgraded");
                println!(
                    "✅ The image has been signed with {}",
                    signatures::default_pubkey_location().display()
                );
                println!("✅ Signatures have been verified and stored locally");
                Ok(())
            }
            Err(UpdateError::ImageAlreadyUpToDate) => {
                println!("✅ The local image {image}@{manifest_digest} is already up to date");
                Ok(())
            }
            Err(error) => Err(CliFailure::Update(error)),
        }
    })
}

/// Retrieves and stores the signatures of the remote sandbox.
fn cmd_store_signatures(image: &str) -> Result<(), CliFailure> {
    let manifest_digest = registry::get_manifest_digest(image).map_err(CliFailure::Update)?;
    let sigs =
        signatures::get_remote_signatures(image, &manifest_digest).map_err(CliFailure::Update)?;
    signatures::verify_signatures(&sigs, &manifest_digest).map_err(CliFailure::Update)?;
    signatures::store_signatures(&sigs, &manifest_digest, false).map_err(CliFailure::Update)?;
    println!("✅ Signatures have been verified and stored locally");
    Ok(())
}

/// Uses `archive_filename` as the Dangerzone sandbox image.
fn cmd_load_archive(archive_filename: &Path, force: bool) -> Result<(), CliFailure> {
    requires_container_runtime(|| {
        match signatures::upgrade_container_image_airgapped(archive_filename, force) {
            Ok((loaded_image, image_digest)) => {
                println!(
                    "✅ Installed image {} on the system as {loaded_image} with digest \
                     {image_digest}",
                    archive_filename.display()
                );
                Ok(())
            }
            Err(error @ UpdateError::ImageAlreadyUpToDate) => {
                println!("✅ {error}");
                Ok(())
            }
            Err(UpdateError::InvalidLogIndex(_)) => {
                println!("❌ Trying to install image older that the currently installed one");
                Err(CliFailure::Aborted)
            }
            Err(UpdateError::Signature(_)) => {
                println!("❌ Failed to verify the signatures.");
                Err(CliFailure::Aborted)
            }
            Err(error) => Err(CliFailure::Update(error)),
        }
    })
}

/// Prepares an archive to upgrade the Dangerzone image for air-gapped
/// environments.
fn cmd_prepare_archive(image: &str, output: &str, arch: &str) -> Result<(), CliFailure> {
    let archive = output.replace("{arch}", arch);
    match signatures::prepare_airgapped_archive(image, &archive, arch) {
        Ok(()) => {
            println!("✅ Archive {archive} created");
            Ok(())
        }
        Err(UpdateError::Signature(_)) => {
            println!("❌ Failed to verify the signatures.");
            Err(CliFailure::Aborted)
        }
        Err(error) => Err(CliFailure::Update(error)),
    }
}

/// Ensures the local image signature(s) match the embedded public key.
fn cmd_verify_local(image: &str) -> Result<(), CliFailure> {
    requires_container_runtime(|| {
        println!(
            "Verifying the local image:\n\npubkey: {}\nimage: {image}\n",
            signatures::default_pubkey_location().display()
        );
        if signatures::verify_local_image(image).map_err(CliFailure::Update)? {
            println!("✅ The local image {image} has been signed with the public key");
        }
        Ok(())
    })
}

/// Dispatches to the requested subcommand.
fn run(cli: Cli) -> Result<(), CliFailure> {
    match cli.command {
        Command::Upgrade => cmd_upgrade(),
        Command::StoreSignatures { image } => cmd_store_signatures(&image),
        Command::LoadArchive {
            archive_filename,
            force,
        } => cmd_load_archive(&archive_filename, force),
        Command::PrepareArchive {
            image,
            output,
            arch,
        } => cmd_prepare_archive(&image, &output, &arch),
        Command::VerifyLocal { image } => cmd_verify_local(&image),
    }
}

/// Entry point of the `dz-update-image` binary.
pub fn main() -> ExitCode {
    let cli = Cli::parse();

    let mut builder = env_logger::Builder::new();
    builder.filter_level(if cli.debug {
        log::LevelFilter::Debug
    } else {
        log::LevelFilter::Info
    });
    let _ = builder.try_init();

    if cli.debug {
        println!("Debug mode enabled");
    }

    match run(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(CliFailure::Startup(error)) => {
            println!("❌ {error}");
            ExitCode::FAILURE
        }
        Err(CliFailure::Update(error)) => {
            println!("❌ {error}");
            ExitCode::FAILURE
        }
        Err(CliFailure::Aborted) => ExitCode::FAILURE,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_python_originals() {
        assert_eq!(DEFAULT_REPOSITORY, "freedomofpress/dangerzone");
        assert_eq!(DEFAULT_BRANCH, "main");
    }

    #[test]
    fn default_image_name_is_the_local_sandbox_image() {
        assert_eq!(default_image_name(), "dangerzone-sandbox:latest");
    }
}
