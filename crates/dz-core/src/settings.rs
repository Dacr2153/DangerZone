//! Application settings, persisted as JSON.
//!
//! The original Python class is a process-wide singleton. In Rust we expose a
//! shared, lockable instance through [`settings()`]; this keeps the global
//! state visible without forcing call sites to manage the lock themselves.

use std::path::{Path, PathBuf};
use std::sync::{LazyLock, RwLock, RwLockReadGuard, RwLockWriteGuard};

use serde::{Deserialize, Serialize};

use crate::errors::ContainerError;
use crate::util::{get_config_dir, get_version};

/// Name of the file where the settings are persisted.
const SETTINGS_FILENAME: &str = "settings.json";

/// The settings of the application, mirroring the key/value pairs of the
/// original `settings.json`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Settings {
    pub(crate) save: bool,
    pub(crate) archive: bool,
    pub(crate) ocr: bool,
    pub(crate) ocr_language: String,
    pub(crate) open: bool,
    pub(crate) open_app: Option<String>,
    pub(crate) safe_extension: String,
    pub(crate) updater_ask_before_download: bool,
    pub(crate) updater_check_all: Option<bool>,
    pub(crate) updater_last_check: Option<i64>,
    pub(crate) updater_latest_version: String,
    pub(crate) updater_latest_changelog: String,
    pub(crate) updater_remote_log_index: u32,
    pub(crate) updater_errors: u32,
    pub(crate) output_dir: Option<String>,
    pub(crate) stop_other_podman_machines: String,
    pub(crate) container_runtime: Option<String>,
}

impl Default for Settings {
    /// The default settings, mirroring `generate_default_settings()`.
    fn default() -> Self {
        Self {
            save: true,
            archive: true,
            ocr: true,
            ocr_language: "English".to_string(),
            open: true,
            open_app: None,
            safe_extension: crate::document::SAFE_EXTENSION.to_string(),
            updater_ask_before_download: true,
            updater_check_all: None,
            // Last check in UNIX epoch (secs since 1970).
            updater_last_check: None,
            // FIXME: How to invalidate these if they change upstream?
            updater_latest_version: get_version(),
            updater_latest_changelog: String::new(),
            updater_remote_log_index: 0,
            updater_errors: 0,
            output_dir: None,
            stop_other_podman_machines: "ask".to_string(),
            container_runtime: None,
        }
    }
}

static SETTINGS: LazyLock<RwLock<Settings>> =
    LazyLock::new(|| RwLock::new(Settings::load_from_disk()));

/// Returns the process-wide settings instance.
pub fn settings() -> &'static RwLock<Settings> {
    &SETTINGS
}

/// Acquires a read guard on the process-wide settings.
///
/// A poisoned lock still yields the underlying settings instead of panicking.
pub fn read_settings() -> RwLockReadGuard<'static, Settings> {
    SETTINGS
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Acquires a write guard on the process-wide settings.
///
/// A poisoned lock still yields the underlying settings instead of panicking.
pub fn write_settings() -> RwLockWriteGuard<'static, Settings> {
    SETTINGS
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl Settings {
    /// Loads the settings from disk, or starts from the defaults.
    ///
    /// Missing settings are merged from the defaults, and the stored updater
    /// version is bumped if it is older than the installed one. The settings
    /// are then persisted, mirroring the unconditional `save()` call.
    pub fn load_from_disk() -> Self {
        let mut settings = Self::default();
        settings.load();
        settings
    }

    /// Loads settings from the default settings file.
    pub fn load(&mut self) {
        self.load_from(&settings_file_path());
    }

    /// Loads settings from a specific file.
    pub fn load_from(&mut self, filename: &Path) {
        if !filename.is_file() {
            log::info!("Settings file doesn't exist, starting with default");
            *self = Self::default();
        } else {
            match std::fs::read_to_string(filename) {
                Ok(content) => match serde_json::from_str::<Settings>(&content) {
                    Ok(mut parsed) => {
                        // `#[serde(default)]` fills in any missing fields from
                        // the defaults during deserialization.
                        if let Ok(stored) = semver::Version::parse(&parsed.updater_latest_version) {
                            if let Ok(current) = semver::Version::parse(&get_version()) {
                                if current > stored {
                                    parsed.updater_latest_version = get_version();
                                }
                            }
                        }
                        *self = parsed;
                    }
                    Err(e) => {
                        log::error!("Error loading settings, falling back to default {e}");
                        *self = Self::default();
                    }
                },
                Err(e) => {
                    log::error!("Error loading settings, falling back to default {e}");
                    *self = Self::default();
                }
            }
        }
        if let Err(e) = self.save() {
            log::error!("Failed to save settings: {e}");
        }
    }

    /// Saves the settings to the default settings file.
    pub fn save(&self) -> std::io::Result<()> {
        self.save_to(&settings_file_path())
    }

    /// Saves the settings to a specific file.
    pub fn save_to(&self, filename: &Path) -> std::io::Result<()> {
        if let Some(parent) = filename.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
        std::fs::write(filename, json)
    }

    /// Whether a custom container runtime has been configured.
    pub fn custom_runtime_specified(&self) -> bool {
        self.container_runtime.is_some()
    }

    /// Sets the custom container runtime, optionally persisting the change.
    pub fn set_custom_runtime(
        &mut self,
        runtime: &str,
        autosave: bool,
    ) -> Result<PathBuf, ContainerError> {
        let container_runtime = self.path_from_name(runtime)?;
        self.container_runtime = Some(container_runtime.to_string_lossy().to_string());
        if autosave {
            self.save()?;
        }
        Ok(container_runtime)
    }

    /// Resolves a runtime name to a path, either directly or through `PATH`.
    pub fn path_from_name(&self, name: &str) -> Result<PathBuf, ContainerError> {
        let name_path = Path::new(name);
        if name_path.is_file() {
            return Ok(name_path.to_path_buf());
        }
        match find_in_path(name) {
            Some(runtime) => Ok(runtime),
            None => Err(ContainerError::NoContainerTech(name.to_string())),
        }
    }

    /// Removes the configured custom container runtime and persists the change.
    pub fn unset_custom_runtime(&mut self) -> Result<(), ContainerError> {
        self.container_runtime = None;
        self.save()?;
        Ok(())
    }

    /// Returns the `updater_*` settings, mirroring the Python dict of the same
    /// name.
    pub fn get_updater_settings(&self) -> serde_json::Map<String, serde_json::Value> {
        let Ok(serde_json::Value::Object(obj)) = serde_json::to_value(self) else {
            return serde_json::Map::new();
        };
        obj.into_iter()
            .filter(|(key, _)| key.starts_with("updater_"))
            .collect()
    }

    /// Whether the user asked to always stop other Podman machines.
    pub fn stop_other_podman_machines(&self) -> &str {
        &self.stop_other_podman_machines
    }

    /// Whether update checks are enabled, if the user has decided.
    pub fn updater_check_all(&self) -> Option<bool> {
        self.updater_check_all
    }

    /// Sets whether update checks are enabled, mirroring the Python `set()`
    /// method with its `autosave` semantics.
    pub fn set_updater_check_all(&mut self, value: bool, autosave: bool) -> std::io::Result<()> {
        let changed = self.updater_check_all != Some(value);
        self.updater_check_all = Some(value);
        if autosave && changed {
            self.save()?;
        }
        Ok(())
    }

    /// The timestamp of the last update check, in seconds since the epoch.
    pub fn updater_last_check(&self) -> Option<i64> {
        self.updater_last_check
    }

    /// Sets the timestamp of the last update check.
    pub fn set_updater_last_check(&mut self, value: i64, autosave: bool) -> std::io::Result<()> {
        let changed = self.updater_last_check != Some(value);
        self.updater_last_check = Some(value);
        if autosave && changed {
            self.save()?;
        }
        Ok(())
    }

    /// The latest known Dangerzone version.
    pub fn updater_latest_version(&self) -> &str {
        &self.updater_latest_version
    }

    /// The changelog of the latest known Dangerzone version, in Markdown.
    pub fn updater_latest_changelog(&self) -> &str {
        &self.updater_latest_changelog
    }

    /// The last observed log index of the remote container image.
    pub fn updater_remote_log_index(&self) -> u32 {
        self.updater_remote_log_index
    }

    /// Sets the last observed log index of the remote container image.
    pub fn set_updater_remote_log_index(
        &mut self,
        value: u32,
        autosave: bool,
    ) -> std::io::Result<()> {
        let changed = self.updater_remote_log_index != value;
        self.updater_remote_log_index = value;
        if autosave && changed {
            self.save()?;
        }
        Ok(())
    }

    /// Whether the user wants original archives preserved.
    pub fn archive(&self) -> bool {
        self.archive
    }

    /// Mutable access to the archive setting.
    pub fn archive_mut(&mut self) -> &mut bool {
        &mut self.archive
    }

    /// Whether OCR is enabled.
    pub fn ocr(&self) -> bool {
        self.ocr
    }

    /// Mutable access to the OCR setting.
    pub fn ocr_mut(&mut self) -> &mut bool {
        &mut self.ocr
    }

    /// The currently selected OCR language.
    pub fn ocr_language(&self) -> &str {
        &self.ocr_language
    }

    /// Mutable access to the OCR language string.
    pub fn ocr_language_mut(&mut self) -> &mut String {
        &mut self.ocr_language
    }

    /// The currently configured output directory.
    pub fn output_dir(&self) -> Option<&str> {
        self.output_dir.as_deref()
    }

    /// Mutable access to the output directory.
    pub fn output_dir_mut(&mut self) -> &mut Option<String> {
        &mut self.output_dir
    }
}

/// The path of the default settings file.
fn settings_file_path() -> PathBuf {
    get_config_dir().join(SETTINGS_FILENAME)
}

/// Searches `PATH` for an executable with the given name.
fn find_in_path(executable: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(executable))
        .find(|candidate| candidate.is_file() && is_executable(candidate))
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .map(|meta| meta.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable(_path: &Path) -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let dir =
                std::env::temp_dir().join(format!("dz_settings_{}_{}", std::process::id(), nanos));
            std::fs::create_dir_all(&dir).unwrap();
            TempDir(dir)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn defaults_match_python_originals() {
        let settings = Settings::default();
        assert!(settings.save);
        assert!(settings.archive);
        assert!(settings.ocr);
        assert_eq!(settings.ocr_language, "English");
        assert_eq!(settings.stop_other_podman_machines, "ask");
        assert_eq!(settings.safe_extension, crate::document::SAFE_EXTENSION);
    }

    #[test]
    fn save_and_load_roundtrip() {
        let temp = TempDir::new();
        let file = temp.path().join("settings.json");

        let settings = Settings {
            ocr: false,
            ocr_language: "Spanish".to_string(),
            ..Settings::default()
        };
        settings.save_to(&file).unwrap();

        let mut loaded = Settings::default();
        loaded.load_from(&file);
        assert!(!loaded.ocr);
        assert_eq!(loaded.ocr_language, "Spanish");
    }

    #[test]
    fn load_merges_missing_fields_from_defaults() {
        let temp = TempDir::new();
        let file = temp.path().join("settings.json");
        std::fs::write(&file, r#"{ "ocr": false }"#).unwrap();

        let mut loaded = Settings::default();
        loaded.load_from(&file);
        assert!(!loaded.ocr);
        assert!(loaded.save);
    }

    #[test]
    fn load_handles_corrupt_json_with_defaults() {
        let temp = TempDir::new();
        let file = temp.path().join("settings.json");
        std::fs::write(&file, "not json").unwrap();

        let mut loaded = Settings::default();
        loaded.load_from(&file);
        assert_eq!(loaded, Settings::default());
    }

    #[test]
    fn custom_runtime_not_specified_by_default() {
        let settings = Settings::default();
        assert!(!settings.custom_runtime_specified());
    }

    #[test]
    fn path_from_name_returns_existing_file() {
        let temp = TempDir::new();
        let runtime = temp.path().join("runtime");
        std::fs::write(&runtime, "binary").unwrap();

        let settings = Settings::default();
        let result = settings.path_from_name(runtime.to_str().unwrap()).unwrap();
        assert_eq!(result, runtime);
    }

    #[test]
    fn path_from_name_errors_for_missing_runtime() {
        let settings = Settings::default();
        let result = settings.path_from_name("definitely-not-a-real-binary-xyz");
        assert!(matches!(result, Err(ContainerError::NoContainerTech(_))));
    }

    #[test]
    fn updater_settings_only_include_updater_keys() {
        let settings = Settings::default();
        let updater = settings.get_updater_settings();
        assert!(!updater.is_empty());
        assert!(updater.keys().all(|key| key.starts_with("updater_")));
    }
}
