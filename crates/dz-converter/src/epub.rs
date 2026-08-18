//! EPUB ebook conversion.
//!
//! Corresponds to `dangerzone/conversion/epub.py`. EPUB files cannot be
//! rasterized directly, so they are first exported to PDF with Calibre's
//! `ebook-convert` and then rasterized with [`crate::pdf`], mirroring the
//! office pipeline. The input is always staged under a private temporary
//! directory so that no untrusted content is written to a shared location.

use crate::errors::ConversionError;
use crate::pdf::{rasterize_pdf, RasterPage};

/// The `ebook-convert` binary, overridable for testing.
const EBOOK_CONVERT_ENV: &str = "DANGERZONE_EBOOK_CONVERT";

/// The path to the `ebook-convert` binary, honouring the
/// `DANGERZONE_EBOOK_CONVERT` override used by the tests.
fn ebook_convert_binary() -> String {
    std::env::var(EBOOK_CONVERT_ENV).unwrap_or_else(|_| "ebook-convert".to_string())
}

/// Converts an EPUB ebook to page pixel buffers.
///
/// The bytes are written to a temporary directory, converted to PDF with
/// Calibre's `ebook-convert`, and the resulting PDF is rasterized.
///
/// # Errors
///
/// Returns [`ConversionError::LibreofficeFailure`] when Calibre fails or does
/// not produce the expected PDF, and [`ConversionError`] errors from the PDF
/// rasterization step.
pub fn epub_to_pixels(bytes: &[u8]) -> Result<Vec<RasterPage>, ConversionError> {
    let dir = tempfile::tempdir()
        .map_err(|error| ConversionError::unexpected_conversion(format!("temp dir: {error}")))?;

    let input = dir.path().join("input.epub");
    std::fs::write(&input, bytes).map_err(|error| {
        ConversionError::unexpected_conversion(format!("staging input: {error}"))
    })?;

    let output = dir.path().join("input.pdf");
    let status = std::process::Command::new(ebook_convert_binary())
        .arg(&input)
        .arg(&output)
        .status()
        .map_err(|error| {
            ConversionError::unexpected_conversion(format!("running ebook-convert: {error}"))
        })?;
    if !status.success() {
        return Err(ConversionError::LibreofficeFailure);
    }
    if !output.exists() {
        return Err(ConversionError::LibreofficeFailure);
    }

    let pdf_bytes = std::fs::read(&output).map_err(|error| {
        ConversionError::unexpected_conversion(format!("reading converted PDF: {error}"))
    })?;

    rasterize_pdf(pdf_bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The epub pipeline shells out to `ebook-convert`, then rasterizes the
    /// intermediate PDF. This mirrors the office tests: the rasterizer needs
    /// the pinned PDFium library, which is not present on every development
    /// machine, so the test quietly passes when it cannot be bound.
    #[test]
    #[cfg(unix)]
    fn epub_to_pixels_shells_out_to_ebook_convert() {
        use std::os::unix::fs::PermissionsExt;

        let Ok(_pdfium) = crate::pdf::open_pdfium().as_ref() else {
            return;
        };

        let dir = tempfile::tempdir().unwrap();
        let fixture = dir.path().join("fixture.pdf");
        let mut doc = dz_output::pdf::PdfDocument::new();
        doc.insert_pdf(dz_output::pdf::render_pdf_page(
            &[0x41u8; 9 * 9 * 3],
            9,
            9,
            150,
        ));
        std::fs::write(&fixture, doc.to_bytes().unwrap()).unwrap();

        let stub = dir.path().join("ebook-convert");
        std::fs::write(
            &stub,
            format!("#!/bin/sh\ncp '{}' \"$2\"\n", fixture.display()),
        )
        .unwrap();
        std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::env::set_var(EBOOK_CONVERT_ENV, &stub);

        let pages = epub_to_pixels(b"PK\x03\x04 not a real epub").unwrap();
        assert_eq!(pages.len(), 1);
        assert_eq!(
            pages[0].rgb.len(),
            (pages[0].width * pages[0].height * 3) as usize
        );
    }
}
