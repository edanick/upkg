//! `upkg create` (Sections 5, 6, 7 of the spec).
//!
//! Builds a `.upkg` package from a config file. The pipeline:
//! 1. warn about missing filename components (Section 5);
//! 2. validate path safety of every source path (constraint 13);
//! 3. compress files per kind, laying out the data section;
//! 4. serialize the metadata file and entries tree;
//! 5. assemble header + hashes + metadata + tree + data (+ optional ed25519
//!    signature over all preceding bytes, Section 7.7).

use std::io::Read;
use std::path::{Path, PathBuf};

use crate::compress;
use crate::config::CreateConfig;
use crate::entries::{self, Entry, EntryAttributes, EntryKind, TreeFormat};
use crate::error::{Result, UpkgError};
use crate::header::{CompressionKind, Header, MAGIC};
use crate::metadata::{self, Metadata};
use crate::signature;
use crate::util::sha1;
use sha1::Digest;

/// Build the package and return the path of the written file.
pub fn create(config: &CreateConfig) -> Result<PathBuf> {
    warn_missing_components(config);

    // --- walk the source folder ---
    let mut roots = Vec::new();
    walk(&config.source, "", &mut roots, config.modes, config.attributes)?;
    if roots.is_empty() {
        return Err(UpkgError::Config(format!(
            "source folder `{}` is empty",
            config.source.display()
        )));
    }
    // --- validate path safety before any work (constraint 13) ---
    entries::validate_tree_paths(&roots, config.os)?;
    if config.os == crate::header::Os::Windows {
        entries::check_case_collisions(&roots)?;
    }
    validate_text(&config.app_name, "app-name")?;
    validate_text(&config.app_version, "app-version")?;
    validate_text(&config.os_version, "os-version")?;
    if let Some(v) = &config.min {
        validate_text(v, "min")?;
    }
    if let Some(v) = &config.max {
        validate_text(v, "max")?;
    }
    if let Some(v) = &config.distro {
        validate_text(v, "distro")?;
    }

    let file_paths: Vec<String> = roots
        .iter()
        .flat_map(|e| e.files())
        .map(|e| e.relative_path.clone())
        .collect();

    // --- compress the files ---
    let (data_section, master_sha1, blob_lens) = match config.compression_kind {
        CompressionKind::PerFile => {
            let mut blobs: Vec<Vec<u8>> = Vec::with_capacity(file_paths.len());
            let mut master = sha1::Sha1::new();
            for path in &file_paths {
                let bytes = read_file_bytes(&config.source, path)?;
                master.update(&bytes);
                let compressed =
                    compress::compress(config.compression, config.compression_level, &bytes)?;
                let post = sha1(&compressed);
                attach_file_meta(&mut roots, path, post)?;
                blobs.push(compressed);
            }
            let lens: Vec<u64> = blobs.iter().map(|b| b.len() as u64).collect();
            let mut data = Vec::new();
            for blob in &blobs {
                data.extend_from_slice(blob);
            }
            let master_sha1: [u8; 20] = master.finalize().into();
            (data, master_sha1, lens)
        }
        CompressionKind::WholeArchive => {
            let mut raw_all = Vec::new();
            for path in &file_paths {
                let bytes = read_file_bytes(&config.source, path)?;
                raw_all.extend_from_slice(&bytes);
            }
            let compressed =
                compress::compress(config.compression, config.compression_level, &raw_all)?;
            let master_sha1 = sha1(&compressed);
            (compressed, master_sha1, Vec::new())
        }
    };

    // --- metadata file bytes ---
    let mut metadata = Metadata::default();
    metadata.app_name = config.app_name.clone();
    metadata.app_version = config.app_version.clone();
    metadata.os = config.os;
    metadata.os_version = config.os_version.clone();
    metadata.min = config.min.clone();
    metadata.max = config.max.clone();
    metadata.distro = config.distro.clone();
    metadata.dependencies = config.dependencies.clone();
    metadata.requirements = config.requirements.clone();
    metadata.strict = config.strict;
    metadata.attributes = config.attributes;
    metadata.modes = config.modes;
    metadata.conflicts = config.conflicts.clone();
    metadata.replaces = config.replaces.clone();
    metadata.signing = config.signing.is_some();
    metadata.package_type = config.package_type;
    metadata.arch = config.arch;
    metadata.description = config.description.clone();
    metadata.homepage = config.homepage.clone();
    metadata.author = config.author.clone();
    metadata.license = config.license.clone();
    metadata.shortcut = config.shortcut.clone();
    let metadata_bytes = metadata::serialize(&metadata).into_bytes();

    // --- assemble offsets (fixed point over the tree length) ---
    let mut header = Header {
        format_version: crate::header::FORMAT_VERSION,
        os: config.os,
        os_version: config.os_version.clone(),
        min: config.min.clone(),
        max: config.max.clone(),
        compression: config.compression,
        compression_kind: config.compression_kind,
        compression_level: config.compression_level,
        tree_start: 0,
        tree_end: 0,
        strict: config.strict,
        distro: config.distro.clone(),
        metadata_start: 0,
        metadata_end: 0,
        package_type: config.package_type,
        arch: config.arch,
    };

    let tree_format = TreeFormat {
        has_offsets: config.compression_kind == CompressionKind::PerFile,
        has_mode: config.os != crate::header::Os::Windows && config.modes,
        has_attributes: config.os == crate::header::Os::Windows && config.attributes,
    };

    let header_len = header.encode().len() as u64;
    header.metadata_start = header_len + crate::hashes::HASH_BLOCK_LEN as u64;
    header.metadata_end = header.metadata_start + metadata_bytes.len() as u64;
    header.tree_start = header.metadata_end;

    // Per-file data offsets are absolute and depend on the tree length, which
    // in turn depends on the offsets' decimal widths; iterate until stable.
    let mut tree_len = 0u64;
    for _ in 0..16 {
        if config.compression_kind == CompressionKind::PerFile {
            let mut base = header.tree_start + tree_len;
            for (path, len) in file_paths.iter().zip(&blob_lens) {
                set_entry_offsets(&mut roots, path, base, base + len)?;
                base += len;
            }
        }
        let bytes = entries::serialize(&roots, &tree_format);
        let new_len = bytes.len() as u64;
        if new_len == tree_len {
            tree_len = new_len;
            break;
        }
        tree_len = new_len;
    }
    header.tree_end = header.tree_start + tree_len;
    let tree_bytes = entries::serialize(&roots, &tree_format);
    if header.tree_end != header.tree_start + tree_bytes.len() as u64 {
        return Err(UpkgError::Format("entries tree size did not converge".into()));
    }

    // --- hashes (Section 7.3) ---
    let header_bytes = header.encode();
    let hash_block = crate::hashes::HashBlock {
        header_sha1: sha1(&header_bytes),
        master_sha1,
        tree_sha1: sha1(&tree_bytes),
        metadata_sha1: sha1(&metadata_bytes),
    };

    // --- write the file ---
    let output_path = output_path(config)?;
    let mut out = Vec::with_capacity(
        header_bytes.len()
            + crate::hashes::HASH_BLOCK_LEN
            + metadata_bytes.len()
            + tree_bytes.len()
            + data_section.len()
            + signature::SIGNATURE_SECTION_LEN,
    );
    out.extend_from_slice(&header_bytes);
    out.extend_from_slice(&hash_block.encode());
    out.extend_from_slice(&metadata_bytes);
    out.extend_from_slice(&tree_bytes);
    out.extend_from_slice(&data_section);

    if let Some(key_path) = &config.signing {
        let seed = signature::load_seed(key_path)?;
        let section = signature::sign(&out, &seed);
        out.extend_from_slice(&signature::encode_section(&section));
        println!("signed with ed25519 key from `{}`", key_path.display());
    }

    if let Some(parent) = output_path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .map_err(|e| UpkgError::io_context(e, "cannot create output directory"))?;
        }
    }
    std::fs::write(&output_path, &out)
        .map_err(|e| UpkgError::io_context(e, "cannot write package"))?;

    println!(
        "created `{}` ({} files, {} bytes)",
        output_path.display(),
        file_paths.len(),
        out.len()
    );
    Ok(output_path)
}

/// Warn about filename-convention components the user did not provide
/// (Section 5: "warning: version number was not provided", etc.).
fn warn_missing_components(config: &CreateConfig) {
    if config.app_version.is_empty() {
        eprintln!("warning: version number was not provided");
    }
    if config.os_version.is_empty() {
        eprintln!("warning: OS version was not provided");
    }
    if config.arch.is_none() {
        eprintln!("warning: architecture was not provided");
    }
}

fn validate_text(value: &str, field: &str) -> Result<()> {
    if value.contains('\0') || value.contains('\n') || value.contains('\r') {
        return Err(UpkgError::Config(format!(
            "field `{field}` must not contain NUL or line-breaker bytes"
        )));
    }
    Ok(())
}

/// Recursively walk the source folder building entries. Every entry's
/// `relative path` is relative to the package root (Section 7.4): the
/// `prefix` accumulates the enclosing folder names.
fn walk(
    dir: &Path,
    prefix: &str,
    out: &mut Vec<Entry>,
    modes: bool,
    attributes: bool,
) -> Result<()> {
    let mut children: Vec<(PathBuf, bool)> = Vec::new(); // (path, is_dir)
    for item in std::fs::read_dir(dir)
        .map_err(|e| UpkgError::io_context(e, &format!("cannot read `{}`", dir.display())))?
    {
        let item = item.map_err(UpkgError::Io)?;
        let path = item.path();
        let is_dir = path.is_dir();
        children.push((path, is_dir));
    }
    children.sort_by(|a, b| a.0.file_name().cmp(&b.0.file_name()));

    for (path, is_dir) in children {
        let filename = path
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .ok_or_else(|| UpkgError::Config("invalid file name".into()))?;
        let relative = if prefix.is_empty() {
            filename.clone()
        } else {
            format!("{prefix}/{filename}")
        };
        if is_dir {
            let mut entry = Entry::new_folder(&relative, &filename);
            walk(&path, &relative, &mut entry.children, modes, attributes)?;
            out.push(entry);
        } else {
            let bytes = std::fs::read(&path)
                .map_err(|e| UpkgError::io_context(e, &format!("cannot read `{}`", path.display())))?;
            let mut entry = Entry::new_file(&relative, &filename, sha1(&bytes), bytes.len() as u64);
            if modes {
                entry.mode = Some(file_mode(&path));
            }
            if attributes {
                entry.attributes = Some(entry_attributes(&path));
            }
            out.push(entry);
        }
    }
    Ok(())
}

/// Read the raw bytes of a file entry from the source folder.
fn read_file_bytes(source: &Path, relative_path: &str) -> Result<Vec<u8>> {
    let mut file = std::fs::File::open(source.join(relative_path))
        .map_err(|e| UpkgError::io_context(e, "cannot open source file"))?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    Ok(bytes)
}

/// Unix permission bits of a file (best effort).
#[cfg(unix)]
fn file_mode(path: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .map(|m| m.permissions().mode() & 0o7777)
        .unwrap_or(0o644)
}

#[cfg(not(unix))]
fn file_mode(_path: &Path) -> u32 {
    0o644
}

/// Windows attributes of a file (best effort: archive for all, readonly
/// from the filesystem).
fn entry_attributes(path: &Path) -> EntryAttributes {
    let readonly = std::fs::metadata(path)
        .map(|m| m.permissions().readonly())
        .unwrap_or(false);
    EntryAttributes {
        readonly,
        hidden: false,
        archive: true,
        system: false,
    }
}

/// Attach the post-compression hash to a file entry (per-file kind).
fn attach_file_meta(roots: &mut [Entry], relative_path: &str, post: [u8; 20]) -> Result<()> {
    let entry = find_file_mut(roots, relative_path)?;
    entry.post_compression_sha1 = Some(post);
    Ok(())
}

/// Set the data offsets of a file entry (per-file kind).
fn set_entry_offsets(roots: &mut [Entry], relative_path: &str, start: u64, end: u64) -> Result<()> {
    let entry = find_file_mut(roots, relative_path)?;
    entry.data_start = Some(start);
    entry.data_end = Some(end);
    Ok(())
}

fn find_file_mut<'a>(roots: &'a mut [Entry], relative_path: &str) -> Result<&'a mut Entry> {
    for root in roots.iter_mut() {
        if let Some(e) = find_in(root, relative_path) {
            return Ok(e);
        }
    }
    Err(UpkgError::Format(format!(
        "internal: file entry `{relative_path}` not found"
    )))
}

fn find_in<'a>(entry: &'a mut Entry, relative_path: &str) -> Option<&'a mut Entry> {
    if entry.relative_path == relative_path {
        return Some(entry);
    }
    for child in entry.children.iter_mut() {
        if let Some(e) = find_in(child, relative_path) {
            return Some(e);
        }
    }
    None
}

/// Compute the output file path following the naming convention
/// `<app-name>-<os>-<arch>.upkg` (recommendation, Section 5).
fn output_path(config: &CreateConfig) -> Result<PathBuf> {
    let os_abbr = match config.os {
        crate::header::Os::Windows => "win",
        crate::header::Os::Linux => "linux",
        crate::header::Os::Mac => "mac",
    };
    let name = sanitize_filename(&config.app_name);
    let mut parts = vec![name];
    parts.push(os_abbr.to_string());
    if let Some(arch) = &config.arch {
        parts.push(arch.as_str().to_string());
    }
    let file_name = format!("{}.upkg", parts.join("-"));
    match &config.output {
        Some(dir) => Ok(dir.join(file_name)),
        None => Ok(PathBuf::from(file_name)),
    }
}

/// Replace filename-hostile characters for the convention name.
fn sanitize_filename(name: &str) -> String {
    let mut out = String::new();
    for c in name.chars() {
        let ok =
            !c.is_control() && !matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|');
        out.push(if ok { c } else { '-' });
    }
    if out.is_empty() {
        out.push_str("app");
    }
    out
}

/// Keep `EntryKind` and `MAGIC` referenced (documentation of the layout).
#[allow(dead_code)]
const _REF: (EntryKind, &[u8; 4]) = (EntryKind::Folder, MAGIC);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize() {
        assert_eq!(sanitize_filename("My App"), "My App");
        assert_eq!(sanitize_filename("a/b:c"), "a-b-c");
        assert_eq!(sanitize_filename(""), "app");
    }
}
