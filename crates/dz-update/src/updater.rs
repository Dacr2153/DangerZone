//! Concrete installer and update checker.
//!
//! Corresponds to `dangerzone/updater/installer.py` and
//! `dangerzone/updater/releases.py`. The trait interfaces live in `dz-core`
//! (see [`dz_core::updater`]); this module provides the real implementations,
//! which the applications wire into the startup tasks.

use std::time::{SystemTime, UNIX_EPOCH};

use dz_core::errors::ContainerError;
use dz_core::settings::{read_settings, Settings};
use dz_core::updater::{
    ContainerInstaller, ErrorReport, InstallationStrategy, ReleaseReport, UpdateChecker,
    UpdaterError,
};
use dz_core::util::{get_version, is_dev};
use dz_runtime::container_utils;
use dz_runtime::updater::signatures as updater_signatures;

use crate::errors::UpdateError;
use crate::signatures;

/// Check for updates at most every 12 hours, mirroring
/// `releases.UPDATE_CHECK_COOLDOWN_SECS`.
const UPDATE_CHECK_COOLDOWN_SECS: i64 = 60 * 60 * 12;

/// The concrete installer and update checker wired into the applications.
#[derive(Debug, Default, Clone, Copy)]
pub struct Updater;

impl ContainerInstaller for Updater {
    fn get_installation_strategy(
        &self,
        settings: &Settings,
    ) -> Result<InstallationStrategy, ContainerError> {
        // This logic compares the following indexes to make a decision:
        //
        // local_log_index:
        //   The largest log index of any installed container image. If an image
        //   is not present or this information is missing, treat it as 0. Since
        //   it is read from the signatures, it can be greater than the log
        //   index of the actual installed image, in case of application
        //   downgrades.
        //
        // remote_log_index:
        //   The largest log index for remote updates, verified by the update
        //   checker before. If updates are disabled, or errors occurred while
        //   attempting to detect updates, it is treated as 0 for this run.
        //
        // bundled_log_index:
        //   The log index of the image bundled with Dangerzone. If no
        //   container.tar is bundled, this is set to 0 so that the installer
        //   falls back to remote installation.
        //
        // max_log_index:
        //   The target log index for this run, the max of all the above.

        let bundled_log_index = if signatures::is_container_tar_bundled() {
            updater_signatures::LAST_KNOWN_LOG_INDEX
        } else {
            0
        };

        let podman_images = container_utils::list_image_digests()?;

        // Compute the local log index.
        let local_log_index =
            if podman_images.is_empty() || !updater_signatures::last_log_index_path().exists() {
                log::debug!("No podman images or no last_log_index file");
                0
            } else {
                updater_signatures::get_last_log_index()
            };

        // Compute the remote log index.
        let remote_log_index = match settings.updater_check_all() {
            Some(true) => settings.updater_remote_log_index() as i64,
            _ => {
                log::debug!("Skipping remote container upgrade (applying user settings)");
                0
            }
        };

        // Get the greatest log index, and store it as our target number.
        let max_log_index = local_log_index.max(remote_log_index).max(bundled_log_index);
        log::debug!("local_log_index={local_log_index}");
        log::debug!("remote_log_index={remote_log_index}");
        log::debug!("bundled_log_index={bundled_log_index}");
        log::debug!("max_log_index={max_log_index}");

        if local_log_index == max_log_index {
            // Sandbox is either up-to-date, user has disabled upgrades, or has
            // downgraded to a previous application version.
            log::debug!("Installation strategy: Do nothing");
            Ok(InstallationStrategy::DoNothing)
        } else if bundled_log_index == max_log_index {
            // The bundled sandbox image is fresher than the installed version.
            log::debug!("Installation strategy: Install the local container");
            Ok(InstallationStrategy::InstallLocalContainer)
        } else {
            // There is a remote update that is fresher than the currently
            // installed/available tarball.
            log::debug!("Installation strategy: Remote container update");
            Ok(InstallationStrategy::InstallRemoteContainer)
        }
    }

    fn install(&self) -> Result<(), ContainerError> {
        let strategy = self.get_installation_strategy(&read_settings())?;
        match strategy {
            InstallationStrategy::DoNothing => Ok(()),
            InstallationStrategy::InstallLocalContainer => {
                log::debug!("Install the local container tarball");
                let image_digest = signatures::install_local_container_tar().map_err(map_update)?;
                // Always clear old images, since we expect only one to exist at
                // a time.
                container_utils::clear_old_images(&image_digest)?;
                Ok(())
            }
            InstallationStrategy::InstallRemoteContainer => {
                log::debug!("Download and install a remote container image");
                let container_name = container_utils::expected_image_name();
                let (remote_digest, _, remote_signatures) =
                    signatures::get_remote_digest_and_logindex(&container_name)
                        .map_err(map_update)?;
                signatures::upgrade_container_image(
                    &remote_digest,
                    &container_name,
                    Some(&remote_signatures),
                )
                .map_err(map_update)?;
                // Always clear old images, since we expect only one to exist at
                // a time.
                container_utils::clear_old_images(&remote_digest)?;
                Ok(())
            }
        }
    }
}

impl UpdateChecker for Updater {
    fn should_check_for_updates(&self, settings: &mut Settings) -> Result<bool, UpdaterError> {
        if !signatures::is_container_tar_bundled() && !signatures::is_container_image_installed() {
            // Updates are required if there is neither a downloaded image on the
            // host, nor a container image bundled in the installer.
            log::debug!("No container available, prompting user to enable updates");
            return Err(UpdaterError::NeedUserInputNoContainer);
        }

        if settings.updater_last_check().is_none() {
            log::debug!("Dangerzone is running for the first time, updates are stalled");
            // Ignore the save error; the in-memory value is still updated.
            let _ = settings.set_updater_last_check(0, true);
            return Ok(false);
        }

        match settings.updater_check_all() {
            None => {
                log::debug!("User has not been asked yet for update checks");
                Err(UpdaterError::NeedUserInput)
            }
            Some(false) => {
                log::debug!("User has expressed that they don't want to check for updates");
                Ok(false)
            }
            Some(true) => Ok(true),
        }
    }

    fn check_for_updates(
        &self,
        settings: &mut Settings,
    ) -> Result<Option<ReleaseReport>, ErrorReport> {
        // On Linux, GitHub release checks are skipped, since users get
        // Dangerzone updates from their package manager.
        let is_linux = cfg!(target_os = "linux") && !is_dev();

        // If we already know from a previous run that there is a pending GitHub
        // release, return the report (but skip on Linux).
        if !is_linux {
            let latest_version = settings.updater_latest_version().to_string();
            let new_gh_version = match (
                semver::Version::parse(&get_version()),
                semver::Version::parse(&latest_version),
            ) {
                (Ok(current), Ok(latest)) => current < latest,
                _ => false,
            };
            if new_gh_version {
                return Ok(Some(ReleaseReport {
                    version: Some(latest_version),
                    changelog: Some(settings.updater_latest_changelog().to_string()),
                    container_image_bump: false,
                }));
            }
        }

        // If the previous check happened before the cooldown period expires, do
        // not check again. Else, bump the last check timestamp before making the
        // actual check, so that even failed update checks respect the cooldown.
        let current_time = now_timestamp();
        if current_time < settings.updater_last_check().unwrap_or(0) + UPDATE_CHECK_COOLDOWN_SECS {
            log::debug!("Cooling down update checks");
            return Ok(None);
        }
        if let Err(error) = settings.set_updater_last_check(current_time, true) {
            return Err(ErrorReport {
                error: error.to_string(),
            });
        }

        let mut report = ReleaseReport::default();

        // On Linux, skip GitHub release checks.
        if !is_linux {
            match fetch_github_release_info() {
                Ok((gh_version, gh_changelog)) => {
                    let latest_version = settings.updater_latest_version().to_string();
                    if ensure_sane_update(&latest_version, &gh_version) {
                        log::debug!("New GitHub release detected: {latest_version} < {gh_version}");
                        report.version = Some(gh_version);
                        report.changelog = Some(gh_changelog);
                    }
                }
                Err(error) => return Err(ErrorReport { error }),
            }
        }

        // Check for container image updates (on all platforms).
        let container_name = container_utils::expected_image_name();
        let (_, remote_log_index, _) =
            match signatures::get_remote_digest_and_logindex(&container_name) {
                Ok(result) => result,
                Err(error) => {
                    return Err(ErrorReport {
                        error: error.to_string(),
                    })
                }
            };
        let previous_remote_log_index = settings.updater_remote_log_index();
        if let Err(error) = settings.set_updater_remote_log_index(remote_log_index as u32, true) {
            return Err(ErrorReport {
                error: error.to_string(),
            });
        }
        if previous_remote_log_index < remote_log_index as u32 {
            report.container_image_bump = true;
        }

        if report.is_empty() {
            Ok(None)
        } else {
            Ok(Some(report))
        }
    }
}

/// Fetches the latest GitHub release info, returning the `(version, changelog)`
/// pair, mirroring `releases.fetch_github_release_info()`.
fn fetch_github_release_info() -> Result<(String, String), String> {
    log::debug!("Checking the latest GitHub release");
    let json = crate::manifest::download_manifest().map_err(|e| e.to_string())?;
    let manifest = crate::manifest::parse_manifest(&json).map_err(|e| e.to_string())?;
    let version = manifest.version();
    let changelog = manifest.changelog().unwrap_or_default();
    log::debug!("Latest version in GitHub is {version}");
    Ok((version, changelog))
}

/// Whether the latest GitHub version is a sane update over the current one,
/// mirroring `releases.ensure_sane_update()`.
fn ensure_sane_update(cur_version: &str, latest_version: &str) -> bool {
    match (
        semver::Version::parse(cur_version),
        semver::Version::parse(latest_version),
    ) {
        (Ok(cur), Ok(latest)) => {
            if cur == latest {
                false
            } else if cur > latest {
                // This case should only affect our QA releases. Log an error,
                // but don't block the rest of the update tasks.
                log::error!(
                    "The version received from Github Releases is older than the latest \
                     known version: ({cur} > {latest})"
                );
                false
            } else {
                true
            }
        }
        _ => false,
    }
}

/// The current time as a UNIX timestamp in seconds.
fn now_timestamp() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
}

/// Maps an update error onto a container error, preserving signature errors.
fn map_update(error: UpdateError) -> ContainerError {
    match error {
        UpdateError::Signature(signature) => ContainerError::Signature(signature),
        other => ContainerError::Io(std::io::Error::other(other.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sane_update_only_when_newer() {
        assert!(!ensure_sane_update("0.5.0", "0.5.0"));
        assert!(!ensure_sane_update("0.6.0", "0.5.0"));
        assert!(ensure_sane_update("0.5.0", "0.6.0"));
        assert!(!ensure_sane_update("not-a-version", "0.6.0"));
    }

    #[test]
    fn cooldown_matches_upstream() {
        assert_eq!(UPDATE_CHECK_COOLDOWN_SECS, 60 * 60 * 12);
    }
}
