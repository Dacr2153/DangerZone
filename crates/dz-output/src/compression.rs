//! Flate (Deflate) compression helpers for PDF streams.
//!
//! The safe PDF writer compresses image and content streams with the
//! `/FlateDecode` filter, which is the most widely supported compression
//! filter in PDF readers. The same helpers are used by the [`crate::validator`]
//! when it needs to inspect decompressed stream contents.

use std::io::{Read, Write};

use flate2::write::ZlibEncoder;
use flate2::{read::ZlibDecoder, Compression};

/// Compresses `data` with the Flate/Deflate algorithm used by the PDF
/// `/FlateDecode` filter.
///
/// The output is a `zlib` wrapper (a 2-byte header, the raw deflate stream,
/// and an Adler-32 checksum), which is exactly what PDF viewers expect from
/// `/FlateDecode`.
pub fn compress(data: &[u8]) -> Vec<u8> {
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
    // Writing to a `Vec` sink cannot fail; `finish` only returns the sink.
    encoder
        .write_all(data)
        .expect("writing to an in-memory buffer cannot fail");
    encoder
        .finish()
        .expect("finishing an in-memory zlib encoder cannot fail")
}

/// Decompresses a Flate/Deflate stream, returning `None` when the input is not
/// a valid compressed stream.
pub fn decompress(data: &[u8]) -> Option<Vec<u8>> {
    let mut decoder = ZlibDecoder::new(data);
    let mut out = Vec::new();
    decoder.read_to_end(&mut out).ok().map(|_| out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compressed_data_round_trips_through_decompress() {
        let payload = b"the quick brown fox jumps over the lazy dog";
        let compressed = compress(payload);
        assert_eq!(decompress(&compressed).unwrap(), payload);
    }

    #[test]
    fn compressed_data_is_smaller_than_repetitive_input() {
        let payload = vec![0x41u8; 4096];
        let compressed = compress(&payload);
        assert!(compressed.len() < payload.len());
    }

    #[test]
    fn decompress_rejects_garbage_input() {
        assert!(decompress(b"this is not deflate data").is_none());
    }
}
