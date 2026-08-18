//! Interfaces for installing and updating the Dangerzone container image.
//!
//! The original Python code imports the installer and the release checker from
//! the `dangerzone.updater` package. Those modules need the container runtime
//! and the signing tooling, which live in `dz-runtime`/`dz-update` and cannot
//! be imported here without creating a dependency cycle. These interfaces are
//! defined in this crate and implemented in `dz-update`; the applications wire
//! the concrete implementation into the startup tasks.

use crate::errors::ContainerError;
use crate::settings::Settings;

/// Strategy for installing the Dangerzone container image.
///
/// Corresponds to `installer.Strategy`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallationStrategy {
    /// Do not install or upgrade the container image.
    DoNothing,
    /// Install the container image bundled with this build.
    InstallLocalContainer,
    /// Download and install the container image from the registry.
    InstallRemoteContainer,
}

/// Installs the Dangerzone container image.
///
/// Corresponds to the public functions of `dangerzone/updater/installer.py`.
pub trait ContainerInstaller {
    /// Decides which installation strategy to apply, mirroring
    /// `installer.get_installation_strategy()`.
    fn get_installation_strategy(
        &self,
        settings: &Settings,
    ) -> Result<InstallationStrategy, ContainerError>;

    /// Installs or upgrades the container image based on the strategy,
    /// mirroring `installer.install()`.
    fn install(&self) -> Result<(), ContainerError>;
}

/// A report of a successful update check.
///
/// Corresponds to `releases.ReleaseReport`.
#[derive(Debug, Clone, Default)]
pub struct ReleaseReport {
    /// The version of the latest GitHub release, when one is pending.
    pub version: Option<String>,
    /// The changelog of the latest GitHub release, in Markdown.
    pub changelog: Option<String>,
    /// Whether the container image needs to be updated.
    pub container_image_bump: bool,
}

impl ReleaseReport {
    /// Whether a new GitHub release has been detected.
    pub fn new_github_release(&self) -> bool {
        self.version.is_some()
    }

    /// Whether the report carries no information at all.
    pub fn is_empty(&self) -> bool {
        self.version.is_none() && self.changelog.is_none() && !self.container_image_bump
    }
}

/// A report of a failed update check.
///
/// Corresponds to `releases.ErrorReport`.
#[derive(Debug)]
pub struct ErrorReport {
    /// The underlying error message.
    pub error: String,
}

/// Errors raised while deciding whether to check for updates.
///
/// Corresponds to the `NeedUserInputError` family of `releases.py`.
#[derive(Debug, thiserror::Error)]
pub enum UpdaterError {
    /// User input is required, but no container image is available.
    #[error("Need user input, but no container is available")]
    NeedUserInputNoContainer,
    /// User input is required.
    #[error("Need user input")]
    NeedUserInput,
}

/// Checks for Dangerzone application and container image updates.
///
/// Corresponds to the public functions of `dangerzone/updater/releases.py`.
pub trait UpdateChecker {
    /// Whether the user should be asked to check for updates, mirroring
    /// `releases.should_check_for_updates()`.
    fn should_check_for_updates(&self, settings: &mut Settings) -> Result<bool, UpdaterError>;

    /// Checks for updates and returns a release report, mirroring
    /// `releases.check_for_updates()`.
    ///
    /// `Ok(None)` means that nothing needs to be reported; `Err(report)`
    /// carries the error report of a failed check.
    fn check_for_updates(
        &self,
        settings: &mut Settings,
    ) -> Result<Option<ReleaseReport>, ErrorReport>;
}
