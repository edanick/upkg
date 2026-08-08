//! Package header (Section 7.2 of the spec).
//!
//! The header is the fixed metadata block at the start of a package. Field
//! order is a proposal (the spec leaves it open). Encoding proposal:
//!
//! ```text
//! offset  size  field
//! 0       4     magic "UPKG"
//! 4       4     format version (u32 LE)
//! 8       4     compression level (u32 LE; 0 when algorithm is none)
//! 12      1     strict (0x00/0x01)
//! 13      8     entries tree start offset (u64 LE)
//! 21      8     entries tree end offset (u64 LE)
//! 29      8     metadata file start offset (u64 LE)
//! 37      8     metadata file end offset (u64 LE)
//! 45      ...   NUL-terminated UTF-8 strings, in order: OS, OS version,
//!               min, max, compression algorithm, compression kind, distro,
//!               package type, arch (empty string = field absent)
//! ```
//!
//! The whole block is self-delimiting: the reader knows the field count, so
//! the header length is `45 + sum(len + 1 for each string)`. The header
//! SHA-1 (Section 7.3) covers exactly these bytes.

use crate::error::{Result, UpkgError};
use crate::util::{get_nul_string, get_u32, get_u64, put_nul_string, put_u32, put_u64};

/// Magic bytes identifying the format (revision 8): UTF-8 `UPKG`.
pub const MAGIC: &[u8; 4] = b"UPKG";

/// Version of the UPKG format itself.
pub const FORMAT_VERSION: u32 = 1;

/// Target OS (Section 4): exactly one of `windows`, `linux`, `mac`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Os {
    Windows,
    Linux,
    Mac,
}

impl Os {
    pub fn as_str(&self) -> &'static str {
        match self {
            Os::Windows => "windows",
            Os::Linux => "linux",
            Os::Mac => "mac",
        }
    }

    /// Map the Rust `std::env::consts::OS` value to an `Os`.
    pub fn from_std(value: &str) -> Option<Os> {
        match value {
            "windows" => Some(Os::Windows),
            "linux" => Some(Os::Linux),
            "macos" => Some(Os::Mac),
            _ => None,
        }
    }
}

impl std::str::FromStr for Os {
    type Err = String;
    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s {
            "windows" => Ok(Os::Windows),
            "linux" => Ok(Os::Linux),
            "mac" => Ok(Os::Mac),
            _ => Err(format!("unknown OS `{s}` (must be windows, linux or mac)")),
        }
    }
}

/// Compression algorithm (Section 8): `none` or `zstd`. RAR and any
/// proprietary format are forbidden.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Compression {
    None,
    Zstd,
}

impl Compression {
    pub fn as_str(&self) -> &'static str {
        match self {
            Compression::None => "none",
            Compression::Zstd => "zstd",
        }
    }

    /// Valid compression level range for this algorithm.
    pub fn valid_level(&self, level: u32) -> bool {
        match self {
            Compression::None => level == 0,
            Compression::Zstd => (1..=22).contains(&level),
        }
    }
}

impl std::str::FromStr for Compression {
    type Err = String;
    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s {
            "none" => Ok(Compression::None),
            "zstd" => Ok(Compression::Zstd),
            _ => Err(format!("unknown compression algorithm `{s}` (allowed: none, zstd)")),
        }
    }
}

/// Compression kind (Section 7.5): each file compressed separately, or all
/// files compressed together as a single archive stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompressionKind {
    PerFile,
    WholeArchive,
}

impl CompressionKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            CompressionKind::PerFile => "per-file",
            CompressionKind::WholeArchive => "whole-archive",
        }
    }
}

impl std::str::FromStr for CompressionKind {
    type Err = String;
    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s {
            "per-file" => Ok(CompressionKind::PerFile),
            "whole-archive" => Ok(CompressionKind::WholeArchive),
            _ => Err(format!("unknown compression kind `{s}` (allowed: per-file, whole-archive)")),
        }
    }
}

/// Package type (revision 28, Section 13). Unknown types fall back to `misc`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackageType {
    Application,
    Game,
    Album,
    MusicAlbum,
    Pictures,
    Documents,
    Data,
    Database,
    Misc,
}

impl PackageType {
    pub fn as_str(&self) -> &'static str {
        match self {
            PackageType::Application => "application",
            PackageType::Game => "game",
            PackageType::Album => "album",
            PackageType::MusicAlbum => "music album",
            PackageType::Pictures => "pictures",
            PackageType::Documents => "documents",
            PackageType::Data => "data",
            PackageType::Database => "database",
            PackageType::Misc => "misc",
        }
    }

    /// Default install folder for this type (Section 13).
    pub fn default_folder(&self) -> &'static str {
        match self {
            PackageType::Application => "apps",
            PackageType::Game => "games",
            PackageType::Album | PackageType::MusicAlbum => "music",
            PackageType::Pictures => "pictures",
            PackageType::Documents => "documents",
            PackageType::Data => "data",
            PackageType::Database => "databases",
            PackageType::Misc => "misc",
        }
    }
}

impl std::str::FromStr for PackageType {
    type Err = String;
    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s.trim() {
            "" | "application" => Ok(PackageType::Application),
            "game" => Ok(PackageType::Game),
            "album" => Ok(PackageType::Album),
            "music album" => Ok(PackageType::MusicAlbum),
            "pictures" => Ok(PackageType::Pictures),
            "documents" => Ok(PackageType::Documents),
            "data" => Ok(PackageType::Data),
            "database" => Ok(PackageType::Database),
            "misc" => Ok(PackageType::Misc),
            other => Err(format!(
                "unknown package type `{other}` (allowed: application, game, album, music album, \
                 pictures, documents, data, database, misc)"
            )),
        }
    }
}

/// Target architecture (revision 28): `32`, `64`, `arm64`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Arch {
    X32,
    X64,
    Arm64,
}

impl Arch {
    pub fn as_str(&self) -> &'static str {
        match self {
            Arch::X32 => "32",
            Arch::X64 => "64",
            Arch::Arm64 => "arm64",
        }
    }
}

impl std::str::FromStr for Arch {
    type Err = String;
    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s.trim() {
            "32" => Ok(Arch::X32),
            "64" => Ok(Arch::X64),
            "arm64" => Ok(Arch::Arm64),
            _ => Err(format!("unknown architecture `{s}` (allowed: 32, 64, arm64)")),
        }
    }
}

/// The fixed metadata block at the start of the package (Section 7.2).
#[derive(Debug, Clone)]
pub struct Header {
    /// Version of the UPKG format itself.
    pub format_version: u32,
    /// Target OS.
    pub os: Os,
    /// Target OS version.
    pub os_version: String,
    /// Inclusive lower bound (optional).
    pub min: Option<String>,
    /// Inclusive upper bound (optional).
    pub max: Option<String>,
    /// Compression algorithm.
    pub compression: Compression,
    /// Compression kind.
    pub compression_kind: CompressionKind,
    /// Compression level (0 when algorithm is `none`).
    pub compression_level: u32,
    /// Byte offset where the entries tree begins.
    pub tree_start: u64,
    /// Byte offset where the entries tree ends.
    pub tree_end: u64,
    /// When true, install rejects on OS-version/distro/arch mismatch.
    pub strict: bool,
    /// Target Linux distribution (optional).
    pub distro: Option<String>,
    /// Byte offset where the metadata file begins.
    pub metadata_start: u64,
    /// Byte offset where the metadata file ends.
    pub metadata_end: u64,
    /// Package type (defaults to `application`).
    pub package_type: PackageType,
    /// Target architecture (optional).
    pub arch: Option<Arch>,
}

impl Header {
    /// Size of the fixed binary prefix before the string fields.
    pub const FIXED_SIZE: usize = 45;

    /// Serialize the header to bytes (see module docs for the layout).
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(512);
        out.extend_from_slice(MAGIC);
        put_u32(&mut out, self.format_version);
        put_u32(&mut out, self.compression_level);
        out.push(self.strict as u8);
        put_u64(&mut out, self.tree_start);
        put_u64(&mut out, self.tree_end);
        put_u64(&mut out, self.metadata_start);
        put_u64(&mut out, self.metadata_end);
        put_nul_string(&mut out, self.os.as_str());
        put_nul_string(&mut out, &self.os_version);
        put_nul_string(&mut out, self.min.as_deref().unwrap_or(""));
        put_nul_string(&mut out, self.max.as_deref().unwrap_or(""));
        put_nul_string(&mut out, self.compression.as_str());
        put_nul_string(&mut out, self.compression_kind.as_str());
        put_nul_string(&mut out, self.distro.as_deref().unwrap_or(""));
        put_nul_string(&mut out, self.package_type.as_str());
        put_nul_string(&mut out, self.arch.map(|a| a.as_str()).unwrap_or(""));
        out
    }

    /// Parse a header from the start of `buf`.
    ///
    /// Returns the header and its exact byte length (so the header SHA-1 can
    /// be checked over the stored bytes).
    pub fn parse(buf: &[u8]) -> Result<(Header, usize)> {
        if buf.len() < Self::FIXED_SIZE {
            return Err(UpkgError::Format("header too short".into()));
        }
        if &buf[0..4] != MAGIC {
            return Err(UpkgError::Format("not a UPKG package (bad magic)".into()));
        }
        let format_version = get_u32(buf, 4)?;
        if format_version != FORMAT_VERSION {
            return Err(UpkgError::Format(format!(
                "unsupported format version {format_version} (expected {FORMAT_VERSION})"
            )));
        }
        let compression_level = get_u32(buf, 8)?;
        let strict = match buf[12] {
            0x00 => false,
            0x01 => true,
            other => {
                return Err(UpkgError::Format(format!(
                    "invalid boolean value 0x{other:02x} in header"
                )))
            }
        };
        let tree_start = get_u64(buf, 13)?;
        let tree_end = get_u64(buf, 21)?;
        let metadata_start = get_u64(buf, 29)?;
        let metadata_end = get_u64(buf, 37)?;

        let mut pos = Self::FIXED_SIZE;
        let next_string = |buf: &[u8], pos: &mut usize| -> Result<String> {
            let (s, next) = get_nul_string(buf, *pos)?;
            *pos = next;
            Ok(s)
        };
        let os_str = next_string(buf, &mut pos)?;
        let os_version = next_string(buf, &mut pos)?;
        let min = next_string(buf, &mut pos)?;
        let max = next_string(buf, &mut pos)?;
        let compression_str = next_string(buf, &mut pos)?;
        let compression_kind_str = next_string(buf, &mut pos)?;
        let distro = next_string(buf, &mut pos)?;
        let package_type_str = next_string(buf, &mut pos)?;
        let arch_str = next_string(buf, &mut pos)?;

        let os: Os = os_str.parse().map_err(UpkgError::Format)?;
        let compression: Compression = compression_str.parse().map_err(UpkgError::Format)?;
        let compression_kind: CompressionKind =
            compression_kind_str.parse().map_err(UpkgError::Format)?;
        // Unknown types fall back to `misc` (proposal).
        let package_type = package_type_str
            .parse::<PackageType>()
            .unwrap_or(PackageType::Misc);
        let arch = if arch_str.is_empty() {
            None
        } else {
            Some(arch_str.parse().map_err(UpkgError::Format)?)
        };

        if !compression.valid_level(compression_level) {
            return Err(UpkgError::Format(format!(
                "invalid compression level {compression_level} for algorithm `{}`",
                compression.as_str()
            )));
        }

        let header = Header {
            format_version,
            os,
            os_version,
            min: nonempty(min),
            max: nonempty(max),
            compression,
            compression_kind,
            compression_level,
            tree_start,
            tree_end,
            strict,
            distro: nonempty(distro),
            metadata_start,
            metadata_end,
            package_type,
            arch,
        };
        Ok((header, pos))
    }

    /// Structural sanity check of the section offsets.
    pub fn check_layout(&self, header_len: usize) -> Result<()> {
        let hash_block_len = 80usize;
        let hash_block_end = header_len + hash_block_len;
        if self.metadata_start != hash_block_end as u64 {
            return Err(UpkgError::Format(
                "metadata file does not start right after the hash block".into(),
            ));
        }
        if self.metadata_end < self.metadata_start {
            return Err(UpkgError::Format("metadata file end before its start".into()));
        }
        if self.tree_start != self.metadata_end {
            return Err(UpkgError::Format(
                "entries tree does not start right after the metadata file".into(),
            ));
        }
        if self.tree_end < self.tree_start {
            return Err(UpkgError::Format("entries tree end before its start".into()));
        }
        Ok(())
    }
}

fn nonempty(s: String) -> Option<String> {
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Header {
        Header {
            format_version: FORMAT_VERSION,
            os: Os::Linux,
            os_version: "22.04".into(),
            min: Some("20.04".into()),
            max: Some("24.04".into()),
            compression: Compression::Zstd,
            compression_kind: CompressionKind::PerFile,
            compression_level: 3,
            tree_start: 1000,
            tree_end: 1200,
            strict: true,
            distro: Some("ubuntu".into()),
            metadata_start: 900,
            metadata_end: 1000,
            package_type: PackageType::Game,
            arch: Some(Arch::X64),
        }
    }

    #[test]
    fn round_trip() {
        let h = sample();
        let bytes = h.encode();
        let (parsed, len) = Header::parse(&bytes).unwrap();
        assert_eq!(len, bytes.len());
        assert_eq!(parsed.format_version, h.format_version);
        assert_eq!(parsed.os, h.os);
        assert_eq!(parsed.os_version, "22.04");
        assert_eq!(parsed.min.as_deref(), Some("20.04"));
        assert_eq!(parsed.max.as_deref(), Some("24.04"));
        assert_eq!(parsed.compression, Compression::Zstd);
        assert_eq!(parsed.compression_kind, CompressionKind::PerFile);
        assert_eq!(parsed.compression_level, 3);
        assert!(parsed.strict);
        assert_eq!(parsed.distro.as_deref(), Some("ubuntu"));
        assert_eq!(parsed.tree_start, 1000);
        assert_eq!(parsed.tree_end, 1200);
        assert_eq!(parsed.metadata_start, 900);
        assert_eq!(parsed.metadata_end, 1000);
        assert_eq!(parsed.package_type, PackageType::Game);
        assert_eq!(parsed.arch, Some(Arch::X64));
    }

    #[test]
    fn missing_optionals() {
        let mut h = sample();
        h.min = None;
        h.max = None;
        h.distro = None;
        h.arch = None;
        let bytes = h.encode();
        let (parsed, _) = Header::parse(&bytes).unwrap();
        assert_eq!(parsed.min, None);
        assert_eq!(parsed.max, None);
        assert_eq!(parsed.distro, None);
        assert_eq!(parsed.arch, None);
    }

    #[test]
    fn rejects_bad_magic() {
        assert!(Header::parse(b"XXXXrest").is_err());
    }
}
