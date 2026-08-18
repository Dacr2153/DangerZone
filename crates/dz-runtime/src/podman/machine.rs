//! Management of the Dangerzone Podman machine.
//!
//! Corresponds to `dangerzone/podman/machine.py`. Podman machines are only
//! used on macOS and Windows; on Linux, Dangerzone talks to the container
//! runtime directly. Every lifecycle operation shells out to the Podman
//! binary, whose path is resolved by [`crate::container_utils::get_podman_path`]
//! (honouring the `DANGERZONE_PODMAN` override used by the tests).

use std::path::PathBuf;

use super::cli_runner::Runner;
use super::errors::PodmanError;
use crate::container_utils::get_podman_path;

/// The default name of the Dangerzone Podman machine.
pub const DEFAULT_MACHINE_NAME: &str = "dangerzone";

/// A Podman machine as reported by `podman machine list`.
#[derive(Debug, Clone)]
pub struct PodmanMachine {
    /// The machine's name.
    pub name: String,
    /// Whether the machine is currently running.
    pub running: bool,
}

/// A single entry of `podman machine list --format json`.
#[derive(serde::Deserialize)]
struct MachineListEntry {
    #[serde(rename = "Name")]
    name: String,
    #[serde(rename = "Running")]
    running: bool,
}

/// Manages the Dangerzone Podman machine.
pub struct PodmanMachineManager {
    /// The name of the Dangerzone machine.
    pub name: String,
    runner: Runner,
}

impl PodmanMachineManager {
    /// Creates a new manager for the Dangerzone machine.
    pub fn new() -> Self {
        Self::with_name(DEFAULT_MACHINE_NAME.to_string())
    }

    /// Creates a new manager for a machine with the given name.
    pub fn with_name(name: String) -> Self {
        let runner = Runner::new(Some(PathBuf::from(get_podman_path())), false, None, None);
        Self { name, runner }
    }

    /// Builds a `podman machine <subcommand>` argument vector from the
    /// trailing arguments.
    fn machine_args(&self, cmd: &[&str]) -> Vec<String> {
        std::iter::once("machine".to_string())
            .chain(cmd.iter().map(|arg| arg.to_string()))
            .collect()
    }

    /// Runs a `podman` command, raising a [`PodmanError`] on a non-zero exit.
    fn run_checked(&self, args: &[String]) -> Result<(), PodmanError> {
        self.runner
            .run_captured(args, true)
            .map(|_| ())
            .map_err(|error| PodmanError(error.0))
    }

    /// Runs a `podman machine <subcommand> <name>` command.
    fn run_on_machine(&self, subcommand: &str) -> Result<(), PodmanError> {
        let mut args = self.machine_args(&[subcommand]);
        args.push(self.name.clone());
        self.run_checked(&args)
    }

    /// Lists the Podman machines managed by Dangerzone.
    ///
    /// Corresponds to `machine.py:list()`. The machines are read from the
    /// `podman machine list --format json` output; an empty output (no
    /// machines) or unrecognized output yields an empty list.
    pub fn list(&self) -> Result<Vec<PodmanMachine>, PodmanError> {
        let args = self.machine_args(&["list", "--format", "json"]);
        let output = self
            .runner
            .run_captured(&args, false)
            .map_err(|error| PodmanError(error.0))?;
        if output.trim().is_empty() {
            return Ok(Vec::new());
        }
        let entries: Vec<MachineListEntry> = serde_json::from_str(&output).unwrap_or_default();
        Ok(entries
            .into_iter()
            .map(|entry| PodmanMachine {
                name: entry.name,
                running: entry.running,
            })
            .collect())
    }

    /// Lists the Podman machines other than the Dangerzone one that are
    /// currently running.
    ///
    /// Corresponds to `machine.py:list_other_running_machines()`, which the
    /// startup flow consults to ask the user to stop conflicting machines.
    pub fn list_other_running_machines(&self) -> Vec<String> {
        self.list()
            .unwrap_or_default()
            .into_iter()
            .filter(|machine| machine.running && machine.name != self.name)
            .map(|machine| machine.name)
            .collect()
    }

    /// Ensures the Dangerzone machine is running, starting it if needed.
    ///
    /// Corresponds to `machine.py:ensure_running()`. The upstream `--linger`
    /// flag that keeps a rootful machine running is not translated: the
    /// machine is started on demand and stopped on shutdown, which matches how
    /// the rest of this port drives the runtime.
    pub fn ensure_running(&self) -> Result<(), PodmanError> {
        let running = self
            .list()
            .unwrap_or_default()
            .iter()
            .any(|machine| machine.name == self.name && machine.running);
        if running {
            return Ok(());
        }
        self.start()
    }

    /// Initializes the Dangerzone machine.
    ///
    /// Corresponds to `machine.py:init()`. The machine name is passed as the
    /// positional argument, alongside the optional CPU/memory allocation and
    /// the timezone.
    pub fn init(
        &self,
        cpus: Option<u64>,
        memory: Option<u64>,
        timezone: &str,
    ) -> Result<(), PodmanError> {
        let mut args = self.machine_args(&["init"]);
        if let Some(cpus) = cpus {
            args.push("--cpus".to_string());
            args.push(cpus.to_string());
        }
        if let Some(memory) = memory {
            args.push("--memory".to_string());
            args.push(memory.to_string());
        }
        args.push("--timezone".to_string());
        args.push(timezone.to_string());
        args.push(self.name.clone());
        self.run_checked(&args)
    }

    /// Starts the Dangerzone machine.
    pub fn start(&self) -> Result<(), PodmanError> {
        self.run_on_machine("start")
    }

    /// Stops the Dangerzone machine.
    pub fn stop(&self) -> Result<(), PodmanError> {
        self.run_on_machine("stop")
    }

    /// Removes the Dangerzone machine.
    pub fn remove(&self) -> Result<(), PodmanError> {
        self.run_on_machine("rm")
    }

    /// Resets all Podman machines.
    pub fn reset(&self) -> Result<(), PodmanError> {
        let args = self.machine_args(&["reset", "--force"]);
        self.run_checked(&args)
    }

    /// Runs a raw `podman` command against the Dangerzone machine.
    pub fn run_raw_podman_command(&self, args: &[String]) -> Result<(), PodmanError> {
        self.runner
            .run_captured(args, true)
            .map(|_| ())
            .map_err(|error| PodmanError(error.0))
    }
}

impl Default for PodmanMachineManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::Mutex;

    /// Serializes access to the `DANGERZONE_PODMAN` environment variable.
    static PODMAN_ENV_LOCK: Mutex<()> = Mutex::new(());

    /// Writes a stub `podman` executable and points `DANGERZONE_PODMAN` at
    /// it, returning the temp directory's path.
    fn stub_podman(dir: &std::path::Path, script: &str) {
        let stub = dir.join("podman");
        std::fs::write(&stub, format!("#!/bin/sh\n{script}\n")).unwrap();
        std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::env::set_var("DANGERZONE_PODMAN", stub.display().to_string());
    }

    #[test]
    fn list_parses_machine_list_json() {
        let _guard = PODMAN_ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        stub_podman(
            dir.path(),
            r#"if [ "$1" = "machine" ] && [ "$2" = "list" ]; then
    printf '%s' '[{"Name":"dangerzone","Running":true},{"Name":"other","Running":false}]'
fi"#,
        );

        let manager = PodmanMachineManager::new();
        let machines = manager.list().unwrap();
        assert_eq!(machines.len(), 2);
        assert_eq!(machines[0].name, "dangerzone");
        assert!(machines[0].running);
        assert!(!machines[1].running);
        std::env::remove_var("DANGERZONE_PODMAN");
    }

    #[test]
    fn list_filters_out_other_running_machines() {
        let _guard = PODMAN_ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        stub_podman(
            dir.path(),
            r#"if [ "$1" = "machine" ] && [ "$2" = "list" ]; then
    printf '%s' '[{"Name":"dangerzone","Running":false},{"Name":"other","Running":true}]'
fi"#,
        );

        let manager = PodmanMachineManager::new();
        let other = manager.list_other_running_machines();
        assert_eq!(other, vec!["other".to_string()]);
        std::env::remove_var("DANGERZONE_PODMAN");
    }

    #[test]
    fn start_runs_the_machine_subcommand_with_the_name() {
        let _guard = PODMAN_ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        stub_podman(
            dir.path(),
            r#"echo "$@" >> "$DZ_LOG"
if [ "$1" = "machine" ] && [ "$2" = "start" ] && [ "$3" = "dangerzone" ]; then
    exit 0
fi
exit 1"#,
        );
        let log = dir.path().join("args.log");
        std::env::set_var("DZ_LOG", &log);

        let manager = PodmanMachineManager::new();
        manager.start().unwrap();
        let args = std::fs::read_to_string(&log).unwrap();
        assert_eq!(args.trim(), "machine start dangerzone");
        std::env::remove_var("DANGERZONE_PODMAN");
        std::env::remove_var("DZ_LOG");
    }
}
