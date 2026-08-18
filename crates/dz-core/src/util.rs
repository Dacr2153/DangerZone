//! Platform and I/O helpers shared across the Dangerzone core library.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use unicode_general_category::{get_general_category, GeneralCategory};

/// Errors raised by the utility helpers.
#[derive(Debug, thiserror::Error)]
pub enum UtilError {
    /// Tesseract language data could not be found.
    #[error("Tesseract language data are not installed in the system")]
    TessdataNotFound,
}

/// Returns whether this is a Dangerzone development build.
///
/// Mirrors `DANGERZONE_DEV=1` setting `sys.dangerzone_dev` in the original code.
pub fn is_dev() -> bool {
    std::env::var("DANGERZONE_DEV").as_deref() == Ok("1")
}

/// Runs a `Command`, mirroring `subprocess_run`.
///
/// On Windows, the child process is started without a window, mirroring the
/// `STARTF_USESHOWWINDOW` startup info used by the original wrapper. Callers
/// decide whether to inspect the exit status, just like the Python version
/// forwards `check` verbatim.
pub fn run_command(cmd: &mut Command) -> std::io::Result<Output> {
    // FIXME: The Windows flag below hard-codes the CREATE_NO_WINDOW constant.
    // There is no standard-library constant for it yet.
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    cmd.output()
}

/// Runs a `Command` and verifies that it exited successfully.
///
/// Unlike `run_command`, this returns an error when the exit status is not
/// success, so callers cannot silently ignore a failed subprocess.
pub fn run_command_checked(cmd: &mut Command) -> std::io::Result<Output> {
    let output = run_command(cmd)?;
    if !output.status.success() {
        return Err(std::io::Error::other(format!(
            "command failed with status: {}",
            output.status
        )));
    }
    Ok(output)
}

/// Returns the currently detected architecture, normalized to `amd64` or
/// `arm64`.
pub fn get_architecture() -> &'static str {
    match std::env::consts::ARCH {
        "x86_64" | "amd64" => "amd64",
        "aarch64" | "arm64" => "arm64",
        other => other,
    }
}

/// Returns the Dangerzone user cache directory.
pub fn get_cache_dir() -> PathBuf {
    dirs_next::cache_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("dangerzone")
}

/// Returns the Dangerzone user config directory.
pub fn get_config_dir() -> PathBuf {
    dirs_next::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("dangerzone")
}

/// Returns the path to a resource shipped with Dangerzone, if any.
///
/// In development builds the resource is looked up relative to the project
/// root. Otherwise the lookup depends on the platform's install layout.
/// Returns `None` on platforms without a known layout.
pub fn get_resource_path(filename: &str) -> Option<PathBuf> {
    let prefix = if is_dev() {
        // Look for the resources directory relative to the project root.
        let project_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
        project_root.join("share")
    } else {
        resource_prefix_non_dev()?
    };
    Some(prefix.join(filename))
}

#[cfg(target_os = "linux")]
fn resource_prefix_non_dev() -> Option<PathBuf> {
    // The Python original uses `sys.prefix`. In Rust we honour an explicit
    // DANGERZONE_PREFIX env var, defaulting to the usual /usr/local prefix.
    let prefix = std::env::var_os("DANGERZONE_PREFIX")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/usr/local"));
    Some(prefix.join("share").join("dangerzone"))
}

#[cfg(target_os = "macos")]
fn resource_prefix_non_dev() -> Option<PathBuf> {
    let bin_path = std::env::current_exe().ok()?;
    let app_path = bin_path.parent()?.parent()?;
    Some(app_path.join("Resources").join("share"))
}

#[cfg(target_os = "windows")]
fn resource_prefix_non_dev() -> Option<PathBuf> {
    let exe_path = std::env::current_exe().ok()?;
    Some(exe_path.parent()?.join("share"))
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
fn resource_prefix_non_dev() -> Option<PathBuf> {
    None
}

/// Returns the Tesseract language data directory.
pub fn get_tessdata_dir() -> Result<PathBuf, UtilError> {
    if is_dev() || cfg!(target_os = "windows") || cfg!(target_os = "macos") {
        // Always use the tessdata path from the Dangerzone share directory, for
        // development builds, or on Windows/macOS platforms.
        return get_resource_path("tessdata").ok_or(UtilError::TessdataNotFound);
    }

    // On Linux, grab the Tesseract data from any of the following locations.
    // Some were found through trial and error, others are taken from the docs:
    //
    //     [...] Possibilities are /usr/share/tesseract-ocr/tessdata or
    //     /usr/share/tessdata or /usr/share/tesseract-ocr/4.00/tessdata. [1]
    //
    // [1] https://tesseract-ocr.github.io/tessdoc/Installation.html
    let tessdata_dirs = [
        Path::new("/usr/share/tessdata/"),
        Path::new("/usr/share/tesseract/tessdata/"),
        Path::new("/usr/share/tesseract-ocr/tessdata/"),
        Path::new("/usr/share/tesseract-ocr/4.00/tessdata/"),
        Path::new("/usr/share/tesseract-ocr/5/tessdata/"),
    ];

    for dir in tessdata_dirs {
        if dir.is_dir() {
            return Ok(dir.to_path_buf());
        }
    }

    Err(UtilError::TessdataNotFound)
}

/// Returns the Dangerzone version string, or `"unknown"` if it cannot be read.
pub fn get_version() -> String {
    match get_resource_path("version.txt") {
        Some(path) => std::fs::read_to_string(path)
            .map(|content| content.trim().to_string())
            .unwrap_or_else(|_| "unknown".to_string()),
        None => "unknown".to_string(),
    }
}

/// Removes control characters from a string, protecting a terminal emulator
/// from obscure control characters.
///
/// Unsafe characters are replaced by U+FFFD REPLACEMENT CHARACTER. Control
/// characters (Unicode General Category `C*`), as well as the line and
/// paragraph separators (`Zl`/`Zp`), are considered unsafe. When
/// `keep_newlines` is set, `\n` is preserved so multi-line text can be
/// sanitized without losing its structure.
pub fn replace_control_chars(untrusted_str: &str, keep_newlines: bool) -> String {
    fn is_safe(ch: char) -> bool {
        !matches!(
            get_general_category(ch),
            GeneralCategory::Control
                | GeneralCategory::Format
                | GeneralCategory::Surrogate
                | GeneralCategory::PrivateUse
                | GeneralCategory::Unassigned
                | GeneralCategory::LineSeparator
                | GeneralCategory::ParagraphSeparator
        )
    }

    let mut sanitized = String::with_capacity(untrusted_str.len());
    for ch in untrusted_str.chars() {
        if (keep_newlines && ch == '\n') || is_safe(ch) {
            sanitized.push(ch);
        } else {
            sanitized.push('\u{FFFD}');
        }
    }
    sanitized
}

/// Formats an error together with its source chain.
///
/// This is the Rust equivalent of `traceback.format_exception`.
pub fn format_exception(error: &dyn std::error::Error) -> String {
    let mut output = format!("{error}");
    let mut source = error.source();
    while let Some(err) = source {
        output.push_str("\nCaused by: ");
        output.push_str(&format!("{err}"));
        source = err.source();
    }
    output
}

/// Returns whether any of the given names is present in `/etc/os-release`,
/// on Linux.
pub fn linux_system_is(names: &[&str]) -> bool {
    if !cfg!(target_os = "linux") {
        return false;
    }
    let Ok(os_release) = std::fs::read_to_string("/etc/os-release") else {
        return false;
    };
    names.iter().any(|name| os_release.contains(name))
}

/// Generates a SOCKS5 proxy connection address that works on Tails.
///
/// Passing a random value for the username makes C Tor use stream isolation,
/// which allows to isolate unrelated streams, putting them on separate circuits
/// so that semantically unrelated traffic is not inadvertently made linkable
/// [1].
///
/// [1] https://spec.torproject.org/proposals/171-separate-streams.txt
pub fn get_tails_socks_proxy() -> String {
    use rand::RngCore;

    let mut bytes = [0u8; 8];
    rand::thread_rng().fill_bytes(&mut bytes);
    let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    format!("socks5://{hex}:0@127.0.0.1:9050")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replace_control_chars_replaces_unsafe_chars() {
        assert_eq!(replace_control_chars("a\u{0}b", false), "a\u{FFFD}b");
    }

    #[test]
    fn replace_control_chars_keeps_newline_when_requested() {
        assert_eq!(replace_control_chars("a\nb", true), "a\nb");
    }

    #[test]
    fn replace_control_chars_replaces_newline_by_default() {
        assert_eq!(replace_control_chars("a\nb", false), "a\u{FFFD}b");
    }

    #[test]
    fn replace_control_chars_replaces_line_separator() {
        assert_eq!(replace_control_chars("a\u{2028}b", false), "a\u{FFFD}b");
    }

    #[test]
    fn replace_control_chars_keeps_plain_text() {
        assert_eq!(replace_control_chars("hello world", false), "hello world");
    }

    #[test]
    fn get_architecture_returns_normalized_names() {
        let arch = get_architecture();
        assert!(arch == "amd64" || arch == "arm64");
    }

    #[test]
    fn format_exception_includes_source_chain() {
        let err = std::io::Error::other("inner");
        let wrapped = std::io::Error::other(err);
        let formatted = format_exception(&wrapped);
        assert!(formatted.contains("inner"));
    }

    #[test]
    fn get_tails_socks_proxy_matches_expected_format() {
        let proxy = get_tails_socks_proxy();
        assert!(proxy.starts_with("socks5://"));
        assert!(proxy.ends_with(":0@127.0.0.1:9050"));
    }
}
