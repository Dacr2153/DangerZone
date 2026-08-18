//! Defense-in-depth validation of the reconstructed safe PDF.
//!
//! The PDF is authored by [`crate::pdf`], so its object graph is fully known in
//! advance. The validator re-parses the serialized output and walks every
//! object reachable from the document catalog, failing if any dictionary key or
//! action would reintroduce active content (JavaScript, embedded files, launch
//! actions, forms, ...). This guards against a regression in the writer that
//! would otherwise ship dangerous content into the sanitized document.

use std::collections::HashSet;

use lopdf::{Document, Object, ObjectId};

/// Errors raised while validating a safe PDF.
#[derive(Debug, thiserror::Error)]
pub enum ValidationError {
    /// The bytes do not form a parseable PDF.
    #[error("the safe PDF cannot be parsed: {0}")]
    NotParseable(#[from] lopdf::Error),
    /// The trailer does not point at a document catalog.
    #[error("the safe PDF has no document catalog")]
    MissingCatalog,
    /// The catalog does not point at a page tree.
    #[error("the safe PDF has no page tree")]
    MissingPages,
    /// The page tree contains no pages.
    #[error("the safe PDF has no pages")]
    NoPages,
    /// The page tree is malformed or contains a cycle.
    #[error("the safe PDF page tree is malformed or cyclic")]
    MalformedPageTree,
    /// The document contains a feature that must not survive sanitization.
    #[error("the safe PDF contains a forbidden feature: {0}")]
    ForbiddenFeature(&'static str),
}

/// Validates that `bytes` form a safe PDF.
///
/// The document must parse and contain at least one page, and none of the
/// dictionaries reachable from the catalog may carry a feature that the
/// sanitizer is meant to strip.
///
/// # Errors
///
/// Returns a [`ValidationError`] when any of the checks above fails.
pub fn validate_pdf(bytes: &[u8]) -> Result<(), ValidationError> {
    let doc = Document::load_mem(bytes)?;
    validate_document(&doc)
}

/// Validates a parsed [`Document`].
fn validate_document(doc: &Document) -> Result<(), ValidationError> {
    let catalog_id = match doc.trailer.get(b"Root")? {
        Object::Reference(id) => *id,
        _ => return Err(ValidationError::MissingCatalog),
    };

    let mut visited = HashSet::new();
    check_object(doc, catalog_id, &mut visited)?;

    let mut pages_visited = HashSet::new();
    let page_count = count_pages(doc, catalog_id, &mut pages_visited)?;
    if page_count == 0 {
        return Err(ValidationError::NoPages);
    }
    Ok(())
}

/// Checks a single object, following its references into the object graph.
fn check_object(
    doc: &Document,
    id: ObjectId,
    visited: &mut HashSet<ObjectId>,
) -> Result<(), ValidationError> {
    if !visited.insert(id) {
        return Ok(());
    }
    let obj = doc.get_object(id)?;
    check_value(doc, obj, visited)
}

/// Checks a value, descending into dictionaries, streams and arrays.
fn check_value(
    doc: &Document,
    value: &Object,
    visited: &mut HashSet<ObjectId>,
) -> Result<(), ValidationError> {
    match value {
        Object::Dictionary(dict) => check_dictionary(doc, dict, visited),
        Object::Stream(stream) => check_dictionary(doc, &stream.dict, visited),
        Object::Array(items) => {
            for item in items {
                check_value(doc, item, visited)?;
            }
            Ok(())
        }
        Object::Reference(id) => check_object(doc, *id, visited),
        _ => Ok(()),
    }
}

/// Checks a dictionary for forbidden keys before descending into its values.
fn check_dictionary(
    doc: &Document,
    dict: &lopdf::Dictionary,
    visited: &mut HashSet<ObjectId>,
) -> Result<(), ValidationError> {
    for (key, value) in dict.iter() {
        if let Some(feature) = forbidden_feature(key, value) {
            return Err(ValidationError::ForbiddenFeature(feature));
        }
        check_value(doc, value, visited)?;
    }
    Ok(())
}

/// Returns the human-readable name of the feature `key` (or the `/S` value)
/// represents when it must be rejected, or `None` when it is benign.
fn forbidden_feature(key: &[u8], value: &Object) -> Option<&'static str> {
    match key {
        b"JavaScript" | b"JS" => Some("JavaScript"),
        b"EmbeddedFiles" => Some("embedded files"),
        b"Filespec" => Some("embedded file specification"),
        b"EF" => Some("embedded file"),
        b"Launch" => Some("launch action"),
        b"GoToE" => Some("embedded go-to action"),
        b"OpenAction" => Some("open action"),
        b"AA" => Some("additional action"),
        b"AcroForm" => Some("form"),
        // An action dictionary whose `/S` value names a dangerous action.
        b"S" if matches!(value, Object::Name(name) if FORBIDDEN_ACTION_NAMES.contains(&name.as_slice())) => {
            Some("named action")
        }
        _ => None,
    }
}

/// Action names that may never appear as the `/S` value of an action.
const FORBIDDEN_ACTION_NAMES: &[&[u8]] = &[
    b"JavaScript",
    b"Launch",
    b"GoToE",
    b"GoToR",
    b"RichMediaExecute",
];

/// Counts the pages reachable from the catalog's page tree.
fn count_pages(
    doc: &Document,
    catalog_id: ObjectId,
    visited: &mut HashSet<ObjectId>,
) -> Result<usize, ValidationError> {
    let catalog = doc.get_object(catalog_id)?;
    let Object::Dictionary(cat) = catalog else {
        return Err(ValidationError::MissingCatalog);
    };
    let pages_id = match cat.get(b"Pages")? {
        Object::Reference(id) => *id,
        _ => return Err(ValidationError::MissingPages),
    };
    count_pages_in_tree(doc, pages_id, visited)
}

/// Counts the pages in a page-tree node, descending into nested `/Pages` nodes.
fn count_pages_in_tree(
    doc: &Document,
    node_id: ObjectId,
    visited: &mut HashSet<ObjectId>,
) -> Result<usize, ValidationError> {
    if !visited.insert(node_id) {
        return Err(ValidationError::MalformedPageTree);
    }
    let node = doc.get_object(node_id)?;
    let Object::Dictionary(dict) = node else {
        return Err(ValidationError::MalformedPageTree);
    };
    match dict.get(b"Type")? {
        Object::Name(name) if name == b"Pages" => {
            let kids = match dict.get(b"Kids")? {
                Object::Array(kids) => kids,
                _ => return Err(ValidationError::MalformedPageTree),
            };
            let mut total = 0;
            for kid in kids {
                let id = match kid {
                    Object::Reference(id) => *id,
                    _ => return Err(ValidationError::MalformedPageTree),
                };
                total += count_pages_in_tree(doc, id, visited)?;
            }
            Ok(total)
        }
        Object::Name(name) if name == b"Page" => Ok(1),
        _ => Err(ValidationError::MalformedPageTree),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pdf::{render_pdf_page, PdfDocument};

    fn single_page_pdf() -> Vec<u8> {
        let mut doc = PdfDocument::new();
        doc.insert_pdf(render_pdf_page(&[0x41u8; 9 * 9 * 3], 9, 9, 150));
        doc.to_bytes().unwrap()
    }

    /// Builds a small parseable PDF from raw objects, merging `extra` into the
    /// page dictionary.
    fn build_pdf_with(extra: lopdf::Dictionary) -> Vec<u8> {
        let mut doc = lopdf::Document::new();
        let mut catalog = lopdf::Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("Pages", Object::Reference((2, 0)));

        let mut pages = lopdf::Dictionary::new();
        pages.set("Type", Object::Name(b"Pages".to_vec()));
        pages.set("Kids", Object::Array(vec![Object::Reference((3, 0))]));
        pages.set("Count", Object::Integer(1));

        let mut page = lopdf::Dictionary::new();
        page.set("Type", Object::Name(b"Page".to_vec()));
        page.set("Parent", Object::Reference((2, 0)));
        page.set(
            "MediaBox",
            Object::Array(vec![
                Object::Integer(0),
                Object::Integer(0),
                Object::Integer(9),
                Object::Integer(9),
            ]),
        );
        for (key, value) in extra {
            page.set(key, value);
        }

        doc.add_object(Object::Dictionary(catalog));
        doc.add_object(Object::Dictionary(pages));
        doc.add_object(Object::Dictionary(page));
        doc.trailer.set("Root", Object::Reference((1, 0)));

        let mut out = Vec::new();
        doc.save_to(&mut out).unwrap();
        out
    }

    #[test]
    fn accepts_output_produced_by_the_writer() {
        assert!(validate_pdf(&single_page_pdf()).is_ok());
    }

    #[test]
    fn rejects_pdf_containing_javascript_action() {
        let mut extra = lopdf::Dictionary::new();
        extra.set("S", Object::Name(b"JavaScript".to_vec()));
        extra.set(
            "JS",
            Object::String(b"app.alert(1);".to_vec(), lopdf::StringFormat::Literal),
        );
        let bytes = build_pdf_with(extra);
        assert!(matches!(
            validate_pdf(&bytes),
            Err(ValidationError::ForbiddenFeature(_))
        ));
    }

    #[test]
    fn rejects_pdf_with_additional_actions() {
        let mut extra = lopdf::Dictionary::new();
        let mut action = lopdf::Dictionary::new();
        action.set("S", Object::Name(b"Launch".to_vec()));
        extra.set("AA", Object::Dictionary(action));
        let bytes = build_pdf_with(extra);
        assert!(matches!(
            validate_pdf(&bytes),
            Err(ValidationError::ForbiddenFeature("additional action"))
        ));
    }

    #[test]
    fn rejects_pdf_with_embedded_file_dictionary() {
        let mut extra = lopdf::Dictionary::new();
        extra.set("EF", Object::Dictionary(lopdf::Dictionary::new()));
        let bytes = build_pdf_with(extra);
        assert!(matches!(
            validate_pdf(&bytes),
            Err(ValidationError::ForbiddenFeature("embedded file"))
        ));
    }

    #[test]
    fn rejects_garbage_bytes() {
        assert!(validate_pdf(b"this is not a pdf at all").is_err());
    }

    #[test]
    fn rejects_pdf_with_no_pages() {
        let mut doc = lopdf::Document::new();
        let mut catalog = lopdf::Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("Pages", Object::Reference((2, 0)));
        let mut pages = lopdf::Dictionary::new();
        pages.set("Type", Object::Name(b"Pages".to_vec()));
        pages.set("Kids", Object::Array(vec![]));
        pages.set("Count", Object::Integer(0));
        doc.add_object(Object::Dictionary(catalog));
        doc.add_object(Object::Dictionary(pages));
        doc.trailer.set("Root", Object::Reference((1, 0)));
        let mut out = Vec::new();
        doc.save_to(&mut out).unwrap();
        assert!(matches!(validate_pdf(&out), Err(ValidationError::NoPages)));
    }
}
