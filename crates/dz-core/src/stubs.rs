//! Minimal interfaces for modules that live outside of this crate.
//!
//! The original Python code imports these from sibling modules
//! (`container_utils`, `podman`, `updater`, `windows`, `qubes`). Since only
//! this folder is being translated, these stubs define the *shape* of those
//! interfaces so the crate compiles and runs on its own. They are intentional
//! no-ops or safe defaults, not real implementations.

use crate::errors::ContainerError;

/// Helpers for listing and killing containers.
///
/// The signatures mirror the real implementation in
/// `dz-runtime::container_utils`, which is not usable from this crate (it
/// would create a dependency cycle).
pub mod container_utils {
    use super::ContainerError;

    /// Lists the names of the currently running containers.
    pub fn list_containers() -> Vec<String> {
        Vec::new()
    }

    /// Kills a running container by name.
    pub fn kill_container(_name: &str) -> Result<(), ContainerError> {
        Ok(())
    }
}

/// Helpers to detect Qubes OS native conversion.
pub mod qubes {
    /// Whether conversion happens natively on Qubes OS.
    pub fn is_qubes_native_conversion() -> bool {
        false
    }
}

/// Helpers to manage the Podman machine used on macOS/Windows.
///
/// This is the lifecycle subset used by the startup and shutdown task runners.
/// The command-line-facing manager with the full API lives in
/// `dz-runtime::podman::machine`.
pub mod podman {
    use super::ContainerError;

    /// Manages the Podman machine used by Dangerzone.
    pub struct PodmanMachineManager;

    impl PodmanMachineManager {
        /// Creates a new manager.
        pub fn new() -> Self {
            Self
        }

        /// Initializes the Dangerzone VM.
        pub fn init(&self) -> Result<(), ContainerError> {
            Ok(())
        }

        /// Starts the Dangerzone VM.
        pub fn start(&self) -> Result<(), ContainerError> {
            Ok(())
        }

        /// Stops the Dangerzone VM, or a specific machine by name.
        pub fn stop(&self, _name: Option<&str>) -> Result<(), ContainerError> {
            Ok(())
        }

        /// Lists other Podman machines that are currently running.
        pub fn list_other_running_machines(&self) -> Vec<String> {
            Vec::new()
        }
    }

    impl Default for PodmanMachineManager {
        fn default() -> Self {
            Self::new()
        }
    }
}

/// Helpers for the Windows Subsystem for Linux.
///
/// Corresponds to `dangerzone/windows/wsl.py`. WSL is only used on Windows,
/// where it provides the Linux VM that Podman runs conversions in; the startup
/// task that calls into this module is skipped on every other platform.
pub mod wsl {
    use crate::errors::WslError;

    /// The `wsl` binary, overridable for testing.
    const WSL_ENV: &str = "DANGERZONE_WSL";

    /// The `wsl` binary path, honouring the `DANGERZONE_WSL` override.
    fn wsl_binary() -> String {
        std::env::var(WSL_ENV).unwrap_or_else(|_| "wsl".to_string())
    }

    /// Returns the status output of the WSL engine.
    ///
    /// Corresponds to `windows/wsl.py:status()`. WSL prints UTF-16LE; since
    /// the output is only used to decide whether WSL is present, the lossy
    /// byte conversion is sufficient. A missing or failing `wsl --status`
    /// invocation means WSL is not installed.
    fn status() -> Result<String, WslError> {
        let output = std::process::Command::new(wsl_binary())
            .arg("--status")
            .output()
            .map_err(|error| WslError::NotInstalled(error.to_string()))?;
        if !output.status.success() {
            return Err(WslError::NotInstalled(
                String::from_utf8_lossy(&output.stderr).trim().to_string(),
            ));
        }
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }

    /// Whether Windows Subsystem for Linux is installed.
    ///
    /// Corresponds to `windows/wsl.py:is_installed()`.
    pub fn is_installed() -> bool {
        status().is_ok()
    }

    /// Installs WSL and checks whether a reboot is required.
    ///
    /// Corresponds to `windows/wsl.py:install_and_check_reboot()`. WSL is
    /// installed through a fallback chain of methods, mirroring upstream: an
    /// update, an install without a default distribution, then a plain
    /// install. On the first success a reboot is reported, matching the
    /// upstream decision to err on the side of rebooting.
    pub fn install_and_check_reboot() -> Result<(), WslError> {
        if is_installed() {
            return Ok(());
        }

        let methods: [(&str, &[&str]); 3] = [
            ("wsl --update", &["--update"]),
            (
                "wsl --install --no-distribution",
                &["--install", "--no-distribution"],
            ),
            ("wsl --install", &["--install"]),
        ];
        for (name, args) in methods {
            let ran = std::process::Command::new(wsl_binary())
                .args(args)
                .status()
                .map(|status| status.success())
                .unwrap_or(false);
            if ran {
                log::info!("WSL was installed via '{name}'; a reboot is required");
                return Err(WslError::InstallNeedsReboot(
                    "Windows Subsystem for Linux (WSL) was installed, but you need to \
                     reboot your computer before Dangerzone can use it."
                        .to_string(),
                ));
            }
            log::info!("Did not manage to install WSL via '{name}'");
        }

        Err(WslError::InstallFailed)
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::os::unix::fs::PermissionsExt;
        use std::sync::Mutex;

        /// Serializes access to the `DANGERZONE_WSL` environment variable.
        static WSL_ENV_LOCK: Mutex<()> = Mutex::new(());

        /// Writes a stub `wsl` executable that behaves according to `script`.
        fn wsl_stub(dir: &std::path::Path, script: &str) -> String {
            let stub = dir.join("wsl");
            std::fs::write(&stub, format!("#!/bin/sh\n{script}\n")).unwrap();
            std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();
            stub.display().to_string()
        }

        #[test]
        fn is_installed_checks_wsl_status() {
            let _guard = WSL_ENV_LOCK.lock().unwrap();
            let dir = tempfile::tempdir().unwrap();
            std::env::set_var(
                WSL_ENV,
                wsl_stub(
                    dir.path(),
                    "case \"$1\" in --status) exit 0;; *) exit 1;; esac",
                ),
            );
            assert!(is_installed());
            std::env::remove_var(WSL_ENV);
        }

        #[test]
        fn is_installed_is_false_when_wsl_is_missing() {
            let _guard = WSL_ENV_LOCK.lock().unwrap();
            let dir = tempfile::tempdir().unwrap();
            std::env::set_var(WSL_ENV, wsl_stub(dir.path(), "exit 1"));
            assert!(!is_installed());
            std::env::remove_var(WSL_ENV);
        }

        #[test]
        fn install_reports_reboot_after_a_successful_method() {
            let _guard = WSL_ENV_LOCK.lock().unwrap();
            let dir = tempfile::tempdir().unwrap();
            // `--status` fails (not yet installed), but every install method
            // succeeds, so a reboot is reported.
            std::env::set_var(
                WSL_ENV,
                wsl_stub(
                    dir.path(),
                    "case \"$1\" in --status) exit 1;; *) exit 0;; esac",
                ),
            );
            assert!(matches!(
                install_and_check_reboot(),
                Err(WslError::InstallNeedsReboot(_))
            ));
            std::env::remove_var(WSL_ENV);
        }

        #[test]
        fn install_fails_when_every_method_fails() {
            let _guard = WSL_ENV_LOCK.lock().unwrap();
            let dir = tempfile::tempdir().unwrap();
            std::env::set_var(WSL_ENV, wsl_stub(dir.path(), "exit 1"));
            assert!(matches!(
                install_and_check_reboot(),
                Err(WslError::InstallFailed)
            ));
            std::env::remove_var(WSL_ENV);
        }
    }
}
