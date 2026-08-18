//! State and validation logic for a single document being converted.

use regex::Regex;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use crate::errors::DocumentFilenameError;
use crate::util;

/// Suffix appended to the safe (converted) version of a document.
pub const SAFE_EXTENSION: &str = "-safe.pdf";

/// Subdirectory where the original (unsafe) documents are archived.
pub const ARCHIVE_SUBDIR: &str = "unsafe";

/// Illegal filename characters on Windows.
static ILLEGAL_WINDOWS_CHARS: LazyLock<Regex> = LazyLock::new(|| {
    // SAFETY: this is a compile-time-constant pattern, so it can never fail.
    Regex::new(r#"["*/:<>?\\|]"#).expect("static regex is valid")
});

/// Illegal filename characters on POSIX systems.
static ILLEGAL_POSIX_CHARS: LazyLock<Regex> = LazyLock::new(|| {
    // SAFETY: this is a compile-time-constant pattern, so it can never fail.
    Regex::new(r"[\\]").expect("static regex is valid")
});

/// The conversion state of a document.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocumentState {
    /// The document has not been converted yet.
    Unconverted,
    /// The document is currently being converted.
    Converting,
    /// The document was converted successfully.
    Safe,
    /// The conversion of this document failed.
    Failed,
}

/// Tracks the state of a single document, and validates its info.
#[derive(Debug)]
pub struct Document {
    id: String,
    input_filename: Option<PathBuf>,
    output_filename: Option<PathBuf>,
    suffix: String,
    archive_after_conversion: bool,
    state: DocumentState,
}

impl Document {
    /// Creates a new document, mirroring the Python constructor.
    ///
    /// `input_filename` and `output_filename`, when given, are normalized and
    /// validated. Enabling `archive` validates that the default archive
    /// directory can be created.
    pub fn new(
        input_filename: Option<&str>,
        output_filename: Option<&str>,
        suffix: &str,
        archive: bool,
    ) -> Result<Self, DocumentFilenameError> {
        let mut doc = Self {
            id: generate_id(),
            input_filename: None,
            output_filename: None,
            suffix: suffix.to_string(),
            archive_after_conversion: false,
            state: DocumentState::Unconverted,
        };

        if let Some(filename) = input_filename {
            doc.set_input_filename(filename)?;
            if let Some(out) = output_filename {
                doc.set_output_filename(out)?;
            }
        }
        doc.set_archive_after_conversion(archive)?;

        Ok(doc)
    }

    /// Creates a new document from an input filename.
    pub fn new_from_filename(
        input_filename: &str,
        output_filename: Option<&str>,
        archive: bool,
    ) -> Result<Self, DocumentFilenameError> {
        Self::new(
            Some(input_filename),
            output_filename,
            SAFE_EXTENSION,
            archive,
        )
    }

    /// The URL-safe identifier assigned to this document.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Normalizes a filename to an absolute path.
    pub fn normalize_filename(filename: &str) -> Result<PathBuf, DocumentFilenameError> {
        Ok(std::path::absolute(filename)?)
    }

    /// Ensures the input file exists and can be opened for reading.
    pub fn validate_input_filename(filename: &Path) -> Result<(), DocumentFilenameError> {
        match std::fs::File::open(filename) {
            Ok(_) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                Err(DocumentFilenameError::InputFileNotFound)
            }
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                Err(DocumentFilenameError::InputFileNotReadable)
            }
            Err(e) => Err(DocumentFilenameError::Io(e)),
        }
    }

    /// Validates the output filename: PDF extension, illegal characters for the
    /// current platform, and a writable parent directory.
    pub fn validate_output_filename(filename: &Path) -> Result<(), DocumentFilenameError> {
        if !filename.to_string_lossy().ends_with(".pdf") {
            return Err(DocumentFilenameError::NonPdfOutputFile);
        }

        let is_windows = cfg!(target_os = "windows");
        let file_name = filename
            .file_name()
            .ok_or(DocumentFilenameError::UnwriteableOutputDir)?
            .to_string_lossy();

        let illegal_chars: &Regex = if is_windows {
            &ILLEGAL_WINDOWS_CHARS
        } else {
            &ILLEGAL_POSIX_CHARS
        };

        if is_windows || cfg!(target_os = "macos") {
            if let Some(matched) = illegal_chars.find(&file_name) {
                // The filename contains illegal characters.
                return Err(DocumentFilenameError::IllegalOutputFilename(
                    matched.as_str().to_string(),
                ));
            }
        }

        let parent = filename
            .parent()
            .ok_or(DocumentFilenameError::UnwriteableOutputDir)?;
        if !is_writable_dir(parent) {
            return Err(DocumentFilenameError::UnwriteableOutputDir);
        }

        Ok(())
    }

    /// Checks that the default archive directory can be created.
    fn validate_default_archive_dir(&self) -> Result<(), DocumentFilenameError> {
        let archive_dir = self.default_archive_dir()?;
        let parent = archive_dir
            .parent()
            .ok_or(DocumentFilenameError::UnwriteableArchiveDir)?;
        if !is_writable_dir(parent) {
            return Err(DocumentFilenameError::UnwriteableArchiveDir);
        }
        Ok(())
    }

    /// The normalized input filename.
    pub fn input_filename(&self) -> Result<&Path, DocumentFilenameError> {
        self.input_filename
            .as_deref()
            .ok_or(DocumentFilenameError::NotSetInputFilename)
    }

    /// Sets and validates the input filename.
    pub fn set_input_filename(&mut self, filename: &str) -> Result<(), DocumentFilenameError> {
        let normalized = Self::normalize_filename(filename)?;
        Self::validate_input_filename(&normalized)?;
        self.input_filename = Some(normalized);
        self.announce_id();
        Ok(())
    }

    /// The output filename, computing the default from the input filename when
    /// none was explicitly set.
    pub fn output_filename(&self) -> Result<PathBuf, DocumentFilenameError> {
        match &self.output_filename {
            Some(path) => Ok(path.clone()),
            None if self.input_filename.is_some() => self.default_output_filename(),
            None => Err(DocumentFilenameError::NotSetOutputFilename),
        }
    }

    /// Sets and validates the output filename.
    pub fn set_output_filename(&mut self, filename: &str) -> Result<(), DocumentFilenameError> {
        let normalized = Self::normalize_filename(filename)?;
        Self::validate_output_filename(&normalized)?;
        self.output_filename = Some(normalized);
        Ok(())
    }

    /// The output filename with any control characters removed, protecting a
    /// terminal emulator from obscure control characters.
    pub fn sanitized_output_filename(&self) -> Result<String, DocumentFilenameError> {
        Ok(util::replace_control_chars(
            &self.output_filename()?.to_string_lossy(),
            false,
        ))
    }

    /// The suffix appended to the output filename.
    pub fn suffix(&self) -> &str {
        &self.suffix
    }

    /// Sets the output filename suffix.
    ///
    /// Fails if an output filename has already been set, since the suffix can
    /// no longer be applied.
    pub fn set_suffix(&mut self, suffix: &str) -> Result<(), DocumentFilenameError> {
        if self.output_filename.is_none() {
            self.suffix = suffix.to_string();
            Ok(())
        } else {
            Err(DocumentFilenameError::SuffixNotApplicable)
        }
    }

    /// Whether the original document should be archived after conversion.
    pub fn archive_after_conversion(&self) -> bool {
        self.archive_after_conversion
    }

    /// Sets whether the original document should be archived after conversion.
    ///
    /// Enabling archiving validates that the default archive directory can be
    /// created.
    pub fn set_archive_after_conversion(
        &mut self,
        enabled: bool,
    ) -> Result<(), DocumentFilenameError> {
        if enabled {
            self.validate_default_archive_dir()?;
            self.archive_after_conversion = true;
        } else {
            self.archive_after_conversion = false;
        }
        Ok(())
    }

    /// Moves the original document to a subdirectory, preventing the user from
    /// mistakenly opening the unsafe (original) document.
    pub fn archive(&self) -> Result<(), DocumentFilenameError> {
        let archive_dir = self.default_archive_dir()?;
        let input = self.input_filename()?;
        let old_file_name = input.file_name().ok_or_else(|| {
            DocumentFilenameError::Io(std::io::Error::other("input filename has no file name"))
        })?;
        let new_file_path = archive_dir.join(old_file_name);
        log::debug!("Archiving doc {} to {}", self.id, new_file_path.display());
        std::fs::create_dir_all(&archive_dir)?;
        // On Windows, moving the file will fail if it already exists.
        let _ = std::fs::remove_file(&new_file_path);
        std::fs::rename(input, &new_file_path)?;
        Ok(())
    }

    /// The directory where the original document will be archived.
    pub fn default_archive_dir(&self) -> Result<PathBuf, DocumentFilenameError> {
        let input = self.input_filename()?;
        Ok(input.parent().unwrap_or(Path::new("")).join(ARCHIVE_SUBDIR))
    }

    /// The default output filename, derived from the input filename and the
    /// configured suffix.
    pub fn default_output_filename(&self) -> Result<PathBuf, DocumentFilenameError> {
        let input = self.input_filename()?;
        let parent = input.parent().unwrap_or(Path::new(""));
        let stem = input.file_stem().unwrap_or(input.as_ref());
        Ok(parent.join(format!("{}{}", stem.to_string_lossy(), self.suffix)))
    }

    /// Logs the ID assignment for this document.
    fn announce_id(&self) {
        let input = self.input_filename().unwrap_or(Path::new(""));
        let sanitized = util::replace_control_chars(&input.to_string_lossy(), false);
        log::info!("Assigning ID '{}' to doc '{}'", self.id, sanitized);
    }

    /// Moves the output to the given directory, keeping the same file name.
    pub fn set_output_dir(&mut self, path: &str) -> Result<(), DocumentFilenameError> {
        let output = self.output_filename()?;
        let old_file_name = output.file_name().ok_or_else(|| {
            DocumentFilenameError::Io(std::io::Error::other("output filename has no file name"))
        })?;

        let new_path = std::path::absolute(path)?;
        if !new_path.exists() {
            return Err(DocumentFilenameError::NonExistantOutputDir);
        }
        if !new_path.is_dir() {
            return Err(DocumentFilenameError::OutputDirIsNotDir);
        }
        if !is_writable_dir(&new_path) {
            return Err(DocumentFilenameError::UnwriteableOutputDir);
        }

        self.output_filename = Some(new_path.join(old_file_name));
        Ok(())
    }

    /// Whether the document has not been converted yet.
    pub fn is_unconverted(&self) -> bool {
        self.state == DocumentState::Unconverted
    }

    /// Whether the document is currently being converted.
    pub fn is_converting(&self) -> bool {
        self.state == DocumentState::Converting
    }

    /// Whether the conversion of this document has failed.
    pub fn is_failed(&self) -> bool {
        self.state == DocumentState::Failed
    }

    /// Whether the document was converted successfully.
    pub fn is_safe(&self) -> bool {
        self.state == DocumentState::Safe
    }

    /// The current conversion state.
    pub fn state(&self) -> DocumentState {
        self.state
    }

    /// Marks the document as converting.
    pub fn mark_as_converting(&mut self) {
        log::debug!("Marking doc {} as 'converting'", self.id);
        self.state = DocumentState::Converting;
    }

    /// Marks the document as failed.
    pub fn mark_as_failed(&mut self) {
        log::debug!("Marking doc {} as 'failed'", self.id);
        self.state = DocumentState::Failed;
    }

    /// Marks the document as safe.
    pub fn mark_as_safe(&mut self) {
        log::debug!("Marking doc {} as 'safe'", self.id);
        self.state = DocumentState::Safe;
    }
}

impl PartialEq for Document {
    /// Two documents are equal when they share the same normalized input file.
    fn eq(&self, other: &Self) -> bool {
        match (&self.input_filename, &other.input_filename) {
            (Some(a), Some(b)) => a == b,
            _ => false,
        }
    }
}

impl Eq for Document {}

impl Hash for Document {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.input_filename.hash(state);
    }
}

impl std::fmt::Display for Document {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.input_filename {
            Some(path) => write!(f, "{}", path.display()),
            None => write!(f, "<unset>"),
        }
    }
}

/// Generates a document ID equivalent to `secrets.token_urlsafe(6)[0:6]`.
///
/// Six random bytes are base64url-encoded (yielding 8 characters) and the
/// first six characters are kept.
fn generate_id() -> String {
    use rand::RngCore;

    let mut bytes = [0u8; 6];
    rand::thread_rng().fill_bytes(&mut bytes);
    let encoded = base64_url_encode(&bytes);
    encoded[..6].to_string()
}

/// Base64url-encodes the given bytes, without padding.
fn base64_url_encode(data: &[u8]) -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let triple = (chunk[0] as u32) << 16
            | (chunk.get(1).copied().unwrap_or(0) as u32) << 8
            | chunk.get(2).copied().unwrap_or(0) as u32;
        out.push(ALPHABET[(triple >> 18) as usize & 0x3F] as char);
        out.push(ALPHABET[(triple >> 12) as usize & 0x3F] as char);
        if chunk.len() > 1 {
            out.push(ALPHABET[(triple >> 6) as usize & 0x3F] as char);
        }
        if chunk.len() > 2 {
            out.push(ALPHABET[triple as usize & 0x3F] as char);
        }
    }
    out
}

/// Probes whether a directory is writable by creating and removing a unique
/// temporary file inside it.
///
/// This is the portable Rust equivalent of `os.access(path, os.W_OK)`, which
/// the standard library does not provide directly.
fn is_writable_dir(dir: &Path) -> bool {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let probe = dir.join(format!(".dz_write_probe_{}_{}", std::process::id(), nanos));
    match std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .open(&probe)
    {
        Ok(file) => {
            drop(file);
            let _ = std::fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let dir =
                std::env::temp_dir().join(format!("dz_test_{}_{}", std::process::id(), nanos));
            fs::create_dir_all(&dir).unwrap();
            TempDir(dir)
        }

        fn path(&self) -> &Path {
            &self.0
        }

        fn file(&self, name: &str) -> PathBuf {
            self.0.join(name)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn write_pdf(temp: &TempDir) -> PathBuf {
        let path = temp.file("input.pdf");
        fs::write(&path, b"%PDF-1.4").unwrap();
        path
    }

    #[test]
    fn new_document_starts_unconverted() {
        let temp = TempDir::new();
        let path = write_pdf(&temp);
        let doc = Document::new_from_filename(path.to_str().unwrap(), None, false).unwrap();
        assert!(doc.is_unconverted());
    }

    #[test]
    fn new_document_with_missing_input_errors() {
        let temp = TempDir::new();
        let missing = temp.file("missing.pdf");
        let result = Document::new_from_filename(missing.to_str().unwrap(), None, false);
        assert!(matches!(
            result,
            Err(DocumentFilenameError::InputFileNotFound)
        ));
    }

    #[test]
    fn new_document_with_non_pdf_output_errors() {
        let temp = TempDir::new();
        let path = write_pdf(&temp);
        let result = Document::new_from_filename(path.to_str().unwrap(), Some("out.txt"), false);
        assert!(matches!(
            result,
            Err(DocumentFilenameError::NonPdfOutputFile)
        ));
    }

    #[test]
    fn default_output_filename_appends_suffix() {
        let temp = TempDir::new();
        let path = write_pdf(&temp);
        let doc = Document::new_from_filename(path.to_str().unwrap(), None, false).unwrap();
        let expected = temp.file("input-safe.pdf");
        assert_eq!(doc.output_filename().unwrap(), expected);
    }

    #[test]
    fn explicit_output_filename_is_preserved() {
        let temp = TempDir::new();
        let input = write_pdf(&temp);
        let output = temp.file("custom.pdf");
        let doc = Document::new_from_filename(
            input.to_str().unwrap(),
            Some(output.to_str().unwrap()),
            false,
        )
        .unwrap();
        assert_eq!(doc.output_filename().unwrap(), output);
    }

    #[test]
    fn set_suffix_errors_when_output_set() {
        let temp = TempDir::new();
        let input = write_pdf(&temp);
        let output = temp.file("custom.pdf");
        let mut doc = Document::new_from_filename(
            input.to_str().unwrap(),
            Some(output.to_str().unwrap()),
            false,
        )
        .unwrap();
        let result = doc.set_suffix("different.pdf");
        assert!(matches!(
            result,
            Err(DocumentFilenameError::SuffixNotApplicable)
        ));
    }

    #[test]
    fn set_suffix_succeeds_when_output_not_set() {
        let temp = TempDir::new();
        let input = write_pdf(&temp);
        let mut doc = Document::new_from_filename(input.to_str().unwrap(), None, false).unwrap();
        doc.set_suffix("-custom.pdf").unwrap();
        assert_eq!(doc.suffix(), "-custom.pdf");
    }

    #[test]
    fn documents_are_equal_by_input_path() {
        let temp = TempDir::new();
        let input = write_pdf(&temp);
        let a = Document::new_from_filename(input.to_str().unwrap(), None, false).unwrap();
        let b = Document::new_from_filename(input.to_str().unwrap(), Some("x.pdf"), false);
        assert!(matches!(b, Ok(ref doc) if doc == &a));
    }

    #[test]
    fn set_output_dir_with_nonexistent_dir_errors() {
        let temp = TempDir::new();
        let input = write_pdf(&temp);
        let mut doc = Document::new_from_filename(input.to_str().unwrap(), None, false).unwrap();
        let missing = temp.path().join("missing-dir");
        let result = doc.set_output_dir(missing.to_str().unwrap());
        assert!(matches!(
            result,
            Err(DocumentFilenameError::NonExistantOutputDir)
        ));
    }

    #[test]
    fn state_transitions_mirror_python() {
        let temp = TempDir::new();
        let input = write_pdf(&temp);
        let mut doc = Document::new_from_filename(input.to_str().unwrap(), None, false).unwrap();
        doc.mark_as_converting();
        assert!(doc.is_converting());
        doc.mark_as_safe();
        assert!(doc.is_safe());
        doc.mark_as_failed();
        assert!(doc.is_failed());
    }

    #[test]
    fn document_id_has_six_urlsafe_characters() {
        let temp = TempDir::new();
        let input = write_pdf(&temp);
        let doc = Document::new_from_filename(input.to_str().unwrap(), None, false).unwrap();
        assert_eq!(doc.id().len(), 6);
        assert!(doc
            .id()
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'));
    }
}
