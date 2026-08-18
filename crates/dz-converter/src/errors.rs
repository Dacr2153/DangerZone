//! Conversion error types and constants.
//!
//! These mirror the error codes used by the container-side conversion
//! process. The actual error classes live here (in the client) rather than
//! being shared with the container image; only the integer error codes cross
//! the boundary.

/// Errors start at 128 for conversion-related issues.
pub const ERROR_SHIFT: u32 = 128;
/// The maximum number of pages a converted document may have.
pub const MAX_PAGES: u32 = 10_000;
/// The maximum width of a converted page, in pixels.
pub const MAX_PAGE_WIDTH: u32 = 10_000;
/// The maximum height of a converted page, in pixels.
pub const MAX_PAGE_HEIGHT: u32 = 10_000;
/// The default resolution of a converted page, in pixels per inch.
pub const DEFAULT_DPI: u32 = 150;
/// The number of bytes used to encode an integer on the conversion boundary.
pub const INT_BYTES: usize = 2;

// Exit codes emitted by the conversion subprocess. These mirror the
// `error_code` class attributes of the Python exception classes.
/// Exit code of `QubesQrexecFailed` (a qrexec error, hence no `ERROR_SHIFT`).
const ERROR_QUBES_QREXEC_FAILED: u32 = 126;
/// Exit code of `DocFormatUnsupported`.
const ERROR_DOC_FORMAT_UNSUPPORTED: u32 = ERROR_SHIFT + 10;
/// Exit code of `DocFormatUnsupportedHWPQubes`.
const ERROR_DOC_FORMAT_UNSUPPORTED_HWP_QUBES: u32 = ERROR_SHIFT + 16;
/// Exit code of `LibreofficeFailure`.
const ERROR_LIBREOFFICE_FAILURE: u32 = ERROR_SHIFT + 20;
/// Exit code of `DocCorruptedException`.
const ERROR_DOC_CORRUPTED: u32 = ERROR_SHIFT + 30;
/// Exit code of `PagesException`.
const ERROR_PAGES: u32 = ERROR_SHIFT + 40;
/// Exit code of `NoPageCountException`.
const ERROR_NO_PAGE_COUNT: u32 = ERROR_SHIFT + 41;
/// Exit code of `MaxPagesException`.
const ERROR_MAX_PAGES: u32 = ERROR_SHIFT + 42;
/// Exit code of `MaxPageWidthException`.
const ERROR_MAX_PAGE_WIDTH: u32 = ERROR_SHIFT + 44;
/// Exit code of `MaxPageHeightException`.
const ERROR_MAX_PAGE_HEIGHT: u32 = ERROR_SHIFT + 45;
/// Exit code of `PageCountMismatch`.
const ERROR_PAGE_COUNT_MISMATCH: u32 = ERROR_SHIFT + 46;
/// Exit code of `UnexpectedConversionError`.
const ERROR_UNEXPECTED: u32 = ERROR_SHIFT + 100;

/// Raised when the process spawned for the conversion has exited early.
///
/// Corresponds to `ConverterProcException`. This is intentionally a separate
/// type from [`ConversionError`]: in the original code it is *not* a subclass
/// of `ConversionException`, and the `doc_to_pixels` machinery uses it to
/// signal a broken subprocess before translating it into a proper conversion
/// error via the process exit code.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("The process spawned for the conversion has exited early")]
pub struct ConverterProcError;

/// Errors raised while converting a document.
///
/// Corresponds to `ConversionException` and its subclasses. The Python class
/// hierarchy has been flattened into a single enum; each variant carries the
/// `error_code` and `error_message` class attributes of the original class.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConversionError {
    /// The base `ConversionException` ("Unspecified error").
    Unspecified,
    /// `QubesQrexecFailed`: could not start a disposable qube.
    QubesQrexecFailed,
    /// `DocFormatUnsupported`: the document format is not supported.
    DocFormatUnsupported,
    /// `DocFormatUnsupportedHWPQubes`: HWP/HWPX are not supported on Qubes.
    DocFormatUnsupportedHwpQubes,
    /// `LibreofficeFailure`: conversion to PDF with LibreOffice failed.
    LibreofficeFailure,
    /// `DocCorruptedException`: the document appears to be corrupted.
    DocCorruptedException,
    /// `PagesException`: a page-related error occurred.
    Pages,
    /// `NoPageCountException`: the page count could not be extracted.
    NoPageCount,
    /// `MaxPagesException`: the document has too many pages.
    MaxPages,
    /// `MaxPageWidthException`: a page exceeded the maximum width.
    MaxPageWidth,
    /// `MaxPageHeightException`: a page exceeded the maximum height.
    MaxPageHeight,
    /// `PageCountMismatch`: the page count changed during conversion.
    PageCountMismatch,
    /// `UnexpectedConversionError`: an unexpected error occurred.
    ///
    /// The message is configurable, mirroring the per-instance `error_message`
    /// that the Python constructor accepts.
    UnexpectedConversion {
        /// The error message shown to the user.
        message: String,
    },
    /// An exit code that does not match any known conversion error.
    ///
    /// Corresponds to `UnexpectedConversionError("Unknown error code '..'")`,
    /// which the original code raises for unrecognized exit codes.
    UnknownErrorCode(u32),
}

impl ConversionError {
    /// Creates an `UnexpectedConversionError` with a custom message.
    ///
    /// Corresponds to `UnexpectedConversionError(error_message)`.
    pub fn unexpected_conversion(message: impl Into<String>) -> Self {
        ConversionError::UnexpectedConversion {
            message: message.into(),
        }
    }

    /// The `error_code` class attribute of the underlying Python class.
    pub fn error_code(&self) -> u32 {
        match self {
            // No ERROR_SHIFT since this is a qrexec error.
            ConversionError::QubesQrexecFailed => ERROR_QUBES_QREXEC_FAILED,
            ConversionError::DocFormatUnsupported => ERROR_DOC_FORMAT_UNSUPPORTED,
            ConversionError::DocFormatUnsupportedHwpQubes => ERROR_DOC_FORMAT_UNSUPPORTED_HWP_QUBES,
            ConversionError::LibreofficeFailure => ERROR_LIBREOFFICE_FAILURE,
            ConversionError::DocCorruptedException => ERROR_DOC_CORRUPTED,
            ConversionError::Pages => ERROR_PAGES,
            ConversionError::NoPageCount => ERROR_NO_PAGE_COUNT,
            ConversionError::MaxPages => ERROR_MAX_PAGES,
            ConversionError::MaxPageWidth => ERROR_MAX_PAGE_WIDTH,
            ConversionError::MaxPageHeight => ERROR_MAX_PAGE_HEIGHT,
            ConversionError::PageCountMismatch => ERROR_PAGE_COUNT_MISMATCH,
            ConversionError::UnexpectedConversion { .. } | ConversionError::UnknownErrorCode(_) => {
                ERROR_UNEXPECTED
            }
            // The base ConversionException itself.
            ConversionError::Unspecified => ERROR_SHIFT,
        }
    }

    /// Returns the conversion error corresponding to an exit code, if any.
    ///
    /// Corresponds to the loop inside `exception_from_error_code`, returning
    /// `None` when the code is unknown.
    pub fn from_error_code(error_code: u32) -> Option<ConversionError> {
        match error_code {
            ERROR_QUBES_QREXEC_FAILED => Some(ConversionError::QubesQrexecFailed),
            ERROR_DOC_FORMAT_UNSUPPORTED => Some(ConversionError::DocFormatUnsupported),
            ERROR_DOC_FORMAT_UNSUPPORTED_HWP_QUBES => {
                Some(ConversionError::DocFormatUnsupportedHwpQubes)
            }
            ERROR_LIBREOFFICE_FAILURE => Some(ConversionError::LibreofficeFailure),
            ERROR_DOC_CORRUPTED => Some(ConversionError::DocCorruptedException),
            ERROR_PAGES => Some(ConversionError::Pages),
            ERROR_NO_PAGE_COUNT => Some(ConversionError::NoPageCount),
            ERROR_MAX_PAGES => Some(ConversionError::MaxPages),
            ERROR_MAX_PAGE_WIDTH => Some(ConversionError::MaxPageWidth),
            ERROR_MAX_PAGE_HEIGHT => Some(ConversionError::MaxPageHeight),
            ERROR_PAGE_COUNT_MISMATCH => Some(ConversionError::PageCountMismatch),
            ERROR_SHIFT => Some(ConversionError::Unspecified),
            ERROR_UNEXPECTED => Some(ConversionError::unexpected_conversion(
                "Some unexpected error occurred while converting the document",
            )),
            _ => None,
        }
    }
}

impl std::fmt::Display for ConversionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConversionError::Unspecified => write!(f, "Unspecified error"),
            ConversionError::QubesQrexecFailed => write!(
                f,
                "Could not start a disposable qube for the file conversion. \
                 More information should have shown up on the top-right corner of your screen."
            ),
            ConversionError::DocFormatUnsupported => {
                write!(f, "The document format is not supported")
            }
            ConversionError::DocFormatUnsupportedHwpQubes => {
                write!(f, "HWP / HWPX formats are not supported in Qubes")
            }
            ConversionError::LibreofficeFailure => {
                write!(f, "Conversion to PDF with LibreOffice failed")
            }
            ConversionError::DocCorruptedException => write!(
                f,
                "The document appears to be corrupted and could not be opened"
            ),
            ConversionError::Pages => write!(f, "Unspecified error"),
            ConversionError::NoPageCount => {
                write!(f, "Number of pages could not be extracted from PDF")
            }
            ConversionError::MaxPages => write!(f, "Number of pages exceeds maximum ({MAX_PAGES})"),
            ConversionError::MaxPageWidth => write!(f, "A page exceeded the maximum width."),
            ConversionError::MaxPageHeight => write!(f, "A page exceeded the maximum height."),
            ConversionError::PageCountMismatch => write!(
                f,
                "The final document does not have the same page count as the original one"
            ),
            ConversionError::UnexpectedConversion { message } => write!(f, "{message}"),
            ConversionError::UnknownErrorCode(code) => {
                write!(f, "Unknown error code '{code}'")
            }
        }
    }
}

impl std::error::Error for ConversionError {}

/// Returns the conversion exception corresponding to an error code.
///
/// Unknown codes produce an `UnexpectedConversionError` with the original
/// "Unknown error code" message.
pub fn exception_from_error_code(error_code: u32) -> ConversionError {
    ConversionError::from_error_code(error_code)
        .unwrap_or(ConversionError::UnknownErrorCode(error_code))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constants_match_python_originals() {
        assert_eq!(ERROR_SHIFT, 128);
        assert_eq!(MAX_PAGES, 10_000);
        assert_eq!(MAX_PAGE_WIDTH, 10_000);
        assert_eq!(MAX_PAGE_HEIGHT, 10_000);
        assert_eq!(DEFAULT_DPI, 150);
        assert_eq!(INT_BYTES, 2);
    }

    #[test]
    fn error_codes_match_python_class_attributes() {
        assert_eq!(ConversionError::Unspecified.error_code(), 128);
        assert_eq!(ConversionError::QubesQrexecFailed.error_code(), 126);
        assert_eq!(ConversionError::DocFormatUnsupported.error_code(), 138);
        assert_eq!(
            ConversionError::DocFormatUnsupportedHwpQubes.error_code(),
            144
        );
        assert_eq!(ConversionError::LibreofficeFailure.error_code(), 148);
        assert_eq!(ConversionError::DocCorruptedException.error_code(), 158);
        assert_eq!(ConversionError::Pages.error_code(), 168);
        assert_eq!(ConversionError::NoPageCount.error_code(), 169);
        assert_eq!(ConversionError::MaxPages.error_code(), 170);
        assert_eq!(ConversionError::MaxPageWidth.error_code(), 172);
        assert_eq!(ConversionError::MaxPageHeight.error_code(), 173);
        assert_eq!(ConversionError::PageCountMismatch.error_code(), 174);
        assert_eq!(
            ConversionError::unexpected_conversion("x").error_code(),
            228
        );
    }

    #[test]
    fn every_known_error_code_maps_to_a_class() {
        for error_code in [
            126, 128, 138, 144, 148, 158, 168, 169, 170, 172, 173, 174, 228,
        ] {
            assert!(
                ConversionError::from_error_code(error_code).is_some(),
                "error code {error_code} should map to a conversion error"
            );
        }
        assert!(ConversionError::from_error_code(0).is_none());
    }

    #[test]
    fn exception_from_error_code_returns_known_class() {
        assert_eq!(
            exception_from_error_code(126),
            ConversionError::QubesQrexecFailed
        );
        assert_eq!(exception_from_error_code(170), ConversionError::MaxPages);
        assert_eq!(
            exception_from_error_code(228),
            ConversionError::UnexpectedConversion {
                message: "Some unexpected error occurred while converting the document".to_string()
            }
        );
    }

    #[test]
    fn exception_from_unknown_error_code_reports_it() {
        let error = exception_from_error_code(999);
        assert_eq!(error, ConversionError::UnknownErrorCode(999));
        assert_eq!(error.to_string(), "Unknown error code '999'");
    }

    #[test]
    fn max_pages_message_embeds_the_limit() {
        let error = ConversionError::MaxPages;
        assert_eq!(error.to_string(), "Number of pages exceeds maximum (10000)");
    }

    #[test]
    fn converter_proc_error_has_exact_message() {
        assert_eq!(
            ConverterProcError.to_string(),
            "The process spawned for the conversion has exited early"
        );
    }
}
