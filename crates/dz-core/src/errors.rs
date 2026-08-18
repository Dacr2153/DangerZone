//! Error types shared across the Dangerzone core library.
//!
//! These mirror the exception hierarchy of the original Python codebase. The
//! Python classes have been flattened into nested `thiserror` enums, since Rust
//! does not support exception subclassing.

/// Errors raised while validating or manipulating document filenames.
///
/// Corresponds to `DocumentFilenameException` and its subclasses.
#[derive(Debug, thiserror::Error)]
pub enum DocumentFilenameError {
    /// A document was added twice.
    #[error("A document was added twice")]
    AddedDuplicateDocument,
    /// The input file does not exist.
    #[error("Input file not found: make sure you typed it correctly.")]
    InputFileNotFound,
    /// The input file exists but cannot be opened for reading.
    #[error("You don't have permission to open the input file.")]
    InputFileNotReadable,
    /// The output file is not a PDF.
    #[error("Safe PDF filename must end in '.pdf'")]
    NonPdfOutputFile,
    /// The output filename contains an illegal character for this platform.
    #[error("Illegal character: {0}")]
    IllegalOutputFilename(String),
    /// The output directory is not writable.
    #[error("Safe PDF filename is not writable")]
    UnwriteableOutputDir,
    /// The input filename was read before it was set.
    #[error("Input filename has not been set yet.")]
    NotSetInputFilename,
    /// The output filename was read before it was set.
    #[error("Output filename has not been set yet.")]
    NotSetOutputFilename,
    /// The output directory does not exist.
    #[error("Output directory does not exist")]
    NonExistantOutputDir,
    /// The specified output path is not a directory.
    #[error("Specified output directory is actually not a directory")]
    OutputDirIsNotDir,
    /// The archive directory for unsafe documents cannot be created.
    #[error("Archive directory for storing unsafe documents cannot be created.")]
    UnwriteableArchiveDir,
    /// A suffix cannot be set after an output filename was set.
    #[error("Cannot set a suffix after setting an output filename")]
    SuffixNotApplicable,
    /// An underlying I/O error occurred.
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// Runs `f`, logging a detailed error on dev environments and surfacing the
/// error to the caller.
///
/// This is the Rust equivalent of the `handle_document_errors` decorator. The
/// Python version echoed the message and exited the process; a library cannot
/// do either, so the caller decides how to present the returned error.
pub fn handle_document_errors<T>(
    dev: bool,
    f: impl FnOnce() -> Result<T, DocumentFilenameError>,
) -> Result<T, DocumentFilenameError> {
    match f() {
        Ok(value) => Ok(value),
        Err(e) => {
            if dev {
                // Show the full details only on dev environments.
                log::error!("An exception occurred while validating a document: {e:?}");
            }
            Err(e)
        }
    }
}

/// Errors found while checking the signatures of the container image.
///
/// Corresponds to `dangerzone/updater/errors.SignatureError` and its
/// subclasses. The type lives here so that both the runtime (which verifies a
/// local image before starting a container) and the updater crate can use it
/// without a dependency cycle.
#[derive(Debug, thiserror::Error)]
pub enum SignatureError {
    /// No remote signatures were found on the container registry.
    #[error("{0}")]
    NoRemoteSignatures(String),
    /// An error occurred when checking the validity of the signatures.
    #[error("{0}")]
    SignatureVerificationError(String),
    /// The signatures do not match the expected format.
    #[error("The signatures do not match the expected format")]
    SignatureExtractionError,
    /// The signatures folder for the specific public key doesn't exist.
    #[error("{0}")]
    SignaturesFolderDoesNotExist(String),
    /// The signatures do not share the expected image digest.
    #[error("{0}")]
    SignatureMismatch(String),
    /// Unable to verify the local signatures as they cannot be found.
    #[error("{0}")]
    LocalSignatureNotFound(String),
    /// Cosign is not installed.
    #[error("Cosign is not installed")]
    CosignNotInstalledError,
}

/// Errors related to the container image and the container runtime.
///
/// Corresponds to `ContainerException` and its subclasses.
#[derive(Debug, thiserror::Error)]
pub enum ContainerError {
    /// The container image is not present.
    #[error("Image is not present")]
    ImageNotPresent,
    /// Multiple container images were found.
    #[error("Multiple images were found")]
    MultipleImagesFound,
    /// The container image could not be installed.
    #[error("The container image could not be installed")]
    ImageInstallation,
    /// A container technology is not installed.
    #[error("{0} is not installed")]
    NoContainerTech(String),
    /// A container technology is installed but not available.
    #[error("{container_tech} is not available")]
    NotAvailableContainerTech {
        /// The name of the container technology.
        container_tech: String,
        /// The underlying availability error.
        error: String,
    },
    /// The container runtime is not supported.
    #[error("Unsupported container runtime")]
    UnsupportedContainerRuntime,
    /// Pulling the container image failed.
    #[error("Container pull failed")]
    ContainerPull,
    /// The signatures of the container image could not be verified.
    #[error(transparent)]
    Signature(#[from] SignatureError),
    /// An underlying I/O error occurred.
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// Raised when another Podman machine is running and must be stopped first.
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct OtherMachineRunningError(pub String);

/// Errors related to the Windows Subsystem for Linux.
#[derive(Debug, thiserror::Error)]
pub enum WslError {
    /// The WSL installation failed.
    #[error("Windows Subsystem for Linux installation failed")]
    InstallFailed,
    /// WSL is not installed.
    #[error("{0}")]
    NotInstalled(String),
    /// WSL was installed and a reboot is required.
    #[error("{0}")]
    InstallNeedsReboot(String),
    /// A process could not be launched through the Windows shell.
    #[error(transparent)]
    WinShellExec(#[from] WinShellExecError),
}

/// Errors raised when launching a process through the Windows shell.
#[derive(Debug, thiserror::Error)]
pub enum WinShellExecError {
    /// The process timed out.
    #[error("Timeout expired for process")]
    TimeoutExpired,
    /// `ShellExecuteEx` failed to start.
    #[error("ShellExecuteEx failed to start")]
    StartFailure,
    /// The process started without a handle.
    #[error("Process started without a handle")]
    NoHandle,
    /// A generic process execution error.
    #[error("Process execution error")]
    ProcessError,
}

/// Raised when the user declined to enable updates but no container image is
/// available to convert documents.
#[derive(Debug, thiserror::Error)]
#[error(
    "No container image is available. Updates must be enabled to download the \
     container and use Dangerzone."
)]
pub struct UpdaterDisabledNoContainer;

/// Raised when an isolation provider that must never run on a real system is
/// invoked.
#[derive(Debug, thiserror::Error)]
#[error("This isolation provider is UNSAFE and should never be called in a non-testing system.")]
pub struct UnsafeIsolationProvider;

/// Generic errors raised during the startup or shutdown task runners.
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct StartupError(pub String);

/// Unified error type surfaced by the startup/shutdown task runners.
///
/// Corresponds to the catch-all exception handling performed by `Runner.run`
/// in the original code.
#[derive(Debug, thiserror::Error)]
pub enum TaskError {
    /// A document-related filename error.
    #[error(transparent)]
    Document(#[from] DocumentFilenameError),
    /// A container-related error.
    #[error(transparent)]
    Container(#[from] ContainerError),
    /// A Windows Subsystem for Linux error.
    #[error(transparent)]
    Wsl(#[from] WslError),
    /// A startup error.
    #[error(transparent)]
    Startup(#[from] StartupError),
    /// Another Podman machine is running and must be stopped first.
    #[error(transparent)]
    OtherMachineRunning(#[from] OtherMachineRunningError),
    /// The user declined the initial container download.
    #[error(transparent)]
    UpdaterDisabledNoContainer(#[from] UpdaterDisabledNoContainer),
    /// The update check itself failed.
    #[error("{0}")]
    UpdateCheck(String),
    /// An underlying I/O error occurred.
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn document_error_displays_python_faithful_message() {
        let err = DocumentFilenameError::NonPdfOutputFile;
        assert_eq!(err.to_string(), "Safe PDF filename must end in '.pdf'");
    }

    #[test]
    fn illegal_output_filename_error_includes_matched_character() {
        let err = DocumentFilenameError::IllegalOutputFilename("/".to_string());
        assert_eq!(err.to_string(), "Illegal character: /");
    }

    #[test]
    fn not_available_container_tech_stores_error_payload() {
        let err = ContainerError::NotAvailableContainerTech {
            container_tech: "podman".to_string(),
            error: "permission denied".to_string(),
        };
        assert_eq!(err.to_string(), "podman is not available");
    }

    #[test]
    fn no_container_tech_error_includes_runtime_name() {
        let err = ContainerError::NoContainerTech("docker".to_string());
        assert_eq!(err.to_string(), "docker is not installed");
    }

    #[test]
    fn handle_document_errors_propagates_success() {
        let result = handle_document_errors(false, || Ok(42));
        assert_eq!(result.unwrap(), 42);
    }

    #[test]
    fn handle_document_errors_propagates_error() {
        let result: Result<(), _> =
            handle_document_errors(false, || Err(DocumentFilenameError::InputFileNotFound));
        assert_eq!(
            result.unwrap_err().to_string(),
            "Input file not found: make sure you typed it correctly."
        );
    }
}
