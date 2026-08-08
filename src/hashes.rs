//! The hash block (Section 7.3 of the spec).
//!
//! Four 20-byte SHA-1 hashes are stored immediately after the header:
//! header SHA-1, master SHA-1, tree SHA-1, metadata SHA-1 (80 bytes total,
//! in that order per the layout "hdr+master+tree+md"). Together they cover
//! every part of the package except the hash block itself.

use crate::error::{Result, UpkgError};

/// Byte length of the hash block: 4 hashes * 20 bytes.
pub const HASH_BLOCK_LEN: usize = 80;

/// The four hashes stored right after the header.
#[derive(Debug, Clone, Copy)]
pub struct HashBlock {
    /// SHA-1 over the entire header - detects a broken header.
    pub header_sha1: [u8; 20],
    /// Archive-level hash (see the spec for the covered input per kind).
    pub master_sha1: [u8; 20],
    /// SHA-1 over the entire entries tree (its stored bytes).
    pub tree_sha1: [u8; 20],
    /// SHA-1 over the entire metadata file (its stored bytes).
    pub metadata_sha1: [u8; 20],
}

impl HashBlock {
    /// Serialize to exactly 80 bytes.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(HASH_BLOCK_LEN);
        out.extend_from_slice(&self.header_sha1);
        out.extend_from_slice(&self.master_sha1);
        out.extend_from_slice(&self.tree_sha1);
        out.extend_from_slice(&self.metadata_sha1);
        out
    }

    /// Parse 80 bytes from `buf` at `pos`.
    pub fn parse(buf: &[u8], pos: usize) -> Result<HashBlock> {
        let end = pos + HASH_BLOCK_LEN;
        let block = buf
            .get(pos..end)
            .ok_or_else(|| UpkgError::Format("hash block truncated".into()))?;
        Ok(HashBlock {
            header_sha1: block[0..20].try_into().unwrap(),
            master_sha1: block[20..40].try_into().unwrap(),
            tree_sha1: block[40..60].try_into().unwrap(),
            metadata_sha1: block[60..80].try_into().unwrap(),
        })
    }
}

/// Compare a stored hash with a computed one.
pub fn check(name: &str, stored: &[u8; 20], computed: &[u8; 20]) -> Result<()> {
    if stored != computed {
        return Err(UpkgError::Verify(format!(
            "{name} mismatch (stored {}, computed {})",
            crate::util::to_hex(stored),
            crate::util::to_hex(computed)
        )));
    }
    Ok(())
}

