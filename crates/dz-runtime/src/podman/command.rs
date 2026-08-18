//! Main class for executing Podman commands.
//!
//! Corresponds to `dangerzone/podman/command.py`. The REST API service methods
//! (`start_service`/`stop_service`) are Linux-only in the original and are not
//! needed by the rest of Dangerzone, so they are not translated.

use std::collections::HashMap;
use std::path::PathBuf;

use super::cli_runner::{GlobalOptions, RunResult, Runner, SpawnOptions};
use super::errors::{CommandError, PodmanError};
use super::machine_manager::MachineManager;
use crate::base::ConversionProcess;

/// Executes Podman commands.
pub struct PodmanCommand {
    runner: Runner,
    // The Python class keeps a machine manager for parity with the Podman SDK;
    // this port does not manage machines, so the field is only kept alive here.
    #[allow(dead_code)]
    machine: MachineManager,
}

impl PodmanCommand {
    /// Creates a new `PodmanCommand`.
    ///
    /// `path` overrides the Podman executable, `privileged` runs commands with
    /// elevated privileges, `options` are the global Podman options, and `env`
    /// is merged into the subprocess environment.
    pub fn new(
        path: Option<PathBuf>,
        privileged: bool,
        options: Option<GlobalOptions>,
        env: Option<HashMap<String, String>>,
    ) -> Self {
        let runner = Runner::new(path, privileged, options, env);
        let machine = MachineManager::new();
        Self { runner, machine }
    }

    /// The global options used by this instance.
    pub fn options(&self) -> &GlobalOptions {
        self.runner.options()
    }

    /// Runs a `podman` command and returns its captured output.
    ///
    /// Corresponds to `Runner.run` with `wait=True` and `capture_output=True`.
    /// When `check` is set, a non-zero exit status raises a `CommandError`.
    pub fn run_captured(&self, cmd: &[String], check: bool) -> Result<String, CommandError> {
        self.runner.run_captured(cmd, check)
    }

    /// Spawns a `podman` command without waiting for it to finish.
    ///
    /// Corresponds to `Runner.run` with `wait=False`, which returns the
    /// `subprocess.Popen` handle.
    pub fn run_spawned(
        &self,
        cmd: &[String],
        spawn: SpawnOptions,
    ) -> Result<ConversionProcess, CommandError> {
        self.runner.run_spawned(cmd, spawn)
    }

    /// Runs a `podman` command in either mode.
    ///
    /// Mirrors the flexible `run()` signature of the Python original, which
    /// returns either the captured output or the spawned process.
    pub fn run(
        &self,
        cmd: &[String],
        check: bool,
        wait: bool,
        spawn: Option<SpawnOptions>,
    ) -> Result<RunResult, CommandError> {
        if wait {
            let output = self.runner.run_captured(cmd, check)?;
            Ok(RunResult::Output(output))
        } else {
            let process = self.runner.run_spawned(cmd, spawn.unwrap_or_default())?;
            Ok(RunResult::Process(process))
        }
    }

    /// Stops the Podman system service.
    ///
    /// The original starts a REST API with `podman system service`; this port
    /// never starts one, so there is nothing to stop.
    pub fn stop_service(&self, _timeout: Option<u64>) -> Result<i32, PodmanError> {
        Err(PodmanError(
            "The Podman service has not started yet, so there's nothing to stop".to_string(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stop_service_fails_without_a_service() {
        let podman = PodmanCommand::new(None, false, None, None);
        let result = podman.stop_service(None);
        assert!(result.is_err());
    }
}
