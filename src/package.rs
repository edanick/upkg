//! Reading and structurally validating a `.upkg` file (Sections 7, 10.1).
//!
//! `open` parses the header, verifies the header/metadata/tree SHA-1s,
//! extracts an optional ed25519 signature (Section 7.7) and parses the
//! metadata file and entries tree. Per-file data access and whole-archive
//! extraction helpers live here too.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use crate::compress;
use crate::entries::{self, Entry, TreeFormat};
use crate::error::{Result, UpkgError};
use crate::hashes::{self, HashBlock};
use crate::header::Header;
use crate::metadata::{self, Metadata};
use crate::signature::{self, SignatureSection};
use crate::util::{sha1, to_hex};

/// Size of the initial read window: enough for header + hashes + metadata +
/// a small entries tree.
const PREAMBLE_WINDOW: u64 = 64 * 1024;

/// An opened package.
pub struct Package {
    pub path: PathBuf,
    pub file: File,
    pub header: Header,
    pub hashes: HashBlock,
    pub metadata: Metadata,
    pub tree: Vec<Entry>,
    /// Signature section when the package carries one.
    pub signature: Option<SignatureSection>,
}

impl Package {
    /// Verify the optional signature over all preceding bytes.
    ///
    /// Reads the whole file into memory (the signature covers every byte).
    pub fn verify_signature(&self) -> Result<()> {
        let Some(sig) = &self.signature else {
            return Ok(());
        };
        let mut bytes = Vec::new();
        std::fs::File::open(&self.path)?
            .read_to_end(&mut bytes)
            .map_err(|e| UpkgError::io_context(e, "cannot read package for signature check"))?;
        let prefix = &bytes[..bytes.len() - signature::SIGNATURE_SECTION_LEN];
        signature::verify(prefix, sig)
    }

    /// Read the stored (possibly compressed) bytes of a per-file entry.
    pub fn read_stored_bytes(&mut self, entry: &Entry) -> Result<Vec<u8>> {
        let start = entry
            .data_start
            .ok_or_else(|| UpkgError::Format("entry has no data start offset".into()))?;
        let end = entry
            .data_end
            .ok_or_else(|| UpkgError::Format("entry has no data end offset".into()))?;
        let len = (end - start) as usize;
        self.file.seek(SeekFrom::Start(start))?;
        let mut buf = vec![0u8; len];
        self.file.read_exact(&mut buf)?;
        Ok(buf)
    }

    /// Read, verify and decompress a per-file entry; returns raw contents.
    pub fn read_entry_raw(&mut self, entry: &Entry) -> Result<Vec<u8>> {
        if entry.kind != entries::EntryKind::File {
            return Err(UpkgError::Format("not a file entry".into()));
        }
        let stored = self.read_stored_bytes(entry)?;
        if let Some(post) = entry.post_compression_sha1 {
            hashes::check("post-compression SHA-1", &post, &sha1(&stored))?;
        }
        let raw = compress::decompress(self.header.compression, &stored)?;
        if let Some(orig) = entry.original_sha1 {
            hashes::check("original SHA-1", &orig, &sha1(&raw))?;
        }
        Ok(raw)
    }

    /// File entries in depth-first order (the order the data section uses).
    pub fn file_entries(&self) -> Vec<&Entry> {
        self.tree.iter().flat_map(|e| e.files()).collect()
    }

    /// Owned copies of the file entries (for loops that also need `&mut self`).
    pub fn file_entries_owned(&self) -> Vec<Entry> {
        self.file_entries().into_iter().cloned().collect()
    }
}

/// The parsed prefix of a package: header + hashes + metadata + tree.
pub struct Prefix {
    pub header: Header,
    pub hashes: HashBlock,
    pub metadata: Metadata,
    pub tree: Vec<Entry>,
}

/// Parse and validate a package prefix from an in-memory buffer (used by
/// streaming install, which fetches the prefix over HTTP first).
pub fn parse_prefix(buf: &[u8]) -> Result<Prefix> {
    // --- header ---
    let (header, header_len) = Header::parse(buf)?;
    header.check_layout(header_len)?;

    // --- hash block ---
    let hashes = HashBlock::parse(buf, header_len)?;
    hashes::check("header SHA-1", &hashes.header_sha1, &sha1(&buf[..header_len]))?;

    // --- metadata file ---
    let metadata_start = header.metadata_start as usize;
    let metadata_end = header.metadata_end as usize;
    let metadata_range = buf
        .get(metadata_start..metadata_end)
        .ok_or_else(|| UpkgError::Format("metadata file range missing".into()))?;
    hashes::check("metadata SHA-1", &hashes.metadata_sha1, &sha1(metadata_range))?;
    let metadata = metadata::parse(metadata_range)?;

    // --- entries tree ---
    let tree_start = header.tree_start as usize;
    let tree_end = header.tree_end as usize;
    let tree_bytes = buf
        .get(tree_start..tree_end)
        .ok_or_else(|| UpkgError::Format("entries tree range missing".into()))?;
    hashes::check("tree SHA-1", &hashes.tree_sha1, &sha1(tree_bytes))?;

    let tree_format = TreeFormat {
        has_offsets: header.compression_kind == crate::header::CompressionKind::PerFile,
        has_mode: header.os != crate::header::Os::Windows && metadata.modes,
        has_attributes: header.os == crate::header::Os::Windows && metadata.attributes,
    };
    let tree = entries::parse(tree_bytes, &tree_format)?;

    Ok(Prefix {
        header,
        hashes,
        metadata,
        tree,
    })
}

/// Open and validate a package file.
pub fn open(path: &Path) -> Result<Package> {
    let mut file = File::open(path)
        .map_err(|e| UpkgError::io_context(e, &format!("cannot open `{}`", path.display())))?;

    // --- read the preamble window ---
    file.seek(SeekFrom::Start(0))?;
    let mut window = vec![0u8; PREAMBLE_WINDOW as usize];
    let mut read = 0usize;
    loop {
        let n = file.read(&mut window[read..])?;
        if n == 0 {
            break;
        }
        read += n;
        if read == window.len() {
            break;
        }
    }
    window.truncate(read);

    let prefix = parse_prefix(&window)?;

    // --- signature ---
    let signature = read_signature(&mut file)?;

    Ok(Package {
        path: path.to_path_buf(),
        file,
        header: prefix.header,
        hashes: prefix.hashes,
        metadata: prefix.metadata,
        tree: prefix.tree,
        signature,
    })
}

/// Read the optional signature section from the end of the file.
fn read_signature(file: &mut File) -> Result<Option<SignatureSection>> {
    let len = file.seek(SeekFrom::End(0))?;
    if len < signature::SIGNATURE_SECTION_LEN as u64 {
        return Ok(None);
    }
    file.seek(SeekFrom::Start(len - signature::SIGNATURE_SECTION_LEN as u64))?;
    let mut tail = vec![0u8; signature::SIGNATURE_SECTION_LEN];
    file.read_exact(&mut tail)?;
    Ok(signature::extract_section(&tail))
}

/// Read a whole-archive package's entire compressed archive stream.
fn read_archive_data(file: &mut File, header: &Header) -> Result<Vec<u8>> {
    let start = header.tree_end;
    let end = file.seek(SeekFrom::End(0))?;
    // The signature section (if any) is excluded from the data.
    let sig_len = if end >= signature::SIGNATURE_SECTION_LEN as u64 {
        // Determine by looking at the tail.
        file.seek(SeekFrom::Start(end - signature::SIGNATURE_SECTION_LEN as u64))?;
        let mut tail = vec![0u8; signature::SIGNATURE_SECTION_LEN];
        file.read_exact(&mut tail)?;
        if signature::extract_section(&tail).is_some() {
            signature::SIGNATURE_SECTION_LEN as u64
        } else {
            0
        }
    } else {
        0
    };
    let end = end - sig_len;
    if end < start {
        return Err(UpkgError::Format("archive data end before tree end".into()));
    }
    file.seek(SeekFrom::Start(start))?;
    let mut buf = vec![0u8; (end - start) as usize];
    file.read_exact(&mut buf)?;
    Ok(buf)
}

/// Verify the master SHA-1 of a whole-archive package over its stored bytes.
pub fn verify_whole_archive_master(pkg: &mut Package) -> Result<()> {
    let archive = read_archive_data(&mut pkg.file, &pkg.header)?;
    hashes::check(
        "master SHA-1",
        &pkg.hashes.master_sha1,
        &sha1(&archive),
    )
}

/// Decompress a whole-archive package and return every file's raw bytes in
/// tree order (used by verify/repair/install; the archive must be
/// decompressed in full - the acknowledged trade-off of Section 10.3).
pub fn whole_archive_extract(pkg: &mut Package) -> Result<Vec<(String, Vec<u8>)>> {
    let archive = read_archive_data(&mut pkg.file, &pkg.header)?;
    hashes::check(
        "master SHA-1",
        &pkg.hashes.master_sha1,
        &sha1(&archive),
    )?;
    let mut raw = compress::decompress(pkg.header.compression, &archive)?;
    let mut out = Vec::new();
    for entry in pkg.file_entries() {
        let size = entry.size.ok_or_else(|| UpkgError::Format("file has no size".into()))? as usize;
        if raw.len() < size {
            return Err(UpkgError::Format(
                "archive is shorter than the sum of its files".into(),
            ));
        }
        let bytes: Vec<u8> = raw.drain(..size).collect();
        if let Some(orig) = entry.original_sha1 {
            hashes::check("original SHA-1", &orig, &sha1(&bytes))?;
        }
        out.push((entry.relative_path.clone(), bytes));
    }
    Ok(out)
}

/// Format a human-readable summary of a package (used by `upkg info`).
pub fn describe(pkg: &Package) -> String {
    let mut out = String::new();
    let h = &pkg.header;
    out.push_str(&format!("app-name:       {}\n", pkg.metadata.app_name));
    out.push_str(&format!("app-version:    {}\n", pkg.metadata.app_version));
    out.push_str(&format!("os:             {}\n", h.os.as_str()));
    out.push_str(&format!("os-version:     {}\n", h.os_version));
    if let Some(min) = &h.min {
        out.push_str(&format!("min:            {min}\n"));
    }
    if let Some(max) = &h.max {
        out.push_str(&format!("max:            {max}\n"));
    }
    if let Some(d) = &h.distro {
        out.push_str(&format!("distro:         {d}\n"));
    }
    out.push_str(&format!(
        "compression:    {} (kind {}, level {})\n",
        h.compression.as_str(),
        h.compression_kind.as_str(),
        h.compression_level
    ));
    out.push_str(&format!("strict:         {}\n", h.strict));
    out.push_str(&format!("type:           {}\n", h.package_type.as_str()));
    if let Some(arch) = &h.arch {
        out.push_str(&format!("arch:           {}\n", arch.as_str()));
    }
    out.push_str(&format!("format-version: {}\n", h.format_version));
    if let Some(distro) = &pkg.metadata.distro {
        out.push_str(&format!("metadata-distro:{distro}\n"));
    }
    if !pkg.metadata.dependencies.is_empty() {
        let deps: Vec<String> = pkg
            .metadata
            .dependencies
            .iter()
            .map(|d| d.to_dpkg())
            .collect();
        out.push_str(&format!("dependencies:   {}\n", deps.join(", ")));
    }
    if !pkg.metadata.requirements.is_empty() {
        out.push_str(&format!(
            "requirements:   {}\n",
            pkg.metadata.requirements.join(", ")
        ));
    }
    if !pkg.metadata.conflicts.is_empty() {
        out.push_str(&format!("conflicts:      {}\n", pkg.metadata.conflicts.join(", ")));
    }
    if !pkg.metadata.replaces.is_empty() {
        out.push_str(&format!("replaces:       {}\n", pkg.metadata.replaces.join(", ")));
    }
    if let Some(d) = &pkg.metadata.description {
        out.push_str(&format!("description:    {d}\n"));
    }
    if let Some(hp) = &pkg.metadata.homepage {
        out.push_str(&format!("homepage:       {hp}\n"));
    }
    if let Some(a) = &pkg.metadata.author {
        out.push_str(&format!("author:         {a}\n"));
    }
    if let Some(l) = &pkg.metadata.license {
        out.push_str(&format!("license:        {l}\n"));
    }
    if let Some(s) = &pkg.signature {
        out.push_str(&format!("signature:      present (key {})\n", to_hex(&s.public_key[..8])));
    } else {
        out.push_str("signature:      absent\n");
    }
    let files = pkg.file_entries();
    let total: u64 = files.iter().filter_map(|e| e.size).sum();
    out.push_str(&format!("files:          {} ({} bytes)\n", files.len(), total));
    if let Some(shortcut) = &pkg.metadata.shortcut {
        out.push_str(&format!("shortcut:       {} ({})\n", shortcut.kind_name(), shortcut_name(shortcut)));
    }
    out
}

fn shortcut_name(s: &crate::metadata::ShortcutTemplate) -> String {
    match s {
        crate::metadata::ShortcutTemplate::Universal { name, .. }
        | crate::metadata::ShortcutTemplate::Desktop { name, .. } => name.clone(),
        crate::metadata::ShortcutTemplate::Lnk { target, .. } => target.clone(),
    }
}
