//! Bitmap image rasterization.
//!
//! Corresponds to `dangerzone/conversion/image.py`. Images are decoded with the
//! `image` crate and normalized to RGB8, which is the format the conversion
//! wire protocol expects. The image forms a single page.

use crate::errors::{ConversionError, MAX_PAGE_HEIGHT, MAX_PAGE_WIDTH};

/// Decodes a bitmap image into RGB8 pixels.
///
/// Returns `(rgb, width, height)`, where `rgb` holds `width * height * 3`
/// bytes in red-green-blue order.
///
/// # Errors
///
/// Returns [`ConversionError::DocCorruptedException`] when the bytes cannot be
/// decoded as an image, and [`ConversionError::MaxPageWidth`] or
/// [`ConversionError::MaxPageHeight`] when the image exceeds the protocol
/// limits.
pub fn image_to_rgb(bytes: &[u8]) -> Result<(Vec<u8>, u32, u32), ConversionError> {
    let decoded =
        image::load_from_memory(bytes).map_err(|_| ConversionError::DocCorruptedException)?;
    let width = decoded.width();
    let height = decoded.height();
    if u64::from(width) > u64::from(MAX_PAGE_WIDTH) {
        return Err(ConversionError::MaxPageWidth);
    }
    if u64::from(height) > u64::from(MAX_PAGE_HEIGHT) {
        return Err(ConversionError::MaxPageHeight);
    }
    let rgb = decoded.to_rgb8().into_raw();
    Ok((rgb, width, height))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    /// Encodes a tiny 2x2 PNG whose pixels are all opaque red.
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

    #[test]
    fn decodes_a_tiny_png_into_rgb8() {
        let (rgb, width, height) = image_to_rgb(&tiny_png()).unwrap();
        assert_eq!((width, height), (2, 2));
        assert_eq!(rgb.len(), 2 * 2 * 3);
        assert!(rgb.chunks_exact(3).all(|pixel| pixel == [255, 0, 0]));
    }

    #[test]
    fn corrupt_bytes_are_rejected() {
        assert_eq!(
            image_to_rgb(b"this is not an image").unwrap_err(),
            ConversionError::DocCorruptedException
        );
    }
}
