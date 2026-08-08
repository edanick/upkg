//! Verification (Section 10 of the spec).
//!
//! - `verify_package_file`: the upkg file itself - header SHA-1, metadata
//!   SHA-1, tree SHA-1, master SHA-1 (per kind) and, for `per-file`
//!   packages, every entry's post-compression + original SHA-1; plus the
//!   ed25519 signature when present.
//! - `verify_folder`: a previously extracted folder against the package's
//!   per-file original SHA-1s (or against a database entry when no
//!   `--package` is given).

use std::collections::BTreeMap;
use std::io::Read;
use std::path::Path;

use sha1::Digest;

use crate::database::DatabaseEntry;
use crate::error::{Result, UpkgError};
use crate::hashes;
use crate::header::CompressionKind;
use crate::package::{self, Package};
use crate::util::to_hex;

/// Result of verifying a package file.
#[derive(Debug, Default)]
pub struct FileCheck {
    pub checked_entries: usize,
    pub whole_archive: bool,
}

/// Verify the integrity of a `.upkg` file itself (Section 10.1).
pub fn verify_package_file(path: &Path) -> Result<FileCheck> {
    let mut pkg = package::open(path)?;
    pkg.verify_signature()?;
    let mut check = FileCheck {
        checked_entries: 0,
        whole_archive: pkg.header.compression_kind == CompressionKind::WholeArchive,
    };

    match pkg.header.compression_kind {
        CompressionKind::WholeArchive => {
            // Archive level: the master SHA-1 over the stored archive data.
            package::verify_whole_archive_master(&mut pkg)?;
            // Reaching per-file data requires decompressing the whole archive
            // (same cost as repair); do it for a complete check.
            let extracted = package::whole_archive_extract(&mut pkg)?;
            check.checked_entries = extracted.len();
        }
        CompressionKind::PerFile => {
            // Per-entry level: stored bytes vs post-compression SHA-1, then
            // decompressed bytes vs original SHA-1; master over raw contents.
            let entries = pkg.file_entries_owned();
            let mut master = sha1::Sha1::new();
            for entry in &entries {
                let raw = pkg.read_entry_raw(entry)?;
                master.update(&raw);
                check.checked_entries += 1;
            }
            let computed: [u8; 20] = master.finalize().into();
            hashes::check("master SHA-1", &pkg.hashes.master_sha1, &computed)?;
        }
    }

    println!(
        "ok: `{}` - header, metadata, tree, master and {} file entr{} verified{}",
        path.display(),
        check.checked_entries,
        if check.checked_entries == 1 { "y" } else { "ies" },
        if check.whole_archive { " (whole-archive)" } else { "" }
    );
    Ok(check)
}

/// Report of a folder verification (Section 10.2).
#[derive(Debug, Default)]
pub struct FolderCheck {
    pub corrupt: Vec<String>,
    pub missing: Vec<String>,
    pub extra: Vec<String>,
    pub ok: usize,
}

/// Verify an extracted folder against a package's entries (Section 10.2).
pub fn verify_folder_against_package(folder: &Path, pkg: &Package) -> Result<FolderCheck> {
    let mut check = FolderCheck::default();
    let mut found: BTreeMap<String, String> = BTreeMap::new();
    walk_hashes(folder, folder, &mut found)?;

    for entry in pkg.tree.iter().flat_map(|e| e.files()) {
        match found.remove(&entry.relative_path) {
            None => check.missing.push(entry.relative_path.clone()),
            Some(actual) => {
                let expected = to_hex(&entry.original_sha1.unwrap_or_default());
                if actual == expected {
                    check.ok += 1;
                } else {
                    check.corrupt.push(entry.relative_path.clone());
                }
            }
        }
    }
    // Reporting extra files is a proposal (Section 10.2) - report info only.
    check.extra = found.keys().cloned().collect();
    Ok(check)
}

/// Verify a folder against a database entry (no `--package`).
pub fn verify_folder_against_entry(folder: &Path, entry: &DatabaseEntry) -> Result<FolderCheck> {
    let mut check = FolderCheck::default();
    let mut found: BTreeMap<String, String> = BTreeMap::new();
    walk_hashes(folder, folder, &mut found)?;

    for f in &entry.files {
        match found.remove(&f.relative_path) {
            None => check.missing.push(f.relative_path.clone()),
            Some(actual) => {
                if actual == f.original_sha1 {
                    check.ok += 1;
                } else {
                    check.corrupt.push(f.relative_path.clone());
                }
            }
        }
    }
    check.extra = found.keys().cloned().collect();
    Ok(check)
}

/// Recursively hash every file under `base`, keyed by relative path.
fn walk_hashes(
    base: &Path,
    dir: &Path,
    out: &mut BTreeMap<String, String>,
) -> Result<()> {
    let mut items: Vec<(std::path::PathBuf, bool)> = Vec::new();
    for item in std::fs::read_dir(dir)
        .map_err(|e| UpkgError::io_context(e, &format!("cannot read `{}`", dir.display())))?
    {
        let item = item.map_err(UpkgError::Io)?;
        let path = item.path();
        items.push((path.clone(), path.is_dir()));
    }
    items.sort_by(|a, b| a.0.cmp(&b.0));
    for (path, is_dir) in items {
        if is_dir {
            walk_hashes(base, &path, out)?;
        } else {
            let rel = path
                .strip_prefix(base)
                .map_err(|_| UpkgError::Format("path escape during folder walk".into()))?;
            let rel_str = rel
                .components()
                .map(|c| c.as_os_str().to_string_lossy().to_string())
                .collect::<Vec<_>>()
                .join("/");
            let mut file = std::fs::File::open(&path)
                .map_err(|e| UpkgError::io_context(e, "cannot open file for hashing"))?;
            let mut hasher = sha1::Sha1::new();
            let mut buf = [0u8; 64 * 1024];
            loop {
                let n = file.read(&mut buf)?;
                if n == 0 {
                    break;
                }
                hasher.update(&buf[..n]);
            }
            let digest: [u8; 20] = hasher.finalize().into();
            out.insert(rel_str, to_hex(&digest));
        }
    }
    Ok(())
}

/// Print a folder check report; returns an error when corrupt or missing.
pub fn report_folder(folder: &Path, check: &FolderCheck, source_desc: &str) -> Result<()> {
    println!(
        "verifying `{}` against {source_desc}: {} file(s) ok",
        folder.display(),
        check.ok
    );
    for f in &check.corrupt {
        eprintln!("corrupt: {f}");
    }
    for f in &check.missing {
        eprintln!("missing: {f}");
    }
    for f in &check.extra {
        eprintln!("info: extra file not in package: {f}");
    }
    if check.corrupt.is_empty() && check.missing.is_empty() {
        println!("ok: folder matches the package");
        Ok(())
    } else {
        Err(UpkgError::Verify(format!(
            "folder has {} corrupt and {} missing file(s)",
            check.corrupt.len(),
            check.missing.len()
        )))
    }
}


