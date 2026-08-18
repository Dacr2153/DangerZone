//! Office document conversion via LibreOffice.
//!
//! Corresponds to `dangerzone/conversion/office.py`. Office documents cannot be
//! rasterized directly, so they are first exported to PDF with a headless
//! LibreOffice invocation and then rasterized with [`crate::pdf`]. The input is
//! always staged under a private temporary directory so that no untrusted
//! content is written to a shared location.
//!
//! HWP (Hangul Word Processor) is converted through the same LibreOffice
//! pipeline. HWPX, the OOXML-like successor, has no LibreOffice import filter,
//! so a failed HWPX conversion is reported as an unsupported format rather than
//! a generic LibreOffice failure.

use crate::errors::ConversionError;
use crate::format_detect::OfficeKind;
use crate::pdf::{rasterize_pdf, RasterPage};

/// The office document kind being converted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OfficeSource {
    /// A classic office document, identified by its [`OfficeKind`].
    Standard(OfficeKind),
    /// A Hangul Word Processor document.
    Hwp,
    /// A Hangul Word Processor XML document.
    Hwpx,
}

impl OfficeSource {
    /// The staged filename extension LibreOffice needs to pick its import
    /// filter.
    fn extension(self) -> &'static str {
        match self {
            OfficeSource::Standard(kind) => match kind {
                OfficeKind::Doc => "doc",
                OfficeKind::Docx => "docx",
                OfficeKind::Xls => "xls",
                OfficeKind::Xlsx => "xlsx",
                OfficeKind::Ppt => "ppt",
                OfficeKind::Pptx => "pptx",
                OfficeKind::Odt => "odt",
                OfficeKind::Ods => "ods",
                OfficeKind::Odp => "odp",
            },
            OfficeSource::Hwp => "hwp",
            OfficeSource::Hwpx => "hwpx",
        }
    }
}

/// Converts an office document to page pixel buffers.
///
/// The bytes are written to a temporary directory, converted to PDF with
/// LibreOffice, and the resulting PDF is rasterized.
///
/// # Errors
///
/// Returns [`ConversionError::LibreofficeFailure`] when LibreOffice fails or
/// does not produce the expected PDF (or [`ConversionError::DocFormatUnsupported`]
/// for HWPX, which LibreOffice cannot import), and [`ConversionError`] errors
/// from the PDF rasterization step.
pub fn office_to_pixels(
    bytes: &[u8],
    source: OfficeSource,
) -> Result<Vec<RasterPage>, ConversionError> {
    let dir = tempfile::tempdir()
        .map_err(|error| ConversionError::unexpected_conversion(format!("temp dir: {error}")))?;

    let input = dir
        .path()
        .join(format!("input_file.{}", source.extension()));
    std::fs::write(&input, bytes).map_err(|error| {
        ConversionError::unexpected_conversion(format!("staging input: {error}"))
    })?;

    let status = std::process::Command::new("libreoffice")
        .args([
            "--headless",
            "--safe-mode",
            "--convert-to",
            "pdf",
            "--outdir",
        ])
        .arg(dir.path())
        // A per-invocation profile avoids clobbering the default LibreOffice
        // user profile (which is not writable inside the sandbox).
        .arg(format!(
            "-env:UserInstallation=file://{}",
            dir.path().join("lo_profile").display()
        ))
        .arg(&input)
        .status()
        .map_err(|error| {
            ConversionError::unexpected_conversion(format!("running libreoffice: {error}"))
        })?;
    if !status.success() {
        return Err(failure_error(source));
    }

    let pdf_path = dir.path().join("input_file.pdf");
    if !pdf_path.exists() {
        return Err(failure_error(source));
    }
    let pdf_bytes = std::fs::read(&pdf_path).map_err(|error| {
        ConversionError::unexpected_conversion(format!("reading converted PDF: {error}"))
    })?;

    rasterize_pdf(pdf_bytes)
}

/// The error reported when LibreOffice fails to produce the PDF.
///
/// LibreOffice has no HWPX import filter, so HWPX documents are reported as
/// unsupported instead of as a LibreOffice failure.
fn failure_error(source: OfficeSource) -> ConversionError {
    match source {
        OfficeSource::Hwpx => ConversionError::DocFormatUnsupported,
        _ => ConversionError::LibreofficeFailure,
    }
}
