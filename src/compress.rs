//! Compression (Sections 7.5 and 8 of the spec).
//!
//! Allowed algorithms: `none` and `zstd`. RAR and any proprietary format are
//! forbidden. Two kinds exist: `per-file` (each file compressed individually)
//! and `whole-archive` (all files compressed together as a single stream).

use crate::error::{Result, UpkgError};
use crate::header::Compression;

/// Compress `data` with the given algorithm and level.
pub fn compress(algorithm: Compression, level: u32, data: &[u8]) -> Result<Vec<u8>> {
    match algorithm {
        Compression::None => Ok(data.to_vec()),
        Compression::Zstd => zstd::bulk::compress(data, level as i32)
            .map_err(|e| UpkgError::Format(format!("zstd compression failed: {e}"))),
    }
}

/// Decompress `data` with the given algorithm.
pub fn decompress(algorithm: Compression, data: &[u8]) -> Result<Vec<u8>> {
    match algorithm {
        Compression::None => Ok(data.to_vec()),
        // `stream::decode_all` grows its buffer dynamically (the bulk API
        // pre-allocates the whole requested capacity, which is unbounded).
        Compression::Zstd => zstd::stream::decode_all(std::io::Cursor::new(data))
            .map_err(|e| UpkgError::Format(format!("zstd decompression failed: {e}"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn none_is_identity() {
        let data = b"hello world";
        let enc = compress(Compression::None, 0, data).unwrap();
        assert_eq!(enc, data);
        assert_eq!(decompress(Compression::None, &enc).unwrap(), data);
    }

    #[test]
    fn zstd_round_trip() {
        let data = vec![b'a'; 4096];
        for level in [1, 3, 22] {
            let enc = compress(Compression::Zstd, level, &data).unwrap();
            assert!(enc.len() < data.len());
            assert_eq!(decompress(Compression::Zstd, &enc).unwrap(), data);
        }
    }
}
