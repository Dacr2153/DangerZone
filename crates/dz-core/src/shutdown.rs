//! Shutdown tasks and the runner that executes them.

use crate::errors::TaskError;
use crate::settings;
use crate::startup::{handle_skip_nonlinux, Runner, RunnerHooks, Task};
use crate::stubs::{container_utils, podman};

/// Stops the Dangerzone VM (macOS/Windows only).
pub struct MachineStopTask;

impl Task for MachineStopTask {
    fn can_fail(&self) -> bool {
        true
    }

    fn name(&self) -> &str {
        "Stopping Dangerzone VM"
    }

    fn should_skip(&self) -> Result<bool, TaskError> {
        Ok(settings::read_settings().custom_runtime_specified() || cfg!(target_os = "linux"))
    }

    fn run(&self) -> Result<(), TaskError> {
        podman::PodmanMachineManager::new().stop(None)?;
        Ok(())
    }

    fn handle_skip(&self) {
        handle_skip_nonlinux(self.name());
    }
}

/// Stops the running sandbox container(s).
pub struct ContainerStopTask;

impl Task for ContainerStopTask {
    fn can_fail(&self) -> bool {
        true
    }

    fn name(&self) -> &str {
        "Stopping the sandbox"
    }

    fn run(&self) -> Result<(), TaskError> {
        // In practice, we don't expect more than 1 container in flight.
        for container in container_utils::list_containers() {
            container_utils::kill_container(&container)?;
        }
        Ok(())
    }
}

/// Mixin providing the shutdown hook implementations.
pub struct ShutdownMixin;

impl RunnerHooks for ShutdownMixin {
    fn handle_start_custom(&self) {
        log::info!("Shutting down Dangerzone");
    }

    fn handle_error_custom(&self, task: &dyn Task, _error: &TaskError) {
        log::error!(
            "Encountered an error in task '{}', while shutting down Dangerzone. Resuming...",
            task.name()
        );
    }

    fn handle_success_custom(&self) {
        log::info!("Dangerzone's shutdown tasks have finished successfully");
    }
}

/// The shutdown task runner.
pub type ShutdownLogic = Runner<ShutdownMixin>;

impl Runner<ShutdownMixin> {
    /// Creates a shutdown runner for the given tasks.
    pub fn new_shutdown(tasks: Vec<Box<dyn Task>>, raise_on_error: bool) -> Self {
        Self::new(tasks, raise_on_error, ShutdownMixin)
    }
}
