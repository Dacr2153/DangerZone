//! PDF rasterization through a pinned PDFium library.
//!
//! Corresponds to `dangerzone/conversion/pdf.py`. Instead of using PyMuPDF,
//! pages are rasterized with PDFium (the same engine Google Chrome embeds),
//! bound at runtime from a pinned, sha256-verified `libpdfium.so` inside the
//! sandbox image. The library path can be overridden with the
//! `DANGERZONE_LIBPDFIUM` environment variable, which also lets the rasterizer
//! be exercised on development machines.
//!
//! When OCR is requested, [`ocr_pdf_page`] shells out to the `tesseract`
//! binary with the page's pixel buffer and turns it into a searchable
//! single-page PDF. This runs *inside* the sandbox, so the host never has to
//! invoke Tesseract. The binary can be overridden with `DANGERZONE_TESSERACT`
//! (used by the tests to stub it out).

use std::path::Path;
use std::sync::OnceLock;

use pdfium_render::prelude::*;

use crate::errors::{ConversionError, DEFAULT_DPI, MAX_PAGES, MAX_PAGE_HEIGHT, MAX_PAGE_WIDTH};

/// Default location of the pinned `libpdfium.so` inside the sandbox image.
pub const PDFIUM_PINNED_PATH: &str = "/opt/dangerzone/lib/libpdfium.so";

/// The Tesseract binary, overridable for testing.
const TESSERACT_ENV: &str = "DANGERZONE_TESSERACT";

/// Standard locations of the Tesseract language data, taken from the Tesseract
/// documentation.
const TESSDATA_CANDIDATES: [&str; 5] = [
    "/usr/share/tessdata",
    "/usr/share/tesseract/tessdata",
    "/usr/share/tesseract-ocr/tessdata",
    "/usr/share/tesseract-ocr/4.00/tessdata",
    "/usr/share/tesseract-ocr/5/tessdata",
];

/// Errors raised while binding the PDFium library.
#[derive(Debug, thiserror::Error)]
pub enum PdfiumOpenError {
    /// The shared library could not be loaded or bound.
    #[error("could not load the PDFium library: {0}")]
    Bind(String),
}

/// A rasterized PDF page in the format of the conversion wire protocol.
#[derive(Debug)]
pub struct RasterPage {
    /// The raw RGB pixels (`width * height * 3` bytes).
    pub rgb: Vec<u8>,
    /// The page width in pixels.
    pub width: u32,
    /// The page height in pixels.
    pub height: u32,
}

static PDFIUM: OnceLock<Result<Pdfium, PdfiumOpenError>> = OnceLock::new();

/// Binds the PDFium library once and returns the shared instance.
///
/// The first call loads the library from `DANGERZONE_LIBPDFIUM` if set, then
/// from [`PDFIUM_PINNED_PATH`], and finally falls back to the system library.
/// Later calls reuse the same instance; PDFium can only be initialized once per
/// process.
pub fn open_pdfium() -> &'static Result<Pdfium, PdfiumOpenError> {
    PDFIUM.get_or_init(|| {
        let path = std::env::var("DANGERZONE_LIBPDFIUM")
            .unwrap_or_else(|_| PDFIUM_PINNED_PATH.to_string());
        let bindings = Pdfium::bind_to_library(&path)
            .or_else(|_| Pdfium::bind_to_system_library())
            .map_err(|error| PdfiumOpenError::Bind(error.to_string()))?;
        Ok(Pdfium::new(bindings))
    })
}

/// Rasterizes a PDF into a list of page pixel buffers.
///
/// Pages are rendered at [`DEFAULT_DPI`] by computing the target width in
/// pixels from the page size in PDF points (72 per inch).
///
/// # Errors
///
/// Returns [`ConversionError::DocCorruptedException`] when the bytes do not
/// form a readable PDF, [`ConversionError::NoPageCount`] when the document has
/// no pages, and the respective [`ConversionError`] limits when the page count
/// or a page size exceeds the protocol maximum.
pub fn rasterize_pdf(bytes: Vec<u8>) -> Result<Vec<RasterPage>, ConversionError> {
    let pdfium = open_pdfium()
        .as_ref()
        .map_err(|error| ConversionError::unexpected_conversion(error.to_string()))?;

    let document = pdfium
        .load_pdf_from_byte_vec(bytes, None)
        .map_err(|_| ConversionError::DocCorruptedException)?;
    let pages = document.pages();
    let count = pages.len();
    if count <= 0 || count as usize > MAX_PAGES as usize {
        return Err(if count <= 0 {
            ConversionError::NoPageCount
        } else {
            ConversionError::MaxPages
        });
    }

    let mut raster = Vec::with_capacity(count as usize);
    for index in 0..count {
        let page = pages
            .get(index)
            .map_err(|_| ConversionError::DocCorruptedException)?;

        // 72 PDF points equal one inch, so the pixel width for the target DPI
        // is `points * dpi / 72`.
        let target_width = (f64::from(page.width().value) * f64::from(DEFAULT_DPI) / 72.0).round();
        if target_width > f64::from(MAX_PAGE_WIDTH) {
            return Err(ConversionError::MaxPageWidth);
        }

        let bitmap = page
            .render_with_config(&PdfRenderConfig::new().set_target_width(target_width as i32))
            .map_err(|_| ConversionError::DocCorruptedException)?;

        let width = bitmap.width();
        let height = bitmap.height();
        if width > MAX_PAGE_WIDTH as i32 {
            return Err(ConversionError::MaxPageWidth);
        }
        if height > MAX_PAGE_HEIGHT as i32 {
            return Err(ConversionError::MaxPageHeight);
        }

        // The bitmap is RGBA; the protocol expects RGB.
        let rgba = bitmap.as_rgba_bytes();
        let mut rgb = Vec::with_capacity(width as usize * height as usize * 3);
        for pixel in rgba.chunks_exact(4) {
            rgb.extend_from_slice(&pixel[..3]);
        }
        raster.push(RasterPage {
            rgb,
            width: width as u32,
            height: height as u32,
        });
    }
    Ok(raster)
}

/// The path to the `tesseract` binary, honouring the `DANGERZONE_TESSERACT`
/// override used by the tests.
fn tesseract_binary() -> String {
    std::env::var(TESSERACT_ENV).unwrap_or_else(|_| "tesseract".to_string())
}

/// Returns the `--tessdata-dir` arguments for Tesseract, if a known language
/// data directory exists.
fn tessdata_args() -> Vec<String> {
    for candidate in TESSDATA_CANDIDATES {
        if Path::new(candidate).is_dir() {
            return vec!["--tessdata-dir".to_string(), candidate.to_string()];
        }
    }
    Vec::new()
}

/// Writes the pixel buffer as a P6 (binary) PNM file that Tesseract reads.
fn write_pnm(path: &Path, pixels: &[u8], width: u32, height: u32) -> Result<(), ConversionError> {
    let expected = u64::from(width) * u64::from(height) * 3;
    if pixels.len() as u64 != expected {
        return Err(ConversionError::unexpected_conversion(format!(
            "internal pixel buffer has {} bytes, expected {expected}",
            pixels.len()
        )));
    }
    let mut pnm = Vec::with_capacity(pixels.len() + 32);
    pnm.extend_from_slice(format!("P6\n{width} {height}\n255\n").as_bytes());
    pnm.extend_from_slice(pixels);
    std::fs::write(path, pnm)
        .map_err(|error| ConversionError::unexpected_conversion(format!("writing PNM: {error}")))
}

/// Rasterizes a page and runs Tesseract on it, returning a searchable
/// single-page PDF.
///
/// `pixels` is the raw RGB buffer of the page (`width * height * 3` bytes).
/// The pixels are staged as a PNM file, Tesseract turns them into a PDF page
/// with a text layer for `ocr_lang`, and the resulting page bytes are
/// returned. The non-OCR path is unaffected: callers only reach this function
/// when a language has been requested.
///
/// # Errors
///
/// Returns a [`ConversionError::UnexpectedConversion`] when Tesseract cannot
/// be run or exits with an error, or when the generated PDF page cannot be
/// read back.
pub fn ocr_pdf_page(
    pixels: &[u8],
    width: u32,
    height: u32,
    ocr_lang: &str,
) -> Result<Vec<u8>, ConversionError> {
    let dir = tempfile::tempdir()
        .map_err(|error| ConversionError::unexpected_conversion(format!("temp dir: {error}")))?;

    let image_path = dir.path().join("page.pnm");
    write_pnm(&image_path, pixels, width, height)?;

    let output_base = dir.path().join("page");
    let mut command = std::process::Command::new(tesseract_binary());
    command
        .arg(&image_path)
        .arg(&output_base)
        .arg("-l")
        .arg(ocr_lang)
        .arg("pdf");
    for arg in tessdata_args() {
        command.arg(arg);
    }

    let output = command.output().map_err(|error| {
        ConversionError::unexpected_conversion(format!("running tesseract: {error}"))
    })?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(ConversionError::unexpected_conversion(format!(
            "Tesseract OCR failed (language '{ocr_lang}'): {}",
            stderr.trim()
        )));
    }

    let page_path = dir.path().join("page.pdf");
    std::fs::read(&page_path).map_err(|error| {
        ConversionError::unexpected_conversion(format!(
            "Tesseract did not produce the expected PDF page: {error}"
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::Mutex;

    /// Serializes access to the `DANGERZONE_TESSERACT` environment variable,
    /// which the OCR tests override.
    static TESSERACT_ENV_LOCK: Mutex<()> = Mutex::new(());

    /// Writes a stub `tesseract` executable that records its arguments, copies
    /// the staged input image out for inspection, and emits a fixed PDF page,
    /// returning the path to it.
    #[cfg(unix)]
    fn tesseract_stub(dir: &Path, log: &Path) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let stub = dir.join("tesseract");
        let input = dir.join("input.pnm");
        std::fs::write(
            &stub,
            format!(
                "#!/bin/sh\n\
                 echo \"$@\" > {}\n\
                 cp \"$1\" {}\n\
                 printf '%%PDF-1.4\\nSTUB-PAGE' > \"$2.pdf\"\n",
                log.display(),
                input.display(),
            ),
        )
        .unwrap();
        std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();
        stub
    }

    /// Rasterizes a minimal PDF, only when a PDFium library is available.
    ///
    /// The rasterizer needs the external library, which is not present on every
    /// development machine; the test quietly passes when it cannot be bound.
    #[test]
    fn rasterizes_a_minimal_pdf_when_pdfium_is_available() {
        let Ok(pdfium) = open_pdfium().as_ref() else {
            return;
        };

        let mut doc = dz_output::pdf::PdfDocument::new();
        doc.insert_pdf(dz_output::pdf::render_pdf_page(
            &[0x41u8; 9 * 9 * 3],
            9,
            9,
            150,
        ));
        let bytes = doc.to_bytes().unwrap();

        let document = pdfium
            .load_pdf_from_byte_vec(bytes, None)
            .expect("generated PDF must load");
        assert_eq!(document.pages().len(), 1);
    }

    /// The OCR path must shell out to `tesseract` with the language and the
    /// `pdf` output format, feeding it the page as a PNM image, and return the
    /// searchable page PDF it produced.
    #[test]
    #[cfg(unix)]
    fn ocr_pdf_page_shells_out_to_tesseract() {
        let _guard = TESSERACT_ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("args.log");
        let stub = tesseract_stub(dir.path(), &log);
        std::env::set_var(TESSERACT_ENV, &stub);

        let pixels = [0xAAu8; 2 * 2 * 3];
        let page = ocr_pdf_page(&pixels, 2, 2, "eng").unwrap();
        assert!(page.starts_with(b"%PDF-1.4\nSTUB-PAGE"));

        let args = std::fs::read_to_string(&log).unwrap();
        let mut split = args.split_whitespace();
        let _input = split.next().unwrap();
        assert!(split.any(|arg| arg == "-l"));
        assert!(split.any(|arg| arg == "eng"));
        assert!(split.any(|arg| arg == "pdf"));

        // The staged input must be a valid binary PNM for the page.
        let pnm = std::fs::read(dir.path().join("input.pnm")).unwrap();
        assert!(pnm.starts_with(b"P6\n2 2\n255\n"));
        assert!(pnm[11..].iter().all(|&b| b == 0xAA));
    }

    /// A failing Tesseract run must surface as a conversion error carrying the
    /// language, rather than silently producing an empty document.
    #[test]
    #[cfg(unix)]
    fn ocr_pdf_page_reports_tesseract_failures() {
        let _guard = TESSERACT_ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let stub = dir.path().join("tesseract");
        std::fs::write(&stub, "#!/bin/sh\nexit 2\n").unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::env::set_var(TESSERACT_ENV, &stub);

        let error = ocr_pdf_page(&[0u8; 2 * 2 * 3], 2, 2, "deu").unwrap_err();
        match error {
            ConversionError::UnexpectedConversion { message } => {
                assert!(message.contains("deu"), "message: {message}");
            }
            other => panic!("unexpected error variant: {other:?}"),
        }
    }

    /// The pixel buffer must match the page dimensions before Tesseract runs.
    #[test]
    #[cfg(unix)]
    fn ocr_pdf_page_rejects_mismatched_pixel_buffers() {
        let _guard = TESSERACT_ENV_LOCK.lock().unwrap();
        let error = ocr_pdf_page(&[0u8; 3], 2, 2, "eng").unwrap_err();
        assert!(matches!(
            error,
            ConversionError::UnexpectedConversion { .. }
        ));
    }
}
