//! Package signing (Section 7.7 of the spec, revision 27).
//!
//! A package *may* carry an optional ed25519 signature over all preceding
//! bytes, stored as the last section of the package. The section layout is a
//! proposal: an 8-byte marker `UPKGSIG`, the 32-byte ed25519 public key, then
//! the 64-byte signature (104 bytes total). The marker makes presence
//! unambiguous: a package carries a signature iff its last 104 bytes start
//! with `UPKGSIG`.
//!
//! Private keys are stored as 64 hex characters (32 seed bytes). `upkg
//! keygen` generates them (proposal - the spec does not define a keygen
//! command, so this is a clearly-marked addition).

use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};

use crate::error::{Result, UpkgError};

/// Marker identifying the signature section (proposal).
pub const SIGNATURE_MARKER: &[u8; 7] = b"UPKGSIG";
/// Public key length (32 bytes).
pub const PUBLIC_KEY_LEN: usize = 32;
/// Signature length (64 bytes).
pub const SIGNATURE_LEN: usize = 64;
/// Total length of the signature section.
pub const SIGNATURE_SECTION_LEN: usize = 7 + PUBLIC_KEY_LEN + SIGNATURE_LEN;

/// A parsed signature section.
#[derive(Debug, Clone, Copy)]
pub struct SignatureSection {
    pub public_key: [u8; PUBLIC_KEY_LEN],
    pub signature: [u8; SIGNATURE_LEN],
}

/// Encode the signature section bytes.
pub fn encode_section(sig: &SignatureSection) -> Vec<u8> {
    let mut out = Vec::with_capacity(SIGNATURE_SECTION_LEN);
    out.extend_from_slice(SIGNATURE_MARKER);
    out.extend_from_slice(&sig.public_key);
    out.extend_from_slice(&sig.signature);
    out
}

/// If the last `SIGNATURE_SECTION_LEN` bytes of `file` start with the marker,
/// parse and return the signature section.
pub fn extract_section(file: &[u8]) -> Option<SignatureSection> {
    if file.len() < SIGNATURE_SECTION_LEN {
        return None;
    }
    let start = file.len() - SIGNATURE_SECTION_LEN;
    let tail = &file[start..];
    if &tail[..7] != SIGNATURE_MARKER {
        return None;
    }
    let mut public_key = [0u8; PUBLIC_KEY_LEN];
    public_key.copy_from_slice(&tail[7..7 + PUBLIC_KEY_LEN]);
    let mut signature = [0u8; SIGNATURE_LEN];
    signature.copy_from_slice(&tail[7 + PUBLIC_KEY_LEN..]);
    Some(SignatureSection { public_key, signature })
}

/// Generate a random 32-byte signing seed (for `upkg keygen`).
pub fn generate_seed() -> [u8; 32] {
    use rand::RngCore;
    let mut seed = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut seed);
    seed
}

/// Load a seed from a hex key file (64 hex chars).
pub fn load_seed(path: &std::path::Path) -> Result<[u8; 32]> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| UpkgError::Config(format!("cannot read private key `{}`: {e}", path.display())))?;
    let hex: String = text.chars().filter(|c| !c.is_whitespace()).collect();
    let bytes = crate::util::from_hex(&hex)
        .map_err(|e| UpkgError::Config(format!("invalid private key file `{}`: {e}", path.display())))?;
    if bytes.len() != 32 {
        return Err(UpkgError::Config(format!(
            "private key file `{}` must contain 32 bytes (64 hex chars), got {}",
            path.display(),
            bytes.len()
        )));
    }
    let mut seed = [0u8; 32];
    seed.copy_from_slice(&bytes);
    Ok(seed)
}

/// Sign `message` with the seed; returns the signature section.
pub fn sign(message: &[u8], seed: &[u8; 32]) -> SignatureSection {
    let signing_key = SigningKey::from_bytes(seed);
    let verifying_key: VerifyingKey = signing_key.verifying_key();
    let signature: Signature = signing_key.sign(message);
    SignatureSection {
        public_key: verifying_key.to_bytes(),
        signature: signature.to_bytes(),
    }
}

/// Verify a signature section over `message`.
pub fn verify(message: &[u8], section: &SignatureSection) -> Result<()> {
    let verifying_key = VerifyingKey::from_bytes(&section.public_key)
        .map_err(|_| UpkgError::Verify("invalid ed25519 public key".into()))?;
    let signature = Signature::from_bytes(&section.signature);
    verifying_key
        .verify(message, &signature)
        .map_err(|_| UpkgError::Verify("ed25519 signature is invalid".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sign_and_verify() {
        let seed = generate_seed();
        let msg = b"hello upkg";
        let section = sign(msg, &seed);
        assert!(verify(msg, &section).is_ok());
        assert!(verify(b"tampered", &section).is_err());
    }

    #[test]
    fn section_round_trip() {
        let seed = generate_seed();
        let section = sign(b"data", &seed);
        let bytes = encode_section(&section);
        assert_eq!(bytes.len(), SIGNATURE_SECTION_LEN);
        let extracted = extract_section(&bytes).unwrap();
        assert_eq!(extracted.public_key, section.public_key);
        assert_eq!(extracted.signature, section.signature);

        // With a longer prefix
        let mut file = b"prefix-bytes".to_vec();
        file.extend_from_slice(&bytes);
        let extracted = extract_section(&file).unwrap();
        assert_eq!(extracted.public_key, section.public_key);

        // Without a marker
        assert!(extract_section(b"no signature here").is_none());
    }
}
