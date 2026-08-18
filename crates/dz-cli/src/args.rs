//! Argument validation helpers for the `dz-dangerzone` command-line interface.
//!
//! Corresponds to `dangerzone/cli/args.py`. Filename options and positional
//! arguments are normalized and validated before the conversion starts, and a
//! paranoid check ensures that no CLI option doubles as a file in the current
//! working directory.

use std::path::PathBuf;

use dz_core::document::Document;
use dz_core::errors::DocumentFilenameError;

/// Validates a single input filename, returning its normalized absolute form.
///
/// Mirrors `_validate_input_filename`: the filename is normalized and then
/// validated (the file must exist and be readable).
pub fn validate_input_filename(value: &str) -> Result<PathBuf, DocumentFilenameError> {
    let filename = Document::normalize_filename(value)?;
    Document::validate_input_filename(&filename)?;
    Ok(filename)
}

/// Validates a list of input filenames, returning their normalized forms.
pub fn validate_input_filenames(values: &[String]) -> Result<Vec<PathBuf>, DocumentFilenameError> {
    values
        .iter()
        .map(|value| validate_input_filename(value))
        .collect()
}

/// Validates the output filename, returning its normalized absolute form.
///
/// Mirrors `_validate_output_filename`: the filename is normalized and then
/// validated (it must be a writable PDF path).
pub fn validate_output_filename(value: &str) -> Result<PathBuf, DocumentFilenameError> {
    let filename = Document::normalize_filename(value)?;
    Document::validate_output_filename(&filename)?;
    Ok(filename)
}

/// Returns a security warning when a CLI option is also a file in the current
/// working directory.
///
/// Mirrors `check_suspicious_options`. An attacker-controlled filename that
/// starts with `-` could otherwise be parsed as an option. When the current
/// directory cannot be listed, no warning is produced.
pub fn check_suspicious_options(args: &[String]) -> Option<String> {
    let options: std::collections::HashSet<&str> = args
        .iter()
        .map(String::as_str)
        .filter(|arg| arg.starts_with('-'))
        .collect();
    if options.is_empty() {
        return None;
    }

    let files: std::collections::HashSet<String> = match std::fs::read_dir(".") {
        Ok(entries) => entries
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.file_name().to_string_lossy().to_string())
            .collect(),
        // If we cannot list files in the current working directory, we are
        // probably in an unlinked directory. Dangerzone should still work in
        // this case, so no warning is produced.
        Err(_) => return None,
    };

    let mut intersection: Vec<&str> = options
        .iter()
        .filter(|option| files.contains(**option))
        .copied()
        .collect();
    if intersection.is_empty() {
        return None;
    }
    intersection.sort_unstable();
    Some(format!(
        "Security: Detected CLI options that are also present as files in the current working \
         directory: {}",
        intersection.join(", ")
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suspicious_options_flags_files_matching_options() {
        let cwd = std::env::current_dir().unwrap();
        let message = check_suspicious_options(&["--archive".to_string()]);
        if cwd.join("--archive").exists() {
            assert!(message.unwrap().contains("--archive"));
        } else {
            assert!(message.is_none());
        }
    }

    #[test]
    fn no_suspicious_options_when_args_are_plain_files() {
        let message =
            check_suspicious_options(&["report.pdf".to_string(), "notes.txt".to_string()]);
        assert!(message.is_none());
    }
}
