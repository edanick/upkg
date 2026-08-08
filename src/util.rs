//! Small byte-level helpers: little-endian integers, hex, and hashing.
//!
//! Per Section 7.6 of the spec, all uints (offsets/lengths/sizes are u64,
//! format version and compression level are u32, booleans are one byte) are
//! stored little-endian. Strings are UTF-8, fields are delimited by the NUL
//! byte `0x00`, records by the LF byte `0x0A`.

use sha1::{Digest, Sha1};

/// Compute the SHA-1 (20 bytes) of a byte slice.
pub fn sha1(data: &[u8]) -> [u8; 20] {
    let mut hasher = Sha1::new();
    hasher.update(data);
    hasher.finalize().into()
}

/// Append `value` as little-endian bytes to `out`.
pub fn put_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}

/// Append `value` as little-endian bytes to `out`.
pub fn put_u64(out: &mut Vec<u8>, value: u64) {
    out.extend_from_slice(&value.to_le_bytes());
}

/// Read a u32 little-endian from `buf` at `pos`.
pub fn get_u32(buf: &[u8], pos: usize) -> Result<u32, String> {
    let bytes = buf
        .get(pos..pos + 4)
        .ok_or_else(|| "unexpected end of data while reading u32".to_string())?;
    Ok(u32::from_le_bytes(bytes.try_into().unwrap()))
}

/// Read a u64 little-endian from `buf` at `pos`.
pub fn get_u64(buf: &[u8], pos: usize) -> Result<u64, String> {
    let bytes = buf
        .get(pos..pos + 8)
        .ok_or_else(|| "unexpected end of data while reading u64".to_string())?;
    Ok(u64::from_le_bytes(bytes.try_into().unwrap()))
}

/// Append a NUL-terminated UTF-8 string to `out` (field separator per 7.6).
pub fn put_nul_string(out: &mut Vec<u8>, value: &str) {
    out.extend_from_slice(value.as_bytes());
    out.push(0x00);
}

/// Read a NUL-terminated UTF-8 string from `buf` starting at `pos`.
/// Returns the string and the position just past the NUL byte.
pub fn get_nul_string(buf: &[u8], pos: usize) -> Result<(String, usize), String> {
    let end = buf[pos..]
        .iter()
        .position(|&b| b == 0x00)
        .ok_or_else(|| "unterminated string field".to_string())?;
    let end = pos + end;
    let s = std::str::from_utf8(&buf[pos..end])
        .map_err(|_| "invalid UTF-8 in string field".to_string())?;
    Ok((s.to_string(), end + 1))
}

/// Encode bytes as lowercase hex.
pub fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Decode a hex string (any case) into bytes.
pub fn from_hex(hex: &str) -> Result<Vec<u8>, String> {
    let hex = hex.trim();
    if hex.len() % 2 != 0 {
        return Err("hex string has odd length".to_string());
    }
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).map_err(|_| "invalid hex".to_string()))
        .collect()
}

/// Decode a 20-byte hex SHA-1.
pub fn from_sha1_hex(hex: &str) -> Result<[u8; 20], String> {
    let bytes = from_hex(hex)?;
    if bytes.len() != 20 {
        return Err("SHA-1 must be 40 hex characters".to_string());
    }
    let mut out = [0u8; 20];
    out.copy_from_slice(&bytes);
    Ok(out)
}
