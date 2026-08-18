//! The container-side document-to-pixels pipeline.
//!
//! Corresponds to `dangerzone/conversion/doc_to_pixels.py`. [`convert`] reads
//! the untrusted document, detects its format, rasterizes it into page pixel
//! buffers, and writes them over the conversion wire protocol: a big-endian
//! `u16` page count, then for every page a big-endian `u16` width, a big-endian
//! `u16` height, and `width * height * 3` raw RGB bytes.

use std::io::{Read, Write};

use crate::errors::{ConversionError, MAX_PAGES};
use crate::format_detect::{detect_format, DocumentFormat};
use crate::pdf::RasterPage;

/// Maximum size, in bytes, of the input document the converter will process.
pub const MAX_INPUT_BYTES: usize = 100 * 1024 * 1024;

/// Converts a document into the page buffers of the wire protocol.
///
/// `input` is read up to [`MAX_INPUT_BYTES`]; `output` receives the encoded
/// page buffers, and `progress` is invoked with a human-readable message for
/// every page converted.
///
/// When `ocr_lang` is set, each page is turned into a searchable single-page
/// PDF with Tesseract and sent as `page_count` (`u16`), followed for every page
/// by its length (`u32`) and the PDF page bytes. Otherwise the pages are sent
/// as raw RGB pixel buffers (`u16` width, `u16` height, `width * height * 3`
/// bytes).
///
/// # Errors
///
/// Returns [`ConversionError::DocFormatUnsupported`] for formats outside the
/// MVP scope, [`ConversionError::MaxPages`] when the page count exceeds the
/// protocol limit, and the format-specific [`ConversionError`] values from the
/// rasterizers.
pub fn convert<R: Read, W: Write, F: FnMut(&str)>(
    mut input: R,
    mut output: W,
    ocr_lang: Option<&str>,
    mut progress: F,
) -> Result<(), ConversionError> {
    let mut bytes = Vec::new();
    input
        .by_ref()
        .take((MAX_INPUT_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|error| {
            ConversionError::unexpected_conversion(format!("reading the input document: {error}"))
        })?;
    if bytes.len() > MAX_INPUT_BYTES {
        return Err(ConversionError::unexpected_conversion(
            "the input document exceeds the 100 MB limit",
        ));
    }

    let format = detect_format(&bytes, None);
    let pages = match format {
        DocumentFormat::Pdf => crate::pdf::rasterize_pdf(bytes),
        DocumentFormat::Image(_) => {
            let (rgb, width, height) = crate::image::image_to_rgb(&bytes)?;
            Ok(vec![RasterPage { rgb, width, height }])
        }
        DocumentFormat::Office(kind) => {
            crate::office::office_to_pixels(&bytes, crate::office::OfficeSource::Standard(kind))
        }
        DocumentFormat::Epub => crate::epub::epub_to_pixels(&bytes),
        DocumentFormat::Svg => {
            let (rgb, width, height) = crate::svg::svg_to_rgb(&bytes)?;
            Ok(vec![RasterPage { rgb, width, height }])
        }
        DocumentFormat::Hwp => {
            crate::office::office_to_pixels(&bytes, crate::office::OfficeSource::Hwp)
        }
        DocumentFormat::Hwpx => {
            crate::office::office_to_pixels(&bytes, crate::office::OfficeSource::Hwpx)
        }
        DocumentFormat::Unsupported => return Err(ConversionError::DocFormatUnsupported),
    }?;

    if pages.is_empty() || pages.len() > MAX_PAGES as usize {
        return Err(ConversionError::MaxPages);
    }
    if pages.len() > u16::MAX as usize {
        return Err(ConversionError::MaxPages);
    }

    output
        .write_all(&(pages.len() as u16).to_be_bytes())
        .map_err(protocol_write_error)?;
    let page_count = pages.len();
    for (index, page) in pages.iter().enumerate() {
        if let Some(ocr_lang) = ocr_lang {
            let page_pdf = crate::pdf::ocr_pdf_page(&page.rgb, page.width, page.height, ocr_lang)?;
            let len = u32::try_from(page_pdf.len()).map_err(|_| ConversionError::MaxPageWidth)?;
            output
                .write_all(&len.to_be_bytes())
                .map_err(protocol_write_error)?;
            output.write_all(&page_pdf).map_err(protocol_write_error)?;
            progress(&format!(
                "Converted page {}/{page_count} to searchable PDF",
                index + 1
            ));
        } else {
            let width = u16::try_from(page.width).map_err(|_| ConversionError::MaxPageWidth)?;
            let height = u16::try_from(page.height).map_err(|_| ConversionError::MaxPageHeight)?;
            output
                .write_all(&width.to_be_bytes())
                .map_err(protocol_write_error)?;
            output
                .write_all(&height.to_be_bytes())
                .map_err(protocol_write_error)?;
            output.write_all(&page.rgb).map_err(protocol_write_error)?;
            progress(&format!("Converted page {}/{page_count} to PDF", index + 1));
        }
    }
    Ok(())
}

/// Maps a write error on the conversion output into a conversion error.
fn protocol_write_error(error: std::io::Error) -> ConversionError {
    ConversionError::unexpected_conversion(format!("writing the conversion output: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::errors::INT_BYTES;
    use std::io::{Cursor, Read};

    fn tiny_png() -> Vec<u8> {
        let mut img = image::RgbImage::new(2, 2);
        for pixel in img.pixels_mut() {
            *pixel = image::Rgb([255, 0, 0]);
        }
        let mut out = Vec::new();
        img.write_to(&mut Cursor::new(&mut out), image::ImageFormat::Png)
            .unwrap();
        out
    }

    /// Reads a big-endian u16 from the protocol stream, like `base.rs` does.
    fn read_int(reader: &mut impl Read) -> u16 {
        let mut buf = [0u8; INT_BYTES];
        reader.read_exact(&mut buf).unwrap();
        u16::from_be_bytes(buf)
    }

    #[test]
    fn convert_writes_a_single_page_for_a_png() {
        let input = tiny_png();
        let mut output = Vec::new();
        let mut progress = Vec::new();
        convert(input.as_slice(), &mut output, None, |message| {
            progress.push(message.to_string())
        })
        .unwrap();

        let mut cursor = Cursor::new(output);
        assert_eq!(read_int(&mut cursor), 1);
        assert_eq!(read_int(&mut cursor), 2);
        assert_eq!(read_int(&mut cursor), 2);
        let mut rgb = vec![0u8; 2 * 2 * 3];
        cursor.read_exact(&mut rgb).unwrap();
        assert!(rgb.chunks_exact(3).all(|pixel| pixel == [255, 0, 0]));
        assert!(cursor.get_ref().len() as u64 == cursor.position());
        assert_eq!(progress.len(), 1);
    }

    #[test]
    fn convert_rejects_unsupported_formats() {
        let mut output = Vec::new();
        let err =
            convert(b"not a real document".as_slice(), &mut output, None, |_| {}).unwrap_err();
        assert_eq!(err, ConversionError::DocFormatUnsupported);
    }

    #[test]
    fn convert_rejects_oversized_input() {
        let huge = vec![0u8; MAX_INPUT_BYTES + 1];
        let err = convert(huge.as_slice(), Vec::new(), None, |_| {}).unwrap_err();
        assert_eq!(
            err,
            ConversionError::unexpected_conversion("the input document exceeds the 100 MB limit")
        );
    }

    /// An SVG is rendered in-process with `resvg`, so the whole pipeline runs
    /// without any external binary.
    #[test]
    fn convert_writes_a_single_page_for_an_svg() {
        let input = b"<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"2\" height=\"2\"><rect width=\"2\" height=\"2\" fill=\"red\"/></svg>";
        let mut output = Vec::new();
        convert(input.as_slice(), &mut output, None, |_| {}).unwrap();

        let mut cursor = Cursor::new(output);
        assert_eq!(read_int(&mut cursor), 1);
        assert_eq!(read_int(&mut cursor), 2);
        assert_eq!(read_int(&mut cursor), 2);
        let mut rgb = vec![0u8; 2 * 2 * 3];
        cursor.read_exact(&mut rgb).unwrap();
        assert!(rgb.chunks_exact(3).all(|pixel| pixel == [255, 0, 0]));
    }

    /// With an OCR language set, every page must be sent as a length-prefixed
    /// searchable PDF page instead of a raw pixel buffer.
    #[test]
    #[cfg(unix)]
    fn convert_writes_length_prefixed_ocr_pages() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let stub = dir.path().join("tesseract");
        std::fs::write(
            &stub,
            "#!/bin/sh\nprintf '%%PDF-1.4\\nOCR-PAGE' > \"$2.pdf\"\n",
        )
        .unwrap();
        std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();
        // pdf::ocr_pdf_page honours this override; the value races with the
        // pdf.rs OCR tests but every stub emits the same marker bytes.
        std::env::set_var("DANGERZONE_TESSERACT", &stub);

        let mut output = Vec::new();
        let mut progress = Vec::new();
        convert(tiny_png().as_slice(), &mut output, Some("eng"), |message| {
            progress.push(message.to_string())
        })
        .unwrap();

        let mut cursor = Cursor::new(output);
        assert_eq!(read_int(&mut cursor), 1);
        let len = {
            let mut buf = [0u8; 4];
            cursor.read_exact(&mut buf).unwrap();
            u32::from_be_bytes(buf)
        };
        let mut page = vec![0u8; len as usize];
        cursor.read_exact(&mut page).unwrap();
        assert!(page.starts_with(b"%PDF-1.4\nOCR-PAGE"));
        assert!(cursor.get_ref().len() as u64 == cursor.position());
        assert_eq!(progress, vec!["Converted page 1/1 to searchable PDF"]);
    }
}
