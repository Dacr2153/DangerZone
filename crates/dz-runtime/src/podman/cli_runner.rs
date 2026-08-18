//! A thin runner that executes `podman` commands.
//!
//! Corresponds to `dangerzone/podman/cli_runner.py`, which Dangerzone vendors
//! from the Podman Python SDK. The runner builds the `podman` invocation from
//! the configured executable, global options and the requested subcommand,
//! then either waits for it (capturing the output) or spawns it.

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use super::errors::CommandError;

/// Global options that precede the `podman` subcommand on the command line.
///
/// Only a small subset of the SDK's options is modeled, since the rest are
/// unused by Dangerzone.
#[derive(Debug, Clone, Default)]
pub struct GlobalOptions {
    /// Whether to log debug output from Podman.
    pub debug: bool,
    /// Connection string for a remote Podman service.
    pub connection: Option<String>,
    /// Registry URL to use for the image.
    pub registry: Option<String>,
}

impl GlobalOptions {
    /// Creates a new set of global options with default values.
    pub fn new() -> Self {
        Self::default()
    }

    /// Appends the global options to a command line.
    fn as_args(&self) -> Vec<String> {
        let mut args = Vec::new();
        if self.debug {
            args.push("--log-level=debug".to_string());
        }
        if let Some(connection) = &self.connection {
            args.push(format!("--connection={connection}"));
        }
        if let Some(registry) = &self.registry {
            args.push(format!("--registry={registry}"));
        }
        args
    }
}

/// How to configure the standard streams of a spawned subprocess.
///
/// Corresponds to the `**skwargs` forwarded to `subprocess.Popen` when the
/// runner is asked not to wait for the command.
#[derive(Debug)]
pub struct SpawnOptions {
    /// What to attach to the child's standard input.
    pub stdin: Stdio,
    /// What to attach to the child's standard output.
    pub stdout: Stdio,
    /// What to attach to the child's standard error.
    pub stderr: Stdio,
    /// Whether to start the child in a new process group/session.
    pub new_session: bool,
}

impl Default for SpawnOptions {
    fn default() -> Self {
        Self {
            stdin: Stdio::piped(),
            stdout: Stdio::piped(),
            stderr: Stdio::null(),
            new_session: false,
        }
    }
}

/// The result of running a `podman` command.
pub enum RunResult {
    /// The command completed and its captured standard output.
    Output(String),
    /// The command was spawned and is still running.
    Process(crate::base::ConversionProcess),
}

/// Executes `podman` commands.
///
/// Corresponds to `cli_runner.Runner`.
pub struct Runner {
    path: Option<PathBuf>,
    privileged: bool,
    options: GlobalOptions,
    env: Option<HashMap<String, String>>,
}

impl Runner {
    /// Creates a new runner.
    ///
    /// `path` overrides the `podman` executable; `privileged` runs commands
    /// with elevated privileges (used by the SDK); `options` are the global
    /// options; `env` is merged into the child's environment.
    pub fn new(
        path: Option<PathBuf>,
        privileged: bool,
        options: Option<GlobalOptions>,
        env: Option<HashMap<String, String>>,
    ) -> Self {
        Self {
            path,
            privileged,
            options: options.unwrap_or_default(),
            env,
        }
    }

    /// The global options for this runner.
    pub fn options(&self) -> &GlobalOptions {
        &self.options
    }

    /// The full command line for a `podman` invocation.
    pub fn construct(&self, cmd: &[&str]) -> Vec<String> {
        let mut args = Vec::new();
        if self.privileged {
            args.push("sudo".to_string());
        }
        if let Some(path) = &self.path {
            args.push(path.to_string_lossy().into_owned());
        } else {
            args.push("podman".to_string());
        }
        args.extend(self.options.as_args());
        args.extend(cmd.iter().map(|arg| arg.to_string()));
        args
    }

    /// Runs a `podman` command, returning the captured output.
    ///
    /// When `check` is set, a non-zero exit status raises a `CommandError`.
    pub fn run_captured(&self, cmd: &[String], check: bool) -> Result<String, CommandError> {
        let mut command = self.build_command(cmd);
        let output = command
            .output()
            .map_err(|e| CommandError(format!("Could not run podman: {e}")))?;
        if check && !output.status.success() {
            let message = String::from_utf8_lossy(&output.stderr).trim().to_string();
            let message = if message.is_empty() {
                format!("podman command failed with status: {}", output.status)
            } else {
                message
            };
            return Err(CommandError(message));
        }
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }

    /// Spawns a `podman` command without waiting for it to finish.
    pub fn run_spawned(
        &self,
        cmd: &[String],
        spawn: SpawnOptions,
    ) -> Result<crate::base::ConversionProcess, CommandError> {
        let mut command = self.build_command(cmd);
        command
            .stdin(spawn.stdin)
            .stdout(spawn.stdout)
            .stderr(spawn.stderr);
        if spawn.new_session {
            set_new_session(&mut command);
        }
        let child = command
            .spawn()
            .map_err(|e| CommandError(format!("Could not run podman: {e}")))?;
        Ok(crate::base::ConversionProcess::new(child))
    }

    /// Builds a `podman` command line ready to be executed.
    fn build_command(&self, cmd: &[String]) -> Command {
        let mut args: Vec<String> = Vec::new();
        if self.privileged {
            args.push("sudo".to_string());
        }
        if let Some(path) = &self.path {
            args.push(path.to_string_lossy().into_owned());
        } else {
            args.push("podman".to_string());
        }
        args.extend(self.options.as_args());
        args.extend(cmd.iter().cloned());

        let mut command = Command::new(&args[0]);
        command.args(&args[1..]);
        if let Some(env) = &self.env {
            command.envs(env);
        }
        command
    }
}

/// Starts the child in a new process group/session, so that the whole process
/// group can be signaled later, without killing the controlling process.
#[cfg(unix)]
fn set_new_session(command: &mut Command) {
    use std::os::unix::process::CommandExt;
    command.process_group(0);
}

/// On Windows there is no process-group concept, so the flag is a no-op.
#[cfg(windows)]
fn set_new_session(_command: &mut Command) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn global_options_are_empty_by_default() {
        assert_eq!(GlobalOptions::new().as_args(), Vec::<String>::new());
    }

    #[test]
    fn global_options_serialize_into_arguments() {
        let options = GlobalOptions {
            debug: true,
            connection: Some("machine".to_string()),
            registry: None,
        };
        let args = options.as_args();
        assert!(args.contains(&"--log-level=debug".to_string()));
        assert!(args.contains(&"--connection=machine".to_string()));
    }

    #[test]
    fn construct_builds_a_podman_invocation() {
        let runner = Runner::new(None, false, None, None);
        let cmd = runner.construct(&["ps", "-a"]);
        assert_eq!(cmd, vec!["podman", "ps", "-a"]);
    }

    #[test]
    fn construct_prepends_sudo_when_privileged() {
        let runner = Runner::new(None, true, None, None);
        let cmd = runner.construct(&["info"]);
        assert_eq!(cmd, vec!["sudo", "podman", "info"]);
    }

    #[test]
    fn construct_uses_configured_path() {
        let runner = Runner::new(Some(PathBuf::from("/usr/bin/podman")), false, None, None);
        let cmd = runner.construct(&["version"]);
        assert_eq!(cmd, vec!["/usr/bin/podman", "version"]);
    }
}
