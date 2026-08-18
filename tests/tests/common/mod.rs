//! Shared helpers for the end-to-end integration tests.
//!
//! The tests drive the real `dz-dangerzone` binary as a subprocess, mirroring
//! how an end user invokes the tool. The binary lives in the workspace target
//! directory; a helper builds it on first use and caches the path.

use std::path::PathBuf;
use std::process::Command;
use std::sync::OnceLock;

/// The path to the `dz-dangerzone` binary, building it on first use.
pub fn dangerzone_binary() -> &'static PathBuf {
    static BINARY: OnceLock<PathBuf> = OnceLock::new();
    BINARY.get_or_init(build_binary)
}

/// Locates the binary, building it if it is not up to date.
fn build_binary() -> PathBuf {
    let cargo = std::env::var_os("CARGO")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("cargo"));

    let status = Command::new(&cargo)
        .arg("build")
        .arg("-p")
        .arg("dz-cli")
        .arg("--bin")
        .arg("dz-dangerzone")
        .arg("--quiet")
        .status()
        .expect("failed to run cargo to build dz-dangerzone");
    assert!(status.success(), "cargo failed to build dz-dangerzone");

    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = manifest
        .parent()
        .expect("tests package sits directly under the workspace root");
    let target_dir = match std::env::var_os("CARGO_TARGET_DIR") {
        Some(dir) if PathBuf::from(&dir).is_absolute() => PathBuf::from(dir),
        _ => workspace_root.join("target"),
    };
    target_dir.join("debug").join("dz-dangerzone")
}

/// Runs `dz-dangerzone` with the given arguments and environment.
///
/// `current_dir` defaults to a fresh temporary directory so the tests never
/// touch the working directory the test harness runs in.
pub struct Runner {
    command: Command,
    /// The temporary directory the command runs in, kept alive for its duration.
    _tempdir: tempfile::TempDir,
}

impl Runner {
    /// Creates a runner for the `dz-dangerzone` binary.
    pub fn new() -> Self {
        let tempdir = tempfile::tempdir().expect("failed to create temporary directory");
        let mut command = Command::new(dangerzone_binary());
        command.current_dir(tempdir.path());
        Self {
            command,
            _tempdir: tempdir,
        }
    }

    /// Sets an environment variable for the child process.
    pub fn env(&mut self, key: &str, value: &str) -> &mut Self {
        self.command.env(key, value);
        self
    }

    /// Appends a command-line argument for the child process.
    pub fn arg(&mut self, value: impl AsRef<std::ffi::OsStr>) -> &mut Self {
        self.command.arg(value.as_ref());
        self
    }

    /// Sets the working directory of the child process.
    #[allow(dead_code)]
    pub fn current_dir(&mut self, path: &std::path::Path) -> &mut Self {
        self.command.current_dir(path);
        self
    }

    /// Runs the binary and returns its output.
    pub fn run(&mut self) -> Output {
        let output = self
            .command
            .output()
            .expect("failed to spawn dz-dangerzone");
        Output {
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            code: output.status.code(),
        }
    }
}

/// The captured output of a finished subprocess.
pub struct Output {
    /// The process's standard output.
    pub stdout: String,
    /// The process's standard error.
    pub stderr: String,
    /// The process's exit code, if it terminated normally.
    pub code: Option<i32>,
}

impl Output {
    /// Asserts the process exited with the given code.
    pub fn assert_code(&self, expected: i32) -> &Self {
        assert_eq!(
            self.code,
            Some(expected),
            "expected exit code {expected}, got {:?}\nstdout:\n{}\nstderr:\n{}",
            self.code,
            self.stdout,
            self.stderr
        );
        self
    }

    /// Asserts the standard output contains the given text.
    pub fn assert_stdout_contains(&self, needle: &str) -> &Self {
        assert!(
            self.stdout.contains(needle),
            "stdout did not contain {needle:?}:\n{}",
            self.stdout
        );
        self
    }
}

/// Copies a checked-in fixture into a fresh temporary directory and returns the
/// copy's path plus the directory (kept alive by the caller).
pub fn fixture_in_tempdir(name: &str) -> (tempfile::TempDir, PathBuf) {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let fixture = manifest.join("fixtures").join(name);
    let tempdir = tempfile::tempdir().expect("failed to create temporary directory");
    let copy = tempdir.path().join(name);
    std::fs::copy(&fixture, &copy)
        .unwrap_or_else(|error| panic!("failed to copy fixture {name}: {error}"));
    (tempdir, copy)
}
