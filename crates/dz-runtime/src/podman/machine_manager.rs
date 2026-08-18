//! Manager for Podman machine lifecycle operations.
//!
//! Corresponds to `dangerzone/podman/machine_manager.py`, which Dangerzone
//! vendors from the Podman Python SDK. Dangerzone only uses this to obtain the
//! machine used to run conversions; the lifecycle operations are delegated to
//! [`super::machine::PodmanMachineManager`], which shells out to the Podman
//! binary.

use super::errors::PodmanError;
use super::machine::{PodmanMachine, PodmanMachineManager};

/// Coordinates the lifecycle of a Podman machine.
pub struct MachineManager;

impl MachineManager {
    /// Creates a new machine manager.
    pub fn new() -> Self {
        Self
    }

    /// Lists the available Podman machines.
    pub fn list(&self) -> Result<Vec<PodmanMachine>, PodmanError> {
        PodmanMachineManager::new().list()
    }

    /// Initializes the Dangerzone machine.
    pub fn init(
        &self,
        cpus: Option<u64>,
        memory: Option<u64>,
        timezone: &str,
    ) -> Result<(), PodmanError> {
        PodmanMachineManager::new().init(cpus, memory, timezone)
    }

    /// Starts the Dangerzone machine.
    pub fn start(&self) -> Result<(), PodmanError> {
        PodmanMachineManager::new().start()
    }

    /// Stops the Dangerzone machine.
    pub fn stop(&self) -> Result<(), PodmanError> {
        PodmanMachineManager::new().stop()
    }

    /// Lists the Podman machines other than the Dangerzone one that are
    /// running.
    pub fn list_other_running_machines(&self) -> Vec<String> {
        PodmanMachineManager::new().list_other_running_machines()
    }

    /// Ensures the Dangerzone machine is running.
    pub fn ensure_running(&self) -> Result<(), PodmanError> {
        PodmanMachineManager::new().ensure_running()
    }
}

impl Default for MachineManager {
    fn default() -> Self {
        Self::new()
    }
}
