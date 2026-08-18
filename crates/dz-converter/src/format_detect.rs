//! Document format detection.
//!
//! Corresponds to `dangerzone/conversion/format_detect.py`. The input document
//! is classified from its magic bytes (via the `infer` crate), with a filename
//! extension fallback for the office subtypes that magic bytes alone cannot
//! always tell apart. The zip-based formats (epub, HWPX) and the HWP magic
//! signature are detected from their container bytes, so classification works
//! even when the converter only sees the raw stream. Anything outside the
//! supported scope is reported as [`DocumentFormat::Unsupported`].

/// Bitmap image formats supported by the MVP rasterizer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageFormat {
    /// Portable Network Graphics.
    Png,
    /// Joint Photographic Experts Group.
    Jpeg,
    /// Graphics Interchange Format.
    Gif,
    /// Windows Bitmap.
    Bmp,
    /// Tagged Image File Format.
    Tiff,
}

/// Office document kinds, converted to PDF via LibreOffice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OfficeKind {
    /// Microsoft Word 97-2003.
    Doc,
    /// Microsoft Word Open XML.
    Docx,
    /// Microsoft Excel 97-2003.
    Xls,
    /// Microsoft Excel Open XML.
    Xlsx,
    /// Microsoft PowerPoint 97-2003.
    Ppt,
    /// Microsoft PowerPoint Open XML.
    Pptx,
    /// OpenDocument Text.
    Odt,
    /// OpenDocument Spreadsheet.
    Ods,
    /// OpenDocument Presentation.
    Odp,
}

/// The recognized document formats of the MVP.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocumentFormat {
    /// A PDF document, rasterized directly with PDFium.
    Pdf,
    /// A bitmap image, decoded with the `image` crate.
    Image(ImageFormat),
    /// An office document, converted with LibreOffice.
    Office(OfficeKind),
    /// An EPUB ebook, converted with Calibre's `ebook-convert`.
    Epub,
    /// A Scalable Vector Graphics document, rendered with `resvg`.
    Svg,
    /// A Hangul Word Processor document, converted with LibreOffice.
    Hwp,
    /// A Hangul Word Processor XML document, an OPC/zip container.
    Hwpx,
    /// A format outside the supported scope, or an unrecognized stream.
    Unsupported,
}

/// The local file header magic that opens every ZIP container, shared by the
/// OOXML/ODF office formats, EPUB, and HWPX.
const ZIP_MAGIC: &[u8] = b"PK\x03\x04";
/// The magic signature of the legacy binary Hangul Word Processor format.
const HWP_MAGIC: &[u8] = b"HWP Document File";

/// Classifies a document from its magic bytes, with an extension fallback.
///
/// HWPX is a plain ZIP container that cannot be told apart from a generic ZIP
/// by its header alone, so its contents are inspected for the `.hpfx` part
/// that only a HWPX document contains. The extension is used as a
/// tie-breaker for the office subtypes and as a fallback for streams whose
/// magic bytes are not recognized.
pub fn detect_format(bytes: &[u8], extension: Option<&str>) -> DocumentFormat {
    if let Some(kind) = infer::get(bytes) {
        match kind.extension() {
            "pdf" => return DocumentFormat::Pdf,
            "png" => return DocumentFormat::Image(ImageFormat::Png),
            "jpg" => return DocumentFormat::Image(ImageFormat::Jpeg),
            "gif" => return DocumentFormat::Image(ImageFormat::Gif),
            "bmp" => return DocumentFormat::Image(ImageFormat::Bmp),
            "tif" => return DocumentFormat::Image(ImageFormat::Tiff),
            "doc" => return DocumentFormat::Office(OfficeKind::Doc),
            "docx" => return DocumentFormat::Office(OfficeKind::Docx),
            "xls" => return DocumentFormat::Office(OfficeKind::Xls),
            "xlsx" => return DocumentFormat::Office(OfficeKind::Xlsx),
            "ppt" => return DocumentFormat::Office(OfficeKind::Ppt),
            "pptx" => return DocumentFormat::Office(OfficeKind::Pptx),
            "odt" => return DocumentFormat::Office(OfficeKind::Odt),
            "ods" => return DocumentFormat::Office(OfficeKind::Ods),
            "odp" => return DocumentFormat::Office(OfficeKind::Odp),
            "epub" => return DocumentFormat::Epub,
            "svg" => return DocumentFormat::Svg,
            // Recognized by `infer` but not supported.
            "swf" => return DocumentFormat::Unsupported,
            _ => {}
        }
    }

    // The legacy HWP magic, recognized independently of the extension.
    if bytes.starts_with(HWP_MAGIC) {
        return DocumentFormat::Hwp;
    }

    // SVG is an XML dialect. It may carry an XML declaration, so the first
    // element is located before checking for the root `<svg>` tag.
    if is_svg(bytes) {
        return DocumentFormat::Svg;
    }

    // HWPX and EPUB are ZIP containers. EPUB is usually caught by `infer`;
    // HWPX needs its contents inspected because nothing distinguishes its
    // header from a generic ZIP archive.
    if bytes.starts_with(ZIP_MAGIC) && (extension == Some("hwpx") || zip_contains_hwpx(bytes)) {
        return DocumentFormat::Hwpx;
    }

    match extension {
        Some("doc") => DocumentFormat::Office(OfficeKind::Doc),
        Some("docx") => DocumentFormat::Office(OfficeKind::Docx),
        Some("xls") => DocumentFormat::Office(OfficeKind::Xls),
        Some("xlsx") => DocumentFormat::Office(OfficeKind::Xlsx),
        Some("ppt") => DocumentFormat::Office(OfficeKind::Ppt),
        Some("pptx") => DocumentFormat::Office(OfficeKind::Pptx),
        Some("odt") => DocumentFormat::Office(OfficeKind::Odt),
        Some("ods") => DocumentFormat::Office(OfficeKind::Ods),
        Some("odp") => DocumentFormat::Office(OfficeKind::Odp),
        Some("pdf") => DocumentFormat::Pdf,
        Some("epub") => DocumentFormat::Epub,
        Some("svg") => DocumentFormat::Svg,
        Some("hwp") => DocumentFormat::Hwp,
        Some("hwpx") => DocumentFormat::Hwpx,
        _ => DocumentFormat::Unsupported,
    }
}

/// Returns whether the bytes look like an SVG document, i.e. an XML stream
/// whose root element is `<svg`.
fn is_svg(bytes: &[u8]) -> bool {
    let head = &bytes[..bytes.len().min(1024)];
    let text = String::from_utf8_lossy(head);
    let mut rest = text.trim_start();
    if let Some(after_decl) = rest.strip_prefix("<?xml") {
        rest = after_decl;
        if let Some(end) = rest.find("?>") {
            rest = rest[end + 2..].trim_start();
        }
    }
    while let Some(after_comment) = rest.strip_prefix("<!--") {
        rest = after_comment;
        if let Some(end) = rest.find("-->") {
            rest = rest[end + 3..].trim_start();
        }
    }
    rest.get(..4)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("<svg"))
}

/// Returns whether the ZIP archive contains a `.hpfx` part, which is the
/// defining member of a Hangul Word Processor XML document.
fn zip_contains_hwpx(bytes: &[u8]) -> bool {
    let Ok(archive) = zip::ZipArchive::new(std::io::Cursor::new(bytes)) else {
        return false;
    };
    let has_hwpx = archive
        .file_names()
        .any(|name| name.to_ascii_lowercase().ends_with(".hpfx"));
    has_hwpx
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn detects_pdf_from_magic_bytes() {
        assert_eq!(detect_format(b"%PDF-1.7\n...", None), DocumentFormat::Pdf);
    }

    #[test]
    fn detects_png_from_magic_bytes() {
        let magic = b"\x89PNG\r\n\x1a\n\x00\x00\x00\x0dIHDR";
        assert_eq!(
            detect_format(magic, None),
            DocumentFormat::Image(ImageFormat::Png)
        );
    }

    #[test]
    fn detects_jpeg_gif_tiff_and_bmp_from_magic_bytes() {
        assert_eq!(
            detect_format(&[0xff, 0xd8, 0xff, 0xe0], None),
            DocumentFormat::Image(ImageFormat::Jpeg)
        );
        assert_eq!(
            detect_format(b"GIF89a....", None),
            DocumentFormat::Image(ImageFormat::Gif)
        );
        assert_eq!(
            detect_format(
                &[0x49, 0x49, 0x2a, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
                None
            ),
            DocumentFormat::Image(ImageFormat::Tiff)
        );
        assert_eq!(
            detect_format(b"BM....", None),
            DocumentFormat::Image(ImageFormat::Bmp)
        );
    }

    #[test]
    fn office_format_falls_back_to_extension() {
        let bytes = b"\x00\x00\x00\x00 this is not a real document";
        assert_eq!(
            detect_format(bytes, Some("docx")),
            DocumentFormat::Office(OfficeKind::Docx)
        );
        assert_eq!(
            detect_format(bytes, Some("ods")),
            DocumentFormat::Office(OfficeKind::Ods)
        );
    }

    #[test]
    fn unknown_input_is_unsupported() {
        assert_eq!(
            detect_format(b"not a document", None),
            DocumentFormat::Unsupported
        );
    }

    #[test]
    fn detects_epub_from_magic_bytes() {
        // A real EPUB carries its media type in the `mimetype` part, which
        // `infer` reads to classify the ZIP container.
        let mut epub = Vec::new();
        {
            let mut writer = zip::ZipWriter::new(std::io::Cursor::new(&mut epub));
            writer
                .start_file("mimetype", zip::write::SimpleFileOptions::default())
                .unwrap();
            writer.write_all(b"application/epub+zip").unwrap();
            writer.finish().unwrap();
        }
        assert_eq!(detect_format(&epub, None), DocumentFormat::Epub);
        // The extension is the tie-breaker when the magic bytes are missing.
        assert_eq!(
            detect_format(b"PK\x03\x04...", Some("epub")),
            DocumentFormat::Epub
        );
    }

    #[test]
    fn detects_svg_from_xml_bytes() {
        let svg = b"<?xml version=\"1.0\"?>\n<svg xmlns=\"http://www.w3.org/2000/svg\">";
        assert_eq!(detect_format(svg, None), DocumentFormat::Svg);
        assert_eq!(
            detect_format(
                b"<!-- leading --><svg xmlns=\"http://www.w3.org/2000/svg\">",
                None
            ),
            DocumentFormat::Svg
        );
        assert_eq!(
            detect_format(b"not svg <svg>", None),
            DocumentFormat::Unsupported
        );
    }

    #[test]
    fn detects_hwp_from_magic_bytes() {
        let magic = b"HWP Document File, 5.0.0.0, KTF, 2007-01-01";
        assert_eq!(detect_format(magic, None), DocumentFormat::Hwp);
        assert_eq!(
            detect_format(b"random bytes", Some("hwp")),
            DocumentFormat::Hwp
        );
    }

    /// Builds a minimal ZIP archive in memory, mirroring the member layout of
    /// a HWPX document.
    fn hwpx_archive(parts: &[&str]) -> Vec<u8> {
        let mut out = Vec::new();
        {
            let mut writer = zip::ZipWriter::new(std::io::Cursor::new(&mut out));
            for part in parts {
                writer
                    .start_file(*part, zip::write::SimpleFileOptions::default())
                    .unwrap();
                writer.write_all(b"<part/>").unwrap();
            }
            writer.finish().unwrap();
        }
        out
    }

    #[test]
    fn detects_hwpx_by_inspecting_the_zip_contents() {
        let hwpx = hwpx_archive(&["[Content_Types].xml", "Contents/contents.hpfx"]);
        assert_eq!(detect_format(&hwpx, None), DocumentFormat::Hwpx);
        assert_eq!(detect_format(&hwpx, Some("hwpx")), DocumentFormat::Hwpx);
    }

    #[test]
    fn a_generic_zip_is_not_hwpx() {
        let zip = hwpx_archive(&["README.txt", "data.bin"]);
        assert_eq!(detect_format(&zip, None), DocumentFormat::Unsupported);
        assert_eq!(detect_format(&zip, Some("hwpx")), DocumentFormat::Hwpx);
    }
}
