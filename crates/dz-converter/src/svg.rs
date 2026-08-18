//! Scalable Vector Graphics rasterization.
//!
//! Corresponds to `dangerzone/conversion/svg.py`. Unlike the bitmap formats,
//! an SVG is rendered with `resvg` (a pure-Rust SVG renderer) at its intrinsic
//! size. Text is converted to paths with the system font database so it
//! survives the untrusted input without ever executing script. The rendered
//! page forms a single page in the conversion wire protocol.

use resvg::tiny_skia::Pixmap;
use resvg::usvg;

use crate::errors::{ConversionError, MAX_PAGE_HEIGHT, MAX_PAGE_WIDTH};

/// Renders an SVG document into RGB8 pixels.
///
/// Returns `(rgb, width, height)`, where `rgb` holds `width * height * 3`
/// bytes in red-green-blue order.
///
/// # Errors
///
/// Returns [`ConversionError::DocCorruptedException`] when the bytes cannot be
/// parsed as an SVG or have no renderable size, and
/// [`ConversionError::MaxPageWidth`] or [`ConversionError::MaxPageHeight`]
/// when the document's intrinsic size exceeds the protocol limits.
pub fn svg_to_rgb(bytes: &[u8]) -> Result<(Vec<u8>, u32, u32), ConversionError> {
    let mut options = usvg::Options::default();
    options.fontdb_mut().load_system_fonts();

    let tree = usvg::Tree::from_data(bytes, &options)
        .map_err(|_| ConversionError::DocCorruptedException)?;

    let size = tree.size().to_int_size();
    let width = size.width();
    let height = size.height();
    if width == 0 || height == 0 {
        return Err(ConversionError::DocCorruptedException);
    }
    if u64::from(width) > u64::from(MAX_PAGE_WIDTH) {
        return Err(ConversionError::MaxPageWidth);
    }
    if u64::from(height) > u64::from(MAX_PAGE_HEIGHT) {
        return Err(ConversionError::MaxPageHeight);
    }

    let mut pixmap = Pixmap::new(width, height).ok_or_else(|| {
        ConversionError::unexpected_conversion("allocating the SVG rendering canvas")
    })?;
    resvg::render(
        &tree,
        resvg::tiny_skia::Transform::default(),
        &mut pixmap.as_mut(),
    );

    // The canvas is RGBA; the protocol expects RGB.
    let rgba = pixmap.data();
    let mut rgb = Vec::with_capacity(width as usize * height as usize * 3);
    for pixel in rgba.chunks_exact(4) {
        rgb.extend_from_slice(&pixel[..3]);
    }
    Ok((rgb, width, height))
}

#[cfg(test)]
mod tests {
    use super::*;

    const RED_RECT: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" width="2" height="2"><rect width="2" height="2" fill="red"/></svg>"#;

    #[test]
    fn renders_a_tiny_svg_into_rgb8() {
        let (rgb, width, height) = svg_to_rgb(RED_RECT.as_bytes()).unwrap();
        assert_eq!((width, height), (2, 2));
        assert_eq!(rgb.len(), 2 * 2 * 3);
        assert!(rgb.chunks_exact(3).all(|pixel| pixel == [255, 0, 0]));
    }

    #[test]
    fn oversized_svgs_are_rejected() {
        let wide = r#"<svg xmlns="http://www.w3.org/2000/svg" width="20000" height="10"/>"#;
        assert_eq!(
            svg_to_rgb(wide.as_bytes()).unwrap_err(),
            ConversionError::MaxPageWidth
        );
        let tall = r#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="20000"/>"#;
        assert_eq!(
            svg_to_rgb(tall.as_bytes()).unwrap_err(),
            ConversionError::MaxPageHeight
        );
    }

    #[test]
    fn corrupt_svg_is_rejected() {
        assert_eq!(
            svg_to_rgb(b"this is not an svg").unwrap_err(),
            ConversionError::DocCorruptedException
        );
    }
}
