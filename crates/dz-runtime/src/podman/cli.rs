//! Command-line interface for managing Dangerzone Podman machines.
//!
//! Corresponds to `dangerzone/podman/cli.py`. The `click` group is translated
//! to a `clap` command tree, and the `requires_wsl` decorator runs the WSL
//! install startup task before the wrapped command executes.

use std::io::Write;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use dz_core::startup::{StartupLogic, WSLInstallTask};
use log::LevelFilter;

use super::errors::PodmanError;
use super::machine::PodmanMachineManager;

/// The `dz-podman` command-line interface.
#[derive(Debug, Parser)]
#[command(name = "dz-podman", about = "Manage Dangerzone Podman machines.")]
struct Cli {
    /// Set the logging level.
    #[arg(
        long,
        default_value = "info",
        value_parser = ["debug", "info", "warning", "error", "critical"]
    )]
    log_level: String,

    #[command(subcommand)]
    command: Command,
}

/// The `dz-podman` subcommands.
#[derive(Debug, Subcommand)]
enum Command {
    /// List Dangerzone Podman machines.
    List,
    /// Initialize a Dangerzone Podman machine.
    Init {
        /// Number of CPUs to allocate.
        #[arg(long)]
        cpus: Option<u64>,
        /// Amount of memory in bytes.
        #[arg(long)]
        memory: Option<u64>,
        /// Timezone for the machine.
        #[arg(long, default_value = "Etc/UTC")]
        timezone: String,
    },
    /// Start the Dangerzone Podman machine.
    Start,
    /// Stop the Dangerzone Podman machine.
    Stop,
    /// Remove the Dangerzone Podman machine.
    Remove {
        /// Force removal without prompt.
        #[arg(short, long)]
        force: bool,
    },
    /// Reset all Podman machines.
    Reset {
        /// Force reset without prompt.
        #[arg(short, long)]
        force: bool,
    },
    /// Run a raw Podman command.
    Raw {
        /// The Podman arguments to pass through.
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
}

/// Failure modes of the `dz-podman` subcommands.
enum CliFailure {
    /// A Podman error, which is echoed to the user.
    Podman(PodmanError),
    /// The user declined a confirmation prompt.
    Aborted,
}

/// Runs the WSL installation startup task, if required.
///
/// Corresponds to the `requires_wsl` decorator.
fn requires_wsl() -> Result<(), PodmanError> {
    let runner = StartupLogic::new_startup(vec![Box::new(WSLInstallTask)], true);
    runner
        .run()
        .map_err(|error| PodmanError(format!("WSL is required: {error}")))
}

/// Lists the Dangerzone Podman machines.
fn cmd_list() -> Result<(), CliFailure> {
    let manager = PodmanMachineManager::new();
    let machines = manager.list().map_err(CliFailure::Podman)?;
    if machines.is_empty() {
        println!("No Dangerzone Podman machines found.");
    } else {
        for machine in machines {
            let running_status = if machine.running {
                "Running"
            } else {
                "Stopped"
            };
            println!("Name: {}, Status: {running_status}", machine.name);
        }
    }
    Ok(())
}

/// Initializes the Dangerzone Podman machine.
fn cmd_init(cpus: Option<u64>, memory: Option<u64>, timezone: &str) -> Result<(), CliFailure> {
    requires_wsl().map_err(CliFailure::Podman)?;
    let manager = PodmanMachineManager::new();
    manager
        .init(cpus, memory, timezone)
        .map_err(CliFailure::Podman)?;
    println!("Machine initialized: {}", manager.name);
    Ok(())
}

/// Starts the Dangerzone Podman machine.
fn cmd_start() -> Result<(), CliFailure> {
    requires_wsl().map_err(CliFailure::Podman)?;
    let manager = PodmanMachineManager::new();
    manager.start().map_err(CliFailure::Podman)?;
    println!("Machine started: {}", manager.name);
    Ok(())
}

/// Stops the Dangerzone Podman machine.
fn cmd_stop() -> Result<(), CliFailure> {
    requires_wsl().map_err(CliFailure::Podman)?;
    let manager = PodmanMachineManager::new();
    manager.stop().map_err(CliFailure::Podman)?;
    println!("Machine stopped: {}", manager.name);
    Ok(())
}

/// Removes the Dangerzone Podman machine.
fn cmd_remove(force: bool) -> Result<(), CliFailure> {
    let manager = PodmanMachineManager::new();
    if !force
        && !confirm(&format!(
            "Are you sure you want to remove machine '{}'?",
            manager.name
        ))
    {
        return Err(CliFailure::Aborted);
    }
    manager.remove().map_err(CliFailure::Podman)?;
    println!("Machine removed: {}", manager.name);
    Ok(())
}

/// Resets all Podman machines.
fn cmd_reset(force: bool) -> Result<(), CliFailure> {
    if !force
        && !confirm(
            "Are you sure you want to reset all Podman machines? This is a destructive action.",
        )
    {
        return Err(CliFailure::Aborted);
    }
    let manager = PodmanMachineManager::new();
    manager.reset().map_err(CliFailure::Podman)?;
    println!("Podman machines reset.");
    Ok(())
}

/// Runs a raw Podman command.
fn cmd_raw(args: &[String]) -> Result<(), CliFailure> {
    let manager = PodmanMachineManager::new();
    manager
        .run_raw_podman_command(args)
        .map_err(CliFailure::Podman)?;
    Ok(())
}

/// Dispatches to the requested subcommand.
fn run(cli: Cli) -> Result<(), CliFailure> {
    match cli.command {
        Command::List => cmd_list(),
        Command::Init {
            cpus,
            memory,
            timezone,
        } => cmd_init(cpus, memory, &timezone),
        Command::Start => cmd_start(),
        Command::Stop => cmd_stop(),
        Command::Remove { force } => cmd_remove(force),
        Command::Reset { force } => cmd_reset(force),
        Command::Raw { args } => cmd_raw(&args),
    }
}

/// Prompts the user for a yes/no answer, aborting on an empty or negative
/// answer. Returns `true` only for an explicit yes.
fn confirm(prompt: &str) -> bool {
    print!("{prompt} [y/N]: ");
    let _ = std::io::stdout().flush();
    let mut line = String::new();
    if std::io::stdin().read_line(&mut line).is_err() {
        return false;
    }
    matches!(line.trim().to_lowercase().as_str(), "y" | "yes")
}

/// Maps a log level string to a [`LevelFilter`].
fn parse_log_level(level: &str) -> LevelFilter {
    match level {
        "debug" => LevelFilter::Debug,
        "warning" => LevelFilter::Warn,
        "error" => LevelFilter::Error,
        "critical" => LevelFilter::Error,
        _ => LevelFilter::Info,
    }
}

/// Entry point of the `dz-podman` binary.
pub fn main() -> ExitCode {
    let cli = Cli::parse();

    let mut builder = env_logger::Builder::new();
    builder.filter_level(parse_log_level(&cli.log_level));
    let _ = builder.try_init();

    match run(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(CliFailure::Podman(error)) => {
            println!("❌ {error}");
            ExitCode::FAILURE
        }
        Err(CliFailure::Aborted) => ExitCode::FAILURE,
    }
}
