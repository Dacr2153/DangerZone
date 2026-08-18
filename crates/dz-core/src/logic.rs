//! Shared state and functionality for the whole application.

use std::collections::BTreeMap;
use std::thread;

use crate::document::Document;
use crate::errors::DocumentFilenameError;
use crate::settings::{self, Settings};
use crate::util;

/// Errors raised while loading the Dangerzone core.
#[derive(Debug, thiserror::Error)]
pub enum LogicError {
    /// A required resource file could not be found.
    #[error("Resource not found: {0}")]
    ResourceNotFound(&'static str),
    /// An I/O error occurred while reading a resource.
    #[error(transparent)]
    Io(#[from] std::io::Error),
    /// A resource could not be parsed as JSON.
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

/// Converts documents in an isolated environment.
///
/// This is a stub interface for the `isolation_provider` module of the
/// original codebase. Real implementations are provided elsewhere.
pub trait IsolationProvider {
    /// Converts a document, reporting progress through the callback.
    fn convert(
        &self,
        document: &mut Document,
        ocr_lang: Option<&str>,
        stdout_callback: Option<&(dyn Fn(&str) + Sync)>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>>;

    /// The maximum number of documents that can be converted in parallel.
    fn get_max_parallel_conversions(&self) -> usize;
}

/// Singleton of shared state / functionality throughout the app.
pub struct DangerzoneCore<IP: IsolationProvider + Sync> {
    ocr_languages: BTreeMap<String, String>,
    settings: Settings,
    documents: Vec<Document>,
    isolation_provider: IP,
}

impl<IP: IsolationProvider + Sync> DangerzoneCore<IP> {
    /// Creates the shared core, loading the OCR languages and settings.
    pub fn new(isolation_provider: IP) -> Result<Self, LogicError> {
        // Terminal colors (colorama) are not needed in Rust; the log crate
        // leaves coloring to the logger implementation.

        // Languages supported by tesseract.
        let ocr_languages = load_ocr_languages()?;

        // Load settings, which also initializes the process-wide singleton.
        let settings = settings::read_settings().clone();

        Ok(Self {
            ocr_languages,
            settings,
            documents: Vec::new(),
            isolation_provider,
        })
    }

    /// The languages supported by tesseract, sorted by name.
    pub fn ocr_languages(&self) -> &BTreeMap<String, String> {
        &self.ocr_languages
    }

    /// Returns the Tesseract language code for an OCR language name, if any.
    ///
    /// The CLI and the GUI present OCR languages by their human-readable names
    /// ("English"), while Tesseract expects a short code ("eng"). The map
    /// loaded from `ocr-languages.json` holds the reverse direction, so the
    /// code is looked up by scanning for the matching name.
    pub fn get_ocr_language_code(&self, ocr_lang: &str) -> Option<String> {
        self.ocr_languages
            .iter()
            .find_map(|(code, name)| (name == ocr_lang).then(|| code.clone()))
    }

    /// The application settings.
    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    /// The isolation provider used to convert documents.
    pub fn isolation_provider(&self) -> &IP {
        &self.isolation_provider
    }

    /// The documents currently managed by the core.
    pub fn documents(&self) -> &[Document] {
        &self.documents
    }

    /// Mutable access to the document list.
    pub fn documents_mut(&mut self) -> &mut [Document] {
        &mut self.documents
    }

    /// Creates a document from an input filename and adds it to the core.
    pub fn add_document_from_filename(
        &mut self,
        input_filename: &str,
        output_filename: Option<&str>,
        archive: bool,
    ) -> Result<(), DocumentFilenameError> {
        let doc = Document::new_from_filename(input_filename, output_filename, archive)?;
        self.add_document(doc)
    }

    /// Adds a document to the core, failing if it was already added.
    pub fn add_document(&mut self, doc: Document) -> Result<(), DocumentFilenameError> {
        if self.documents.contains(&doc) {
            return Err(DocumentFilenameError::AddedDuplicateDocument);
        }
        self.documents.push(doc);
        Ok(())
    }

    /// Removes all documents from the core.
    pub fn clear_documents(&mut self) {
        log::debug!("Removing all documents");
        self.documents.clear();
    }

    /// Converts all documents, running at most as many conversions in parallel
    /// as the isolation provider supports.
    pub fn convert_documents(
        &mut self,
        ocr_lang: Option<&str>,
        stdout_callback: Option<&(dyn Fn(&str) + Sync)>,
    ) {
        let max_jobs = self
            .isolation_provider
            .get_max_parallel_conversions()
            .max(1);
        let num_docs = self.documents.len();
        if num_docs == 0 {
            return;
        }

        // Split the documents across scoped worker threads, mirroring the
        // ThreadPoolExecutor.map() of the original code.
        let chunk_size = num_docs.div_ceil(max_jobs);
        let provider = &self.isolation_provider;
        thread::scope(|scope| {
            for chunk in self.documents.chunks_mut(chunk_size) {
                scope.spawn(move || {
                    for document in chunk {
                        let result = provider.convert(document, ocr_lang, stdout_callback);
                        if let Err(error) = result {
                            log::error!(
                                "Unexpected error occurred while converting '{}': {error}",
                                document
                            );
                            document.mark_as_failed();
                        }
                    }
                });
            }
        });
    }

    /// The documents that have not been converted yet.
    pub fn get_unconverted_documents(&self) -> Vec<&Document> {
        self.documents
            .iter()
            .filter(|doc| doc.is_unconverted())
            .collect()
    }

    /// The documents that were converted successfully.
    pub fn get_safe_documents(&self) -> Vec<&Document> {
        self.documents.iter().filter(|doc| doc.is_safe()).collect()
    }

    /// The documents whose conversion failed.
    pub fn get_failed_documents(&self) -> Vec<&Document> {
        self.documents
            .iter()
            .filter(|doc| doc.is_failed())
            .collect()
    }

    /// The documents that are currently being converted.
    pub fn get_converting_documents(&self) -> Vec<&Document> {
        self.documents
            .iter()
            .filter(|doc| doc.is_converting())
            .collect()
    }
}

/// Loads the OCR languages from the `ocr-languages.json` resource.
fn load_ocr_languages() -> Result<BTreeMap<String, String>, LogicError> {
    let path = util::get_resource_path("ocr-languages.json")
        .ok_or(LogicError::ResourceNotFound("ocr-languages.json"))?;
    let content = std::fs::read_to_string(&path)?;
    let unsorted: std::collections::HashMap<String, String> = serde_json::from_str(&content)?;
    // A BTreeMap sorts the entries by key, mirroring `dict(sorted(...))`.
    Ok(unsorted.into_iter().collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::DocumentState;

    struct FakeProvider;

    impl IsolationProvider for FakeProvider {
        fn convert(
            &self,
            document: &mut Document,
            _ocr_lang: Option<&str>,
            _stdout_callback: Option<&(dyn Fn(&str) + Sync)>,
        ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
            document.mark_as_safe();
            Ok(())
        }

        fn get_max_parallel_conversions(&self) -> usize {
            2
        }
    }

    struct FailingProvider;

    impl IsolationProvider for FailingProvider {
        fn convert(
            &self,
            _document: &mut Document,
            _ocr_lang: Option<&str>,
            _stdout_callback: Option<&(dyn Fn(&str) + Sync)>,
        ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
            Err("boom".into())
        }

        fn get_max_parallel_conversions(&self) -> usize {
            2
        }
    }

    struct TempDir(std::path::PathBuf);

    impl TempDir {
        fn new() -> Self {
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let dir =
                std::env::temp_dir().join(format!("dz_logic_{}_{}", std::process::id(), nanos));
            std::fs::create_dir_all(&dir).unwrap();
            TempDir(dir)
        }

        fn file(&self, name: &str) -> std::path::PathBuf {
            self.0.join(name)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn make_document(temp: &TempDir, name: &str) -> Document {
        let path = temp.file(name);
        std::fs::write(&path, b"%PDF-1.4").unwrap();
        Document::new_from_filename(path.to_str().unwrap(), None, false).unwrap()
    }

    // Builds a core without touching the shared settings singleton or the
    // ocr-languages.json resource, so tests run in isolation.
    fn core_with<IP: IsolationProvider + Sync>(provider: IP) -> DangerzoneCore<IP> {
        DangerzoneCore {
            ocr_languages: BTreeMap::new(),
            settings: Settings::default(),
            documents: Vec::new(),
            isolation_provider: provider,
        }
    }

    #[test]
    fn add_same_document_twice_errors() {
        let temp = TempDir::new();
        let mut core = core_with(FakeProvider);
        let doc = make_document(&temp, "a.pdf");
        core.add_document(doc).unwrap();
        let duplicate = make_document(&temp, "a.pdf");
        let result = core.add_document(duplicate);
        assert!(matches!(
            result,
            Err(DocumentFilenameError::AddedDuplicateDocument)
        ));
    }

    #[test]
    fn convert_documents_marks_docs_as_safe() {
        let temp = TempDir::new();
        let mut core = core_with(FakeProvider);
        core.add_document(make_document(&temp, "a.pdf")).unwrap();
        core.add_document(make_document(&temp, "b.pdf")).unwrap();

        core.convert_documents(Some("eng"), None);

        assert_eq!(core.get_safe_documents().len(), 2);
        assert!(core.get_unconverted_documents().is_empty());
        assert!(core.get_failed_documents().is_empty());
    }

    #[test]
    fn convert_documents_marks_failed_docs_as_failed() {
        let temp = TempDir::new();
        let mut core = core_with(FailingProvider);
        core.add_document(make_document(&temp, "a.pdf")).unwrap();
        core.add_document(make_document(&temp, "b.pdf")).unwrap();

        core.convert_documents(Some("eng"), None);

        assert_eq!(core.get_failed_documents().len(), 2);
    }

    #[test]
    fn clear_documents_removes_everything() {
        let temp = TempDir::new();
        let mut core = core_with(FakeProvider);
        core.add_document(make_document(&temp, "a.pdf")).unwrap();
        core.clear_documents();
        assert!(core.get_unconverted_documents().is_empty());
    }

    #[test]
    fn get_state_filters_match_python() {
        let temp = TempDir::new();
        let mut core = core_with(FakeProvider);
        core.add_document(make_document(&temp, "a.pdf")).unwrap();
        core.add_document(make_document(&temp, "b.pdf")).unwrap();

        let mut doc = make_document(&temp, "c.pdf");
        doc.mark_as_converting();
        core.documents.push(doc);

        assert_eq!(core.get_unconverted_documents().len(), 2);
        assert_eq!(core.get_converting_documents().len(), 1);
        assert_eq!(
            core.get_converting_documents()[0].state(),
            DocumentState::Converting
        );
    }
}
