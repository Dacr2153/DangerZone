//! Minimal, self-contained PDF assembly used to turn pixel buffers into a safe
//! PDF.
//!
//! The original Python code uses PyMuPDF (`fitz`) to render RGB pixel buffers
//! into PDF pages and merge them into a single document, then saves it with
//! compression. This module provides the same operations with no dependency on
//! a PDF library: pages are stored as raw RGB images, compressed with the
//! `/FlateDecode` filter, and written to a PDF 1.4 file with a valid classic
//! cross-reference table and a fixed `/Info` dictionary.
//!
//! When OCR is requested the sandbox does not send pixels: it runs Tesseract
//! and sends back searchable single-page PDFs. The host inserts them verbatim
//! ([`PdfDocument::insert_ocr_page`]); the final assembly then merges the
//! object graphs of those pages with `lopdf`, mirroring
//! `fitz.Document.insert_pdf`. The host never invokes Tesseract itself.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::Path;

use lopdf::{Document, Object, ObjectId, Stream, StringFormat};

use crate::compression;
use crate::metadata;

/// Errors raised while assembling a PDF.
#[derive(Debug, thiserror::Error)]
pub enum PdfError {
    /// A page could not be written to disk.
    #[error(transparent)]
    Io(#[from] std::io::Error),
    /// A searchable page returned by the sandbox is not a valid single-page PDF.
    #[error("the OCR'd page is not a valid single-page PDF: {0}")]
    OcrPageInvalid(String),
}

/// A single PDF page, backed by raw RGB pixel data.
///
/// Corresponds to a single-page PDF document produced by PyMuPDF, before it is
/// merged into the final document.
#[derive(Debug)]
pub struct PdfPage {
    pixels: Vec<u8>,
    width: u32,
    height: u32,
}

impl PdfPage {
    /// The number of pixels per row of the page.
    pub fn width(&self) -> u32 {
        self.width
    }

    /// The number of pixel rows of the page.
    pub fn height(&self) -> u32 {
        self.height
    }

    /// The raw RGB bytes of the page (`width * height * 3` bytes).
    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }
}

/// Renders an RGB pixel buffer into a single-page PDF.
///
/// Corresponds to `IsolationProvider.pixels_to_pdf_page`. The `dpi` value is
/// recorded as image metadata by the original code; in this minimal writer the
/// pixels map 1:1 to PDF points, so it is accepted but not used.
pub fn render_pdf_page(pixels: &[u8], width: u32, height: u32, _dpi: u32) -> PdfPage {
    PdfPage {
        pixels: pixels.to_vec(),
        width,
        height,
    }
}

/// Runs OCR on an RGB pixel buffer, returning a searchable PDF page.
///
/// Corresponds to `pixmap.pdfocr_tobytes`. OCR is performed by Tesseract
/// *inside the sandbox*; the host only receives the resulting page PDF. This
/// function validates that the sandbox returned a well-formed single-page PDF
/// before it is merged into the safe document, so no malformed page reaches the
/// final output.
///
/// # Errors
///
/// Returns [`PdfError::OcrPageInvalid`] when the bytes do not parse as a PDF
/// or do not contain exactly one page.
pub fn ocr_pdf_page(pdf_bytes: &[u8]) -> Result<(), PdfError> {
    let doc = Document::load_mem(pdf_bytes)
        .map_err(|error| PdfError::OcrPageInvalid(error.to_string()))?;
    let page_count = doc.page_iter().count();
    if page_count != 1 {
        return Err(PdfError::OcrPageInvalid(format!(
            "expected exactly one page, found {page_count}"
        )));
    }
    Ok(())
}

/// A page of the document being assembled: either a rasterized pixel buffer or
/// a searchable page PDF produced by the sandbox.
#[derive(Debug)]
enum PageEntry {
    /// A page rendered from raw RGB pixels.
    Raster(PdfPage),
    /// A searchable page PDF returned by the OCR phase.
    Ocr(Vec<u8>),
}

/// An in-memory PDF document being assembled page by page.
///
/// Corresponds to `fitz.Document`, which the host-side conversion uses to
/// collect the pages received from the conversion process.
#[derive(Default)]
pub struct PdfDocument {
    pages: Vec<PageEntry>,
}

impl PdfDocument {
    /// Creates an empty PDF document.
    pub fn new() -> Self {
        Self::default()
    }

    /// Appends a page to the document, mirroring `Document.insert_pdf`.
    pub fn insert_pdf(&mut self, page: PdfPage) {
        self.pages.push(PageEntry::Raster(page));
    }

    /// Appends a searchable page PDF produced by the OCR phase.
    ///
    /// # Errors
    ///
    /// Returns [`PdfError::OcrPageInvalid`] when the page is not a well-formed
    /// single-page PDF.
    pub fn insert_ocr_page(&mut self, page_pdf: Vec<u8>) -> Result<(), PdfError> {
        ocr_pdf_page(&page_pdf)?;
        self.pages.push(PageEntry::Ocr(page_pdf));
        Ok(())
    }

    /// Serializes the document and writes it to `path`, mirroring
    /// `Document.save`.
    ///
    /// # Errors
    ///
    /// Returns [`PdfError::Io`] if the file cannot be written, and
    /// [`PdfError::Io`] when the document has no pages.
    pub fn save(&self, path: &Path) -> Result<(), PdfError> {
        let bytes = self.to_bytes()?;
        std::fs::write(path, bytes)?;
        Ok(())
    }

    /// Serializes the document into a complete PDF byte stream.
    ///
    /// Documents made only of raster pages are written by the minimal writer.
    /// Documents that contain searchable pages merge the page PDFs with
    /// `lopdf`, since arbitrary PDF pages cannot be expressed by the minimal
    /// writer.
    ///
    /// # Errors
    ///
    /// Returns [`PdfError::Io`] when the document has no pages.
    pub fn to_bytes(&self) -> Result<Vec<u8>, PdfError> {
        if self.pages.is_empty() {
            return Err(std::io::Error::other("cannot save an empty PDF document").into());
        }
        if self
            .pages
            .iter()
            .any(|entry| matches!(entry, PageEntry::Ocr(_)))
        {
            write_pdf_merged(&self.pages)
        } else {
            let raster: Vec<&PdfPage> = self
                .pages
                .iter()
                .map(|entry| match entry {
                    PageEntry::Raster(page) => page,
                    PageEntry::Ocr(_) => unreachable!("checked above"),
                })
                .collect();
            Ok(write_pdf(&raster))
        }
    }
}

/// Serializes a list of RGB pages into a single PDF document.
///
/// The document uses one page object, one contents stream and one image
/// XObject per page, followed by an `/Info` object, a valid cross-reference
/// table, and the trailer.
fn write_pdf(pages: &[&PdfPage]) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::new();
    out.extend_from_slice(b"%PDF-1.4\n");
    out.extend_from_slice(b"%\xe2\xe3\xcf\xd3\n");

    // Object numbers:
    //   1: catalog
    //   2: page tree
    //   3 + i*3, 4 + i*3, 5 + i*3: page, contents, image for page `i`.
    //   3 + pages.len()*3: info dictionary.
    let mut offsets: Vec<usize> = Vec::new();

    let mut kids = String::new();
    for i in 0..pages.len() {
        if !kids.is_empty() {
            kids.push(' ');
        }
        let page_obj = 3 + i * 3;
        write!(kids, "{page_obj} 0 R").unwrap();
    }

    let catalog = "<< /Type /Catalog /Pages 2 0 R >>";
    let pages_dict = format!("<< /Type /Pages /Kids [{kids}] /Count {} >>", pages.len());
    write_object(&mut out, &mut offsets, 1, catalog.as_bytes());
    write_object(&mut out, &mut offsets, 2, pages_dict.as_bytes());

    for (i, page) in pages.iter().enumerate() {
        let page_obj = 3 + i * 3;
        let contents_obj = page_obj + 1;
        let image_obj = page_obj + 2;

        let contents = format!(
            "q {w} 0 0 {h} 0 0 cm /Im0 Do Q",
            w = page.width,
            h = page.height
        );
        let compressed_contents = compression::compress(contents.as_bytes());

        let page_dict = format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {w} {h}] \
             /Resources << /XObject << /Im0 {image_obj} 0 R >> >> \
             /Contents {contents_obj} 0 R >>",
            w = page.width,
            h = page.height
        );

        let compressed_pixels = compression::compress(&page.pixels);
        let image_dict = format!(
            "/Type /XObject /Subtype /Image /Width {w} /Height {h} \
             /ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /FlateDecode",
            w = page.width,
            h = page.height
        );

        write_dict_object(&mut out, &mut offsets, page_obj, &page_dict);
        write_stream_object(
            &mut out,
            &mut offsets,
            contents_obj,
            "/Filter /FlateDecode",
            &compressed_contents,
        );
        write_stream_object(
            &mut out,
            &mut offsets,
            image_obj,
            &image_dict,
            &compressed_pixels,
        );
    }

    let info_obj = 3 + pages.len() * 3;
    let info = metadata::info_dict(&dz_core::util::get_version());
    write_dict_object(&mut out, &mut offsets, info_obj, &info);

    let xref_offset = out.len();
    let count = offsets.len() + 1;
    let mut xref = String::new();
    writeln!(xref, "xref\n0 {count}\n0000000000 65535 f ").unwrap();
    for offset in &offsets {
        writeln!(xref, "{offset:010} 00000 n ").unwrap();
    }
    writeln!(
        xref,
        "trailer\n<< /Size {count} /Root 1 0 R /Info {info_obj} 0 R >>\n\
         startxref\n{xref_offset}\n%%EOF"
    )
    .unwrap();
    out.extend_from_slice(xref.as_bytes());
    out
}

/// Appends a dictionary object, recording the byte offset of the object.
fn write_dict_object(out: &mut Vec<u8>, offsets: &mut Vec<usize>, obj_number: usize, body: &str) {
    write_object(out, offsets, obj_number, body.as_bytes());
}

/// Appends a stream object, recording the byte offset of the object.
///
/// The stream is compressed with `/FlateDecode`; its `/Length` accounts for
/// the trailing end-of-line marker before `endstream`.
fn write_stream_object(
    out: &mut Vec<u8>,
    offsets: &mut Vec<usize>,
    obj_number: usize,
    dict_entries: &str,
    data: &[u8],
) {
    while offsets.len() < obj_number {
        offsets.push(0);
    }
    offsets[obj_number - 1] = out.len();
    out.extend_from_slice(
        format!(
            "{obj_number} 0 obj\n<< {dict_entries} /Length {} >>\nstream\n",
            data.len() + 1
        )
        .as_bytes(),
    );
    out.extend_from_slice(data);
    out.extend_from_slice(b"\nendstream\nendobj\n");
}

/// Appends `obj_number 0 obj\n<body>\nendobj\n` to `out`, recording the byte
/// offset of the object in `offsets`.
fn write_object(out: &mut Vec<u8>, offsets: &mut Vec<usize>, obj_number: usize, body: &[u8]) {
    while offsets.len() < obj_number {
        offsets.push(0);
    }
    offsets[obj_number - 1] = out.len();
    out.extend_from_slice(format!("{obj_number} 0 obj\n").as_bytes());
    out.extend_from_slice(body);
    out.extend_from_slice(b"\nendobj\n");
}

/// Serializes a document that contains searchable pages by merging the object
/// graphs of every page with `lopdf`.
///
/// Raster pages are rendered as image pages; searchable pages are imported from
/// the single-page PDFs produced by the sandbox. The result mirrors what
/// `fitz.Document.insert_pdf` builds: a fresh catalog, page tree, and `/Info`
/// dictionary authored by the sanitizer.
fn write_pdf_merged(entries: &[PageEntry]) -> Result<Vec<u8>, PdfError> {
    let mut doc = Document::new();
    // Objects 1 and 2 are reserved for the catalog and page tree.
    let mut next_id = 3u32;

    let mut catalog = lopdf::Dictionary::new();
    catalog.set("Type", Object::Name(b"Catalog".to_vec()));
    catalog.set("Pages", Object::Reference((2, 0)));
    doc.objects.insert((1, 0), Object::Dictionary(catalog));

    let mut kids: Vec<Object> = Vec::new();
    for entry in entries {
        let page_id = match entry {
            PageEntry::Raster(page) => add_raster_page(&mut doc, &mut next_id, page),
            PageEntry::Ocr(page_pdf) => import_ocr_page(&mut doc, &mut next_id, page_pdf)?,
        };
        kids.push(Object::Reference(page_id));
    }

    let mut pages_tree = lopdf::Dictionary::new();
    pages_tree.set("Type", Object::Name(b"Pages".to_vec()));
    pages_tree.set("Kids", Object::Array(kids));
    pages_tree.set("Count", Object::Integer(entries.len() as i64));
    doc.objects.insert((2, 0), Object::Dictionary(pages_tree));

    doc.trailer.set("Root", Object::Reference((1, 0)));

    // A fixed `/Info` dictionary, mirroring the raster writer.
    let info_id = (next_id, 0);
    next_id += 1;
    let producer = format!(
        "{} {}",
        metadata::PRODUCER_NAME,
        dz_core::util::get_version()
    );
    let mut info = lopdf::Dictionary::new();
    info.set(
        "Producer",
        Object::String(producer.as_bytes().to_vec(), StringFormat::Literal),
    );
    info.set(
        "Creator",
        Object::String(producer.as_bytes().to_vec(), StringFormat::Literal),
    );
    doc.objects.insert(info_id, Object::Dictionary(info));
    doc.trailer.set("Info", Object::Reference(info_id));

    doc.max_id = next_id - 1;
    let mut out = Vec::new();
    doc.save_to(&mut out).map_err(PdfError::Io)?;
    Ok(out)
}

/// Adds a raster page to a `lopdf` document, embedding its pixels as a
/// Flate-compressed image XObject.
fn add_raster_page(doc: &mut Document, next_id: &mut u32, page: &PdfPage) -> ObjectId {
    let image_id = (*next_id, 0);
    *next_id += 1;
    let contents_id = (*next_id, 0);
    *next_id += 1;
    let page_id = (*next_id, 0);
    *next_id += 1;

    let mut image_dict = lopdf::Dictionary::new();
    image_dict.set("Type", Object::Name(b"XObject".to_vec()));
    image_dict.set("Subtype", Object::Name(b"Image".to_vec()));
    image_dict.set("Width", Object::Integer(i64::from(page.width)));
    image_dict.set("Height", Object::Integer(i64::from(page.height)));
    image_dict.set("ColorSpace", Object::Name(b"DeviceRGB".to_vec()));
    image_dict.set("BitsPerComponent", Object::Integer(8));
    image_dict.set("Filter", Object::Name(b"FlateDecode".to_vec()));
    doc.objects.insert(
        image_id,
        Object::Stream(Stream::new(image_dict, compression::compress(&page.pixels))),
    );

    let contents = format!(
        "q {w} 0 0 {h} 0 0 cm /Im0 Do Q",
        w = page.width,
        h = page.height
    );
    let mut contents_dict = lopdf::Dictionary::new();
    contents_dict.set("Filter", Object::Name(b"FlateDecode".to_vec()));
    doc.objects.insert(
        contents_id,
        Object::Stream(Stream::new(
            contents_dict,
            compression::compress(contents.as_bytes()),
        )),
    );

    let mut page_dict = lopdf::Dictionary::new();
    page_dict.set("Type", Object::Name(b"Page".to_vec()));
    page_dict.set("Parent", Object::Reference((2, 0)));
    page_dict.set(
        "MediaBox",
        Object::Array(vec![
            Object::Integer(0),
            Object::Integer(0),
            Object::Integer(i64::from(page.width)),
            Object::Integer(i64::from(page.height)),
        ]),
    );
    let mut resources = lopdf::Dictionary::new();
    let mut xobject = lopdf::Dictionary::new();
    xobject.set("Im0", Object::Reference(image_id));
    resources.set("XObject", Object::Dictionary(xobject));
    page_dict.set("Resources", Object::Dictionary(resources));
    page_dict.set("Contents", Object::Reference(contents_id));
    doc.objects.insert(page_id, Object::Dictionary(page_dict));

    page_id
}

/// Imports a searchable single-page PDF into a `lopdf` document, appending its
/// page to the destination page tree.
///
/// The object numbers of the source document are renumbered so they cannot
/// collide with the destination document, and every reference is rewritten to
/// follow. This mirrors the object-graph merge that `fitz.Document.insert_pdf`
/// performs. The source catalog and page tree are dropped: the page's `/Parent`
/// is redirected to the destination page tree.
fn import_ocr_page(
    doc: &mut Document,
    next_id: &mut u32,
    page_pdf: &[u8],
) -> Result<ObjectId, PdfError> {
    let mut src = Document::load_mem(page_pdf)
        .map_err(|error| PdfError::OcrPageInvalid(error.to_string()))?;

    let catalog_id = src
        .trailer
        .get(b"Root")
        .ok()
        .and_then(|object| object.as_reference().ok())
        .ok_or_else(|| PdfError::OcrPageInvalid("missing document catalog".to_string()))?;
    let pages_id = src
        .catalog()
        .map_err(|error| PdfError::OcrPageInvalid(error.to_string()))?
        .get(b"Pages")
        .ok()
        .and_then(|object| object.as_reference().ok())
        .ok_or_else(|| PdfError::OcrPageInvalid("missing page tree".to_string()))?;

    let page_ids: Vec<ObjectId> = src.page_iter().collect();
    if page_ids.len() != 1 {
        return Err(PdfError::OcrPageInvalid(format!(
            "expected exactly one page, found {}",
            page_ids.len()
        )));
    }

    // Renumber every object of the source document, and point the references
    // to its catalog and page tree at the destination's own objects.
    let mut id_map: HashMap<ObjectId, ObjectId> = HashMap::new();
    for &old_id in src.objects.keys() {
        id_map.insert(old_id, (*next_id, 0));
        *next_id += 1;
    }
    id_map.insert(catalog_id, (1, 0));
    id_map.insert(pages_id, (2, 0));

    // The catalog and page tree are replaced by the destination's own.
    src.objects.remove(&catalog_id);
    src.objects.remove(&pages_id);

    let objects: Vec<(ObjectId, Object)> = std::mem::take(&mut src.objects).into_iter().collect();
    for ((old_id, _gen), mut object) in objects {
        rewrite_references(&mut object, &id_map);
        let new_id = id_map[&(old_id, _gen)];
        doc.objects.insert(new_id, object);
        if new_id.0 > doc.max_id {
            doc.max_id = new_id.0;
        }
    }

    let page_id = id_map[&page_ids[0]];
    Ok(page_id)
}

/// Rewrites every object reference according to `id_map`, descending into
/// arrays, dictionaries and streams.
fn rewrite_references(object: &mut Object, id_map: &HashMap<ObjectId, ObjectId>) {
    match object {
        Object::Reference(id) => {
            if let Some(new_id) = id_map.get(id) {
                *object = Object::Reference(*new_id);
            }
        }
        Object::Array(items) => {
            for item in items.iter_mut() {
                rewrite_references(item, id_map);
            }
        }
        Object::Dictionary(dict) => {
            for (_key, value) in dict.iter_mut() {
                rewrite_references(value, id_map);
            }
        }
        Object::Stream(stream) => {
            for (_key, value) in stream.dict.iter_mut() {
                rewrite_references(value, id_map);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compression::decompress;

    fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
        haystack
            .windows(needle.len())
            .position(|window| window == needle)
    }

    /// Extracts the decoded payload of every stream in a serialized PDF.
    fn extract_streams(bytes: &[u8]) -> Vec<Vec<u8>> {
        let mut streams = Vec::new();
        let mut pos = 0;
        while let Some(rel_end) = find_subslice(&bytes[pos..], b"endstream") {
            let end = pos + rel_end;
            // Find the last `stream\n` marker that precedes this `endstream`.
            let mut marker = None;
            let mut search = 0;
            while let Some(rel) = find_subslice(&bytes[search..end], b"stream\n") {
                marker = Some(search + rel);
                search = marker.unwrap() + 7;
            }
            if let Some(m) = marker {
                streams.push(bytes[m + 7..end - 1].to_vec());
            }
            pos = end + 9;
        }
        streams
    }

    fn single_page_document() -> PdfDocument {
        let mut doc = PdfDocument::new();
        doc.insert_pdf(render_pdf_page(&[0x41u8; 9 * 9 * 3], 9, 9, 150));
        doc
    }

    #[test]
    fn rendered_page_is_embedded_as_raw_rgb() {
        let page = render_pdf_page(&[0x41u8; 9 * 9 * 3], 9, 9, 150);
        assert_eq!(page.pixels.len(), 9 * 9 * 3);
        assert_eq!(page.width, 9);
        assert_eq!(page.height, 9);
    }

    #[test]
    fn pdf_starts_with_header_and_ends_with_eof() {
        let bytes = single_page_document().to_bytes().unwrap();
        assert!(bytes.starts_with(b"%PDF-1.4"));
        assert!(bytes.ends_with(b"%%EOF\n"));
    }

    #[test]
    fn pdf_xref_offsets_point_to_their_objects() {
        let mut doc = PdfDocument::new();
        doc.insert_pdf(render_pdf_page(&[0x41u8; 9 * 9 * 3], 9, 9, 150));
        doc.insert_pdf(render_pdf_page(&[0x42u8; 9 * 9 * 3], 9, 9, 150));
        let bytes = doc.to_bytes().unwrap();

        let marker = b"startxref\n";
        let start = find_subslice(&bytes, marker).unwrap() + marker.len();
        let end = bytes[start..].iter().position(|&b| b == b'\n').unwrap() + start;
        let xref_off: usize = std::str::from_utf8(&bytes[start..end])
            .unwrap()
            .parse()
            .unwrap();
        assert!(xref_off < bytes.len());

        let table = &bytes[xref_off..];
        assert!(table.starts_with(b"xref\n"));
        let header_end = find_subslice(table, b"\n").unwrap() + 1;
        let num_start = header_end;
        let num_end = table[num_start..].iter().position(|&b| b == b'\n').unwrap() + num_start;
        let count: usize = std::str::from_utf8(&table[num_start..num_end])
            .unwrap()
            .split_whitespace()
            .nth(1)
            .unwrap()
            .parse()
            .unwrap();

        for obj in 1..count {
            let entry = &table[num_end + 1 + (obj * 20)..num_end + 1 + ((obj + 1) * 20)];
            let offset: usize = std::str::from_utf8(&entry[..10])
                .unwrap()
                .trim()
                .parse()
                .unwrap();
            let expected = format!("{obj} 0 obj");
            assert!(
                bytes[offset..].starts_with(expected.as_bytes()),
                "xref entry for object {obj} does not point at its object"
            );
        }
        assert!(bytes.windows(8).any(|w| w == b"/Count 2"));
    }

    #[test]
    fn image_stream_compresses_the_pixels() {
        let bytes = single_page_document().to_bytes().unwrap();
        let streams = extract_streams(&bytes);
        assert_eq!(streams.len(), 2);
        let decoded: Vec<Vec<u8>> = streams.iter().map(|s| decompress(s).unwrap()).collect();
        // One stream is the page contents, the other the embedded image.
        assert!(decoded.contains(&vec![0x41u8; 9 * 9 * 3]));
        assert!(decoded.iter().any(|d| *d == b"q 9 0 0 9 0 0 cm /Im0 Do Q"));
    }

    #[test]
    fn contents_stream_is_flate_compressed() {
        let bytes = single_page_document().to_bytes().unwrap();
        let contents = find_subslice(&bytes, b"/Filter /FlateDecode").unwrap();
        assert!(contents > 0);
    }

    #[test]
    fn info_dictionary_is_written_and_referenced() {
        let bytes = single_page_document().to_bytes().unwrap();
        assert!(bytes
            .windows(b"/Producer (".len())
            .any(|w| w == b"/Producer ("));
        // One page: catalog (1), page tree (2), page (3), contents (4), image
        // (5), then the info dictionary (6).
        assert!(find_subslice(&bytes, b"/Info 6 0 R").is_some());
    }

    #[test]
    fn empty_document_cannot_be_serialized() {
        let doc = PdfDocument::new();
        assert!(doc.to_bytes().is_err());
    }

    /// Builds a single-page PDF with `lopdf`, as the sandbox's Tesseract step
    /// would produce.
    fn synthetic_single_page_pdf() -> Vec<u8> {
        let mut doc = Document::new();
        let mut catalog = lopdf::Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("Pages", Object::Reference((2, 0)));
        doc.objects.insert((1, 0), Object::Dictionary(catalog));

        let mut pages = lopdf::Dictionary::new();
        pages.set("Type", Object::Name(b"Pages".to_vec()));
        pages.set("Kids", Object::Array(vec![Object::Reference((3, 0))]));
        pages.set("Count", Object::Integer(1));
        doc.objects.insert((2, 0), Object::Dictionary(pages));

        let mut page = lopdf::Dictionary::new();
        page.set("Type", Object::Name(b"Page".to_vec()));
        page.set("Parent", Object::Reference((2, 0)));
        page.set(
            "MediaBox",
            Object::Array(vec![
                Object::Integer(0),
                Object::Integer(0),
                Object::Integer(100),
                Object::Integer(100),
            ]),
        );
        doc.objects.insert((3, 0), Object::Dictionary(page));

        doc.trailer.set("Root", Object::Reference((1, 0)));
        doc.max_id = 3;
        let mut out = Vec::new();
        doc.save_to(&mut out).unwrap();
        out
    }

    #[test]
    fn ocr_pdf_page_accepts_a_single_page_pdf() {
        let page = synthetic_single_page_pdf();
        assert!(ocr_pdf_page(&page).is_ok());
    }

    #[test]
    fn ocr_pdf_page_rejects_malformed_pages() {
        let error = ocr_pdf_page(b"this is not a PDF").unwrap_err();
        assert!(matches!(error, PdfError::OcrPageInvalid(_)));
    }

    #[test]
    fn ocr_pdf_page_rejects_documents_with_multiple_pages() {
        let mut doc = Document::new();
        doc.objects.insert(
            (1, 0),
            Object::Dictionary({
                let mut catalog = lopdf::Dictionary::new();
                catalog.set("Type", Object::Name(b"Catalog".to_vec()));
                catalog.set("Pages", Object::Reference((2, 0)));
                catalog
            }),
        );
        doc.objects.insert(
            (2, 0),
            Object::Dictionary({
                let mut pages = lopdf::Dictionary::new();
                pages.set("Type", Object::Name(b"Pages".to_vec()));
                pages.set(
                    "Kids",
                    Object::Array(vec![Object::Reference((3, 0)), Object::Reference((4, 0))]),
                );
                pages.set("Count", Object::Integer(2));
                pages
            }),
        );
        for id in 3..=4 {
            doc.objects.insert(
                (id, 0),
                Object::Dictionary({
                    let mut page = lopdf::Dictionary::new();
                    page.set("Type", Object::Name(b"Page".to_vec()));
                    page.set("Parent", Object::Reference((2, 0)));
                    page
                }),
            );
        }
        doc.trailer.set("Root", Object::Reference((1, 0)));
        doc.max_id = 4;
        let mut out = Vec::new();
        doc.save_to(&mut out).unwrap();

        let error = ocr_pdf_page(&out).unwrap_err();
        match error {
            PdfError::OcrPageInvalid(message) => {
                assert!(message.contains("exactly one page"), "message: {message}");
            }
            other => panic!("unexpected error variant: {other:?}"),
        }
    }

    #[test]
    fn merged_document_inserts_searchable_pages() {
        let mut doc = PdfDocument::new();
        doc.insert_ocr_page(synthetic_single_page_pdf()).unwrap();
        doc.insert_ocr_page(synthetic_single_page_pdf()).unwrap();

        let bytes = doc.to_bytes().unwrap();
        crate::validator::validate_pdf(&bytes).unwrap();

        let parsed = Document::load_mem(&bytes).unwrap();
        assert_eq!(parsed.get_pages().len(), 2);
        // Both imported pages keep their media boxes.
        for id in parsed.get_pages().values() {
            let page = parsed.get_dictionary(*id).unwrap();
            let media_box = page.get(b"MediaBox").unwrap().as_array().unwrap();
            assert_eq!(media_box.len(), 4);
        }
    }

    #[test]
    fn merged_document_mixes_raster_and_searchable_pages() {
        let mut doc = PdfDocument::new();
        doc.insert_pdf(render_pdf_page(&[0x41u8; 9 * 9 * 3], 9, 9, 150));
        doc.insert_ocr_page(synthetic_single_page_pdf()).unwrap();

        let bytes = doc.to_bytes().unwrap();
        crate::validator::validate_pdf(&bytes).unwrap();

        let parsed = Document::load_mem(&bytes).unwrap();
        assert_eq!(parsed.get_pages().len(), 2);
    }

    #[test]
    fn insert_ocr_page_rejects_malformed_pages() {
        let mut doc = PdfDocument::new();
        let error = doc.insert_ocr_page(b"not a PDF".to_vec()).unwrap_err();
        assert!(matches!(error, PdfError::OcrPageInvalid(_)));
        // The malformed page must not have been inserted.
        assert!(doc.pages.is_empty());
    }
}
