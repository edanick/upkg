//! Repair (Section 10.3 of the spec).
//!
//! `upkg repair <folder> --package <file.upkg>` restores corrupt files
//! (original SHA-1 mismatch) and missing files (in the package but absent
//! from the folder):
//!
//! - `per-file`: only the affected files are read, using their data
//!   start/end offsets - fast;
//! - `whole-archive`: any single file requires decompressing the entire
//!   archive, so the tool extracts to the system temp directory first, then
//!   swaps the affected files - slow (acknowledged trade-off).

use std::io::Read;
use std::path::Path;

use crate::database::{self, DatabaseEntry, Status};
use crate::entries;
use crate::error::{Result, UpkgError};
use crate::header::CompressionKind;
use crate::package::{self, Package};
use crate::util::to_hex;
use crate::verify::{self, FolderCheck};
use sha1::Digest;

/// Repair an extracted folder against a package.
pub fn repair_folder(folder: &Path, pkg: &mut Package) -> Result<()> {
    match pkg.header.compression_kind {
        CompressionKind::PerFile => repair_per_file(folder, pkg),
        CompressionKind::WholeArchive => repair_whole_archive(folder, pkg),
    }
}

fn repair_per_file(folder: &Path, pkg: &mut Package) -> Result<()> {
    let mut check = FolderCheck::default();
    let mut fixed = 0usize;
    let entries = pkg.file_entries_owned();
    for entry in &entries {
        let target = entries::safe_join(folder, &entry.relative_path)?;
        let expected = to_hex(&entry.original_sha1.unwrap_or_default());
        let current = file_sha1(&target);
        if current.as_deref() == Some(expected.as_str()) {
            check.ok += 1;
            continue;
        }
        if current.is_none() {
            check.missing.push(entry.relative_path.clone());
        } else {
            check.corrupt.push(entry.relative_path.clone());
        }
        // Restore from the package: only the affected file is read.
        let raw = pkg.read_entry_raw(entry)?;
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| UpkgError::io_context(e, "cannot create folder"))?;
        }
        std::fs::write(&target, &raw)
            .map_err(|e| UpkgError::io_context(e, "cannot write repaired file"))?;
        apply_meta(&target, entry);
        fixed += 1;
    }
    report(folder, &check, "per-file", fixed)
}

fn repair_whole_archive(folder: &Path, pkg: &mut Package) -> Result<()> {
    // Decompress the whole archive to the system temp directory first
    // (Section 10.3 acknowledged trade-off).
    let temp = temp_dir();
    let archive_dir = temp.join(format!("upkg-repair-{}", std::process::id()));
    if archive_dir.exists() {
        std::fs::remove_dir_all(&archive_dir)
            .map_err(|e| UpkgError::io_context(e, "cannot clean repair temp dir"))?;
    }
    std::fs::create_dir_all(&archive_dir)
        .map_err(|e| UpkgError::io_context(e, "cannot create repair temp dir"))?;

    let extracted = package::whole_archive_extract(pkg)?;
    let mut check = FolderCheck::default();
    let mut fixed = 0usize;
    for (relative, bytes) in extracted {
        let tmp_path = entries::safe_join(&archive_dir, &relative)?;
        if let Some(parent) = tmp_path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| UpkgError::io_context(e, "cannot create temp folder"))?;
        }
        std::fs::write(&tmp_path, &bytes)
            .map_err(|e| UpkgError::io_context(e, "cannot write temp file"))?;

        let target = entries::safe_join(folder, &relative)?;
        let entry = pkg
            .file_entries()
            .into_iter()
            .find(|e| e.relative_path == relative)
            .ok_or_else(|| UpkgError::Format("entry vanished from tree".into()))?;
        let expected = to_hex(&entry.original_sha1.unwrap_or_default());
        let current = file_sha1(&target);
        if current.as_deref() == Some(expected.as_str()) {
            check.ok += 1;
            continue;
        }
        if current.is_none() {
            check.missing.push(relative.clone());
        } else {
            check.corrupt.push(relative.clone());
        }
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| UpkgError::io_context(e, "cannot create folder"))?;
        }
        std::fs::write(&target, &bytes)
            .map_err(|e| UpkgError::io_context(e, "cannot write repaired file"))?;
        apply_meta(&target, entry);
        fixed += 1;
    }

    let _ = std::fs::remove_dir_all(&archive_dir);
    report(folder, &check, "whole-archive", fixed)
}

fn report(folder: &Path, check: &FolderCheck, kind: &str, fixed: usize) -> Result<()> {
    println!(
        "repair of `{}` ({kind}): {} file(s) intact, {fixed} repaired",
        folder.display(),
        check.ok
    );
    for f in &check.corrupt {
        eprintln!("corrupt (repaired): {f}");
    }
    for f in &check.missing {
        eprintln!("missing (restored): {f}");
    }
    if check.corrupt.is_empty() && check.missing.is_empty() {
        println!("ok: nothing to repair");
    }
    Ok(())
}

/// After repairing, complete an interrupted database transaction
/// (revision 29: the entry stays `unpacked` until every file verifies).
pub fn complete_after_repair(folder: &Path) -> Result<()> {
    if let Some(entry) = database::find_by_install_path(folder)? {
        if entry.status == Status::Unpacked {
            let mut entry = entry;
            let mut check = FolderCheck::default();
            for f in &entry.files {
                let path = std::path::PathBuf::from(&entry.install_path).join(&f.relative_path);
                let current = file_sha1(&path);
                if current.as_deref() == Some(f.original_sha1.as_str()) {
                    check.ok += 1;
                } else {
                    check.corrupt.push(f.relative_path.clone());
                }
            }
            if check.corrupt.is_empty() {
                entry.status = Status::Installed;
                entry.save()?;
                println!("completed interrupted install of `{}`", entry.app_name);
            } else {
                return Err(UpkgError::Verify(format!(
                    "interrupted install of `{}` still has corrupt files: {:?}",
                    entry.app_name, check.corrupt
                )));
            }
        }
    }
    Ok(())
}

/// Complete an interrupted install using the database alone.
pub fn complete_entry(entry: &DatabaseEntry) -> Result<()> {
    database::complete_unpacked(entry)
}

/// SHA-1 of a file, or None when it does not exist.
fn file_sha1(path: &Path) -> Option<String> {
    let mut file = std::fs::File::open(path).ok()?;
    let mut hasher = sha1::Sha1::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf).ok()?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    let digest: [u8; 20] = hasher.finalize().into();
    Some(to_hex(&digest))
}

/// Apply stored mode/attributes to a written file (best effort).
fn apply_meta(path: &Path, entry: &entries::Entry) {
    #[cfg(unix)]
    if let Some(mode) = entry.mode {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = std::fs::metadata(path) {
            let mut perms = meta.permissions();
            perms.set_mode(mode);
            let _ = std::fs::set_permissions(path, perms);
        }
    }
    #[cfg(windows)]
    if let Some(attrs) = entry.attributes {
        if attrs.readonly {
            if let Ok(meta) = std::fs::metadata(path) {
                let mut perms = meta.permissions();
                perms.set_readonly(true);
                let _ = std::fs::set_permissions(path, perms);
            }
        }
    }
}

/// The system temp directory.
fn temp_dir() -> std::path::PathBuf {
    std::env::temp_dir()
}

/// Keep `verify::report_folder` referenced (folder reporting shared).
#[allow(dead_code)]
fn _ref_verify(folder: &Path, check: &FolderCheck) {
    let _ = verify::report_folder(folder, check, "package");
}
