//! Startup tasks and the runner that executes them.

use crate::errors::{
    OtherMachineRunningError, StartupError, TaskError, UpdaterDisabledNoContainer, WslError,
};
use crate::settings;
use crate::stubs::{podman, qubes, wsl};
use crate::updater::{
    ContainerInstaller, InstallationStrategy, ReleaseReport, UpdateChecker, UpdaterError,
};
use crate::util;

/// A single startup or shutdown task.
pub trait Task {
    /// Whether this task can fail without aborting the whole run.
    fn can_fail(&self) -> bool {
        false
    }

    /// The name of this task, used in log messages.
    fn name(&self) -> &str;

    /// Whether this task should be skipped.
    fn should_skip(&self) -> Result<bool, TaskError> {
        Ok(false)
    }

    /// Executes the task.
    fn run(&self) -> Result<(), TaskError>;

    /// Handles the task being skipped.
    fn handle_skip(&self) {
        log::info!("Task '{}' will be skipped", self.name());
    }

    /// Handles the task starting.
    fn handle_start(&self) {
        log::info!("Task '{}' is starting...", self.name());
    }

    /// Handles a task error.
    ///
    /// Do not return an error here, so that the error handler of the runner can
    /// run.
    fn handle_error(&self, error: &TaskError) {
        log::error!("Task '{}' failed with error: {}", self.name(), error);
    }

    /// Handles the task completing successfully.
    fn handle_success(&self) {
        log::info!("Task '{}' completed successfully!", self.name());
    }
}

/// Hooks invoked by the task runner.
pub trait RunnerHooks {
    /// Called before the first task starts.
    fn handle_start_custom(&self) {}

    /// Called when a non-failable task fails.
    fn handle_error_custom(&self, _task: &dyn Task, _error: &TaskError) {}

    /// Called after all tasks completed successfully.
    fn handle_success_custom(&self) {}
}

/// Executes a sequence of tasks, mirroring the Python `Runner` class.
pub struct Runner<H> {
    tasks: Vec<Box<dyn Task>>,
    raise_on_error: bool,
    hooks: H,
}

impl<H: RunnerHooks> Runner<H> {
    /// Creates a new runner for the given tasks.
    pub fn new(tasks: Vec<Box<dyn Task>>, raise_on_error: bool, hooks: H) -> Self {
        Self {
            tasks,
            raise_on_error,
            hooks,
        }
    }

    /// Runs a single task, returning an error if the task fails.
    fn run_task(&self, task: &dyn Task) -> Result<(), TaskError> {
        if task.should_skip()? {
            task.handle_skip();
            return Ok(());
        }
        task.handle_start();
        task.run()?;
        task.handle_success();
        Ok(())
    }

    /// Runs all tasks.
    ///
    /// Failable tasks log their error and the run continues. A non-failable
    /// task failure stops the run, surfacing the error when `raise_on_error`
    /// is set. Declining the initial container download stops the run without
    /// being treated as a task failure.
    pub fn run(&self) -> Result<(), TaskError> {
        self.hooks.handle_start_custom();
        for task in &self.tasks {
            match self.run_task(task.as_ref()) {
                Ok(()) => {}
                Err(error) if matches!(error, TaskError::UpdaterDisabledNoContainer(_)) => {
                    // Declining the initial container download is a user choice,
                    // not a task failure: skip the error-logging path. The CLI
                    // still wants the exception to surface; the GUI passes
                    // raise_on_error=false and just exits the run.
                    if self.raise_on_error {
                        return Err(error);
                    }
                    return Ok(());
                }
                Err(error) => {
                    task.handle_error(&error);
                    if !task.can_fail() {
                        self.hooks.handle_error_custom(task.as_ref(), &error);
                        if self.raise_on_error {
                            return Err(error);
                        }
                        return Ok(());
                    }
                }
            }
        }
        self.hooks.handle_success_custom();
        Ok(())
    }
}

/// Logs the skip of a task that only runs on specific platforms.
///
/// On Linux, where the task is never relevant, the skip is only logged at the
/// debug level.
pub(crate) fn handle_skip_nonlinux(name: &str) {
    if cfg!(target_os = "linux") {
        log::debug!("Task '{}' will be skipped", name);
    } else {
        log::info!("Task '{}' will be skipped", name);
    }
}

/// Whether a custom container runtime has been configured, or we are running
/// on Linux, in which case the Dangerzone VM is not used.
fn skip_dangerzone_vm_tasks() -> bool {
    settings::read_settings().custom_runtime_specified() || cfg!(target_os = "linux")
}

/// Initializes the Dangerzone VM (macOS/Windows only).
pub struct MachineInitTask;

impl Task for MachineInitTask {
    fn name(&self) -> &str {
        "Initializing Dangerzone VM"
    }

    fn should_skip(&self) -> Result<bool, TaskError> {
        Ok(skip_dangerzone_vm_tasks())
    }

    fn run(&self) -> Result<(), TaskError> {
        podman::PodmanMachineManager::new().init()?;
        Ok(())
    }

    fn handle_skip(&self) {
        handle_skip_nonlinux(self.name());
    }
}

/// Starts the Dangerzone VM (macOS/Windows only).
pub struct MachineStartTask;

impl Task for MachineStartTask {
    fn name(&self) -> &str {
        "Starting Dangerzone VM"
    }

    fn should_skip(&self) -> Result<bool, TaskError> {
        Ok(skip_dangerzone_vm_tasks())
    }

    fn run(&self) -> Result<(), TaskError> {
        podman::PodmanMachineManager::new().start()?;
        Ok(())
    }

    fn handle_skip(&self) {
        handle_skip_nonlinux(self.name());
    }
}

/// Stops other running Podman machines (macOS only).
pub struct MachineStopOthersTask;

impl MachineStopOthersTask {
    /// Fails with an "other machine running" error.
    fn fail(&self, message: impl Into<String>) -> TaskError {
        TaskError::OtherMachineRunning(OtherMachineRunningError(message.into()))
    }

    /// Returns whether the user has accepted to stop the machine.
    ///
    /// The base implementation cannot prompt, so it always fails.
    fn prompt_user(&self, machine_name: &str) -> Result<bool, TaskError> {
        Err(self.fail(format!(
            "Dangerzone has detected that a Podman machine with name '{}' is already running \
             in the system, but cannot prompt the user to stop it.",
            machine_name
        )))
    }
}

impl Task for MachineStopOthersTask {
    fn name(&self) -> &str {
        "Stopping other Podman VMs"
    }

    fn should_skip(&self) -> Result<bool, TaskError> {
        if settings::read_settings().custom_runtime_specified() {
            return Ok(true);
        }

        if cfg!(target_os = "linux") || cfg!(target_os = "windows") {
            // * On Linux, there are no Podman machines
            // * On Windows, WSL allows multiple VMs:
            //   https://github.com/containers/podman/issues/18415
            // * On macOS, only one Podman machine can run:
            //   https://docs.podman.io/en/v5.2.2/markdown/podman-machine-start.1.html
            return Ok(true);
        }

        let manager = podman::PodmanMachineManager::new();
        let other_running = manager.list_other_running_machines();
        if other_running.is_empty() {
            return Ok(true);
        }
        debug_assert_eq!(other_running.len(), 1);
        let machine_name = &other_running[0];
        log::info!(
            "Dangerzone has detected that a Podman machine with name '{}' is already running in \
             your system. This machine needs to stop so that Dangerzone can run.",
            machine_name
        );

        match settings::read_settings().stop_other_podman_machines() {
            "always" => {
                log::info!(
                    "Stopping the Podman machine because the user has asked us to remember their choice"
                );
                Ok(false)
            }
            "never" => Err(self.fail(
                "Another Podman machine is running and Dangerzone is configured to not stop it.",
            )),
            "ask" => {
                log::debug!("We need to prompt the user to stop the other Podman machine");
                let stop = self.prompt_user(machine_name)?;
                if !stop {
                    Err(self.fail(format!(
                        "User decided to quit Dangerzone instead of stopping Podman machine '{}'.",
                        machine_name
                    )))
                } else {
                    Ok(false)
                }
            }
            _ => Err(TaskError::Startup(StartupError(
                "BUG: Dangerzone cannot decide how to handle running Podman machine".to_string(),
            ))),
        }
    }

    fn run(&self) -> Result<(), TaskError> {
        let manager = podman::PodmanMachineManager::new();
        let other_running = manager.list_other_running_machines();
        for machine_name in other_running {
            log::info!("Stopping other Podman machine: {}", machine_name);
            manager.stop(Some(&machine_name))?;
        }

        // Verify no other machines are running.
        if !manager.list_other_running_machines().is_empty() {
            return Err(TaskError::Startup(StartupError(
                "Failed to stop all other running Podman machines.".to_string(),
            )));
        }
        Ok(())
    }

    fn handle_skip(&self) {
        handle_skip_nonlinux(self.name());
    }
}

/// Installs Windows Subsystem for Linux (Windows only).
pub struct WSLInstallTask;

impl WSLInstallTask {
    /// Whether the user has accepted the WSL install.
    ///
    /// In CLI mode the user is never prompted, so this always fails.
    fn prompt_install(&self) -> Result<bool, TaskError> {
        Err(TaskError::Wsl(WslError::NotInstalled(
            "Dangerzone requires Windows Subsystem for Linux (WSL), but it is not installed. \
             You can install it with 'wsl --install', or follow the instructions in \
             https://aka.ms/wslinstall"
                .to_string(),
        )))
    }

    /// Whether the user has accepted the reboot.
    ///
    /// In CLI mode the user is never prompted, so this always fails.
    fn prompt_reboot(&self) -> Result<bool, TaskError> {
        Err(TaskError::Wsl(WslError::InstallNeedsReboot(
            "Windows Subsystem for Linux (WSL) was installed successfully. Please reboot for \
             the changes to take effect."
                .to_string(),
        )))
    }
}

impl Task for WSLInstallTask {
    fn name(&self) -> &str {
        "Installing Windows Subsystem for Linux"
    }

    fn should_skip(&self) -> Result<bool, TaskError> {
        Ok(!cfg!(target_os = "windows") || wsl::is_installed())
    }

    fn run(&self) -> Result<(), TaskError> {
        if !self.prompt_install()? {
            return Err(TaskError::Wsl(WslError::NotInstalled(
                "User chose to quit instead of installing WSL".to_string(),
            )));
        }

        match wsl::install_and_check_reboot() {
            Ok(()) => Ok(()),
            Err(WslError::InstallNeedsReboot(_)) => {
                if self.prompt_reboot()? {
                    let mut command = std::process::Command::new("shutdown");
                    command.args(["/r", "/t", "0"]);
                    let _ = util::run_command(&mut command);
                    // The OS is about to reboot, so there's no need to continue
                    // with the rest of the startup steps.
                    Err(TaskError::Startup(StartupError(
                        "We are about to reboot..".to_string(),
                    )))
                } else {
                    Err(TaskError::Wsl(WslError::InstallNeedsReboot(
                        "User chose to quit instead of rebooting".to_string(),
                    )))
                }
            }
            Err(error) => Err(TaskError::Wsl(error)),
        }
    }

    fn handle_skip(&self) {
        handle_skip_nonlinux(self.name());
    }
}

/// Configures the Dangerzone sandbox (container image).
pub struct ContainerInstallTask {
    installer: Box<dyn ContainerInstaller>,
}

impl ContainerInstallTask {
    /// Creates a task backed by the given container installer.
    pub fn new(installer: Box<dyn ContainerInstaller>) -> Self {
        Self { installer }
    }
}

impl Task for ContainerInstallTask {
    fn name(&self) -> &str {
        "Configuring Dangerzone sandbox"
    }

    fn should_skip(&self) -> Result<bool, TaskError> {
        let guard = settings::read_settings();
        Ok(self.installer.get_installation_strategy(&guard)? == InstallationStrategy::DoNothing)
    }

    fn run(&self) -> Result<(), TaskError> {
        self.installer.install()?;
        Ok(())
    }
}

/// Checks for Dangerzone updates.
pub struct UpdateCheckTask {
    checker: Box<dyn UpdateChecker>,
}

impl UpdateCheckTask {
    /// Creates a task backed by the given update checker.
    pub fn new(checker: Box<dyn UpdateChecker>) -> Self {
        Self { checker }
    }

    /// Prompts the user to enable updates.
    ///
    /// In CLI mode the user is never prompted, so this always returns `false`.
    fn prompt_user(&self, _download_required: bool) -> Result<bool, TaskError> {
        Ok(false)
    }

    /// Logs that an application update is available.
    fn handle_app_update(&self, report: &ReleaseReport) {
        if let Some(version) = &report.version {
            log::info!("Dangerzone {version} is out and can be installed");
        }
    }

    /// Logs that a container image update is available.
    fn handle_container_update(&self, _report: &ReleaseReport) {
        log::info!("There is an update for the Dangerzone sandbox");
    }
}

impl Task for UpdateCheckTask {
    fn can_fail(&self) -> bool {
        true
    }

    fn name(&self) -> &str {
        "Check for updates"
    }

    fn should_skip(&self) -> Result<bool, TaskError> {
        if qubes::is_qubes_native_conversion() {
            // Update checks on Qubes don't make any sense, because there's no
            // container image, and application updates happen via the package
            // manager anyway.
            return Ok(true);
        }

        let mut guard = settings::write_settings();
        match self.checker.should_check_for_updates(&mut guard) {
            Ok(true) => Ok(false),
            Ok(false) => Ok(true),
            Err(UpdaterError::NeedUserInputNoContainer) => {
                if self.prompt_user(true)? {
                    guard
                        .set_updater_check_all(true, true)
                        .map_err(TaskError::Io)?;
                    Ok(false)
                } else {
                    // User declined or pressed X, raise an error to stop startup.
                    Err(TaskError::UpdaterDisabledNoContainer(
                        UpdaterDisabledNoContainer,
                    ))
                }
            }
            Err(UpdaterError::NeedUserInput) => {
                self.prompt_user(false)?;
                Ok(true)
            }
        }
    }

    fn run(&self) -> Result<(), TaskError> {
        let mut guard = settings::write_settings();
        let report = self.checker.check_for_updates(&mut guard);
        match report {
            Ok(Some(report)) => {
                if report.new_github_release() {
                    self.handle_app_update(&report);
                }
                if report.container_image_bump {
                    self.handle_container_update(&report);
                }
                Ok(())
            }
            Ok(None) => Ok(()),
            Err(report) => Err(TaskError::UpdateCheck(report.error)),
        }
    }
}

/// Mixin providing the default startup hook implementations.
pub struct StartupMixin;

impl RunnerHooks for StartupMixin {
    fn handle_start_custom(&self) {
        log::info!("Performing some Dangerzone startup tasks");
    }

    fn handle_error_custom(&self, task: &dyn Task, _error: &TaskError) {
        log::error!(
            "Stopping startup tasks because task '{}' failed with an error",
            task.name()
        );
    }

    fn handle_success_custom(&self) {
        log::info!("Successfully finished all Dangerzone startup tasks");
    }
}

/// The startup task runner.
pub type StartupLogic = Runner<StartupMixin>;

impl Runner<StartupMixin> {
    /// Creates a startup runner for the given tasks.
    pub fn new_startup(tasks: Vec<Box<dyn Task>>, raise_on_error: bool) -> Self {
        Self::new(tasks, raise_on_error, StartupMixin)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct OkTask {
        name: &'static str,
    }

    impl Task for OkTask {
        fn name(&self) -> &str {
            self.name
        }
        fn run(&self) -> Result<(), TaskError> {
            Ok(())
        }
    }

    struct SkipTask {
        name: &'static str,
    }

    impl Task for SkipTask {
        fn name(&self) -> &str {
            self.name
        }
        fn should_skip(&self) -> Result<bool, TaskError> {
            Ok(true)
        }
        fn run(&self) -> Result<(), TaskError> {
            Ok(())
        }
    }

    struct FailTask {
        name: &'static str,
    }

    impl Task for FailTask {
        fn name(&self) -> &str {
            self.name
        }
        fn run(&self) -> Result<(), TaskError> {
            Err(TaskError::Startup(StartupError("nope".to_string())))
        }
    }

    struct FailableFailTask;

    impl Task for FailableFailTask {
        fn can_fail(&self) -> bool {
            true
        }
        fn name(&self) -> &str {
            "failable"
        }
        fn run(&self) -> Result<(), TaskError> {
            Err(TaskError::Startup(StartupError("nope".to_string())))
        }
    }

    struct CountingHooks(std::sync::atomic::AtomicUsize);

    impl RunnerHooks for CountingHooks {
        fn handle_error_custom(&self, _task: &dyn Task, _error: &TaskError) {
            self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
    }

    #[test]
    fn runner_runs_all_tasks_and_succeeds() {
        let runner: Runner<StartupMixin> = Runner::new(
            vec![
                Box::new(OkTask { name: "one" }),
                Box::new(OkTask { name: "two" }),
            ],
            true,
            StartupMixin,
        );
        assert!(runner.run().is_ok());
    }

    #[test]
    fn runner_skips_tasks() {
        let runner: Runner<StartupMixin> = Runner::new(
            vec![
                Box::new(SkipTask { name: "skip" }),
                Box::new(OkTask { name: "two" }),
            ],
            true,
            StartupMixin,
        );
        assert!(runner.run().is_ok());
    }

    #[test]
    fn runner_propagates_error_when_raise_on_error() {
        let runner: Runner<StartupMixin> = Runner::new(
            vec![Box::new(FailTask { name: "fail" })],
            true,
            StartupMixin,
        );
        let result = runner.run();
        assert!(matches!(result, Err(TaskError::Startup(_))));
    }

    #[test]
    fn runner_stops_without_error_when_not_raising() {
        let hooks = CountingHooks(std::sync::atomic::AtomicUsize::new(0));
        let runner: Runner<CountingHooks> =
            Runner::new(vec![Box::new(FailTask { name: "fail" })], false, hooks);
        let result = runner.run();
        assert!(result.is_ok());
    }

    #[test]
    fn runner_continues_after_failable_task_error() {
        let runner: Runner<StartupMixin> = Runner::new(
            vec![Box::new(FailableFailTask), Box::new(OkTask { name: "two" })],
            true,
            StartupMixin,
        );
        assert!(runner.run().is_ok());
    }

    #[test]
    fn runner_stops_on_updater_disabled_without_logging() {
        struct UpdaterTask;

        impl Task for UpdaterTask {
            fn name(&self) -> &str {
                "updater"
            }
            fn should_skip(&self) -> Result<bool, TaskError> {
                Err(TaskError::UpdaterDisabledNoContainer(
                    UpdaterDisabledNoContainer,
                ))
            }
            fn run(&self) -> Result<(), TaskError> {
                Ok(())
            }
        }

        let runner: Runner<StartupMixin> =
            Runner::new(vec![Box::new(UpdaterTask)], false, StartupMixin);
        let result = runner.run();
        assert!(result.is_ok());
    }
}
