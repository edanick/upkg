//! The package database (Sections 12 of the spec).
//!
//! On every successful install the tool records one JSON file per package in
//! `<root>/packages/<app-name>.json`. The file is the JSON document followed
//! by a newline and the hex SHA-1 of the JSON bytes (database integrity,
//! revision 29); the tool verifies it on every read and reports tampering.
//!
//! Install transactions (revision 29): files are written, the entry is
//! recorded with status `unpacked`, files are verified, then the entry is
//! marked `installed`. An `unpacked` entry means the previous run was
//! interrupted and must be completed.

use std::path::{Path, PathBuf};

use crate::error::{Result, UpkgError};
use crate::metadata::Dependency;
use crate::util::{sha1, to_hex};

/// Install state of an entry (revision 29).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Unpacked,
    Installed,
}

impl Status {
    pub fn as_str(&self) -> &'static str {
        match self {
            Status::Unpacked => "unpacked",
            Status::Installed => "installed",
        }
    }
}

/// One installed file: its relative path and original SHA-1.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct InstalledFile {
    pub relative_path: String,
    pub original_sha1: String,
}

/// A database entry (Section 12).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DatabaseEntry {
    /// Key of the entry (one installed version at a time).
    pub app_name: String,
    pub app_version: String,
    pub os: String,
    pub os_version: String,
    /// Where the files were placed.
    pub install_path: String,
    /// Installed file paths with their original SHA-1s.
    pub files: Vec<InstalledFile>,
    /// Whether a desktop shortcut was generated.
    pub shortcut: bool,
    /// Path of the generated shortcut (proposal - needed by `upkg remove`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shortcut_path: Option<String>,
    /// Declared dependencies, for dependency checks.
    #[serde(default)]
    pub dependencies: Vec<Dependency>,
    /// Install state.
    pub status: Status,
}

impl DatabaseEntry {
    /// The on-disk path for an app's database file.
    pub fn file_path(app_name: &str) -> PathBuf {
        crate::paths::database_dir().join(format!("{app_name}.json"))
    }

    /// Load and verify an entry.
    pub fn load(app_name: &str) -> Result<Option<DatabaseEntry>> {
        let path = Self::file_path(app_name);
        if !path.exists() {
            return Ok(None);
        }
        let raw = std::fs::read(&path)
            .map_err(|e| UpkgError::io_context(e, "cannot read database file"))?;
        let json_bytes = split_trailer(&raw).ok_or_else(|| {
            UpkgError::Verify(format!("database file `{}` has no SHA-1 trailer", path.display()))
        })?;
        let expected = sha1(json_bytes);
        let trailer_hex = std::str::from_utf8(&raw[raw.len() - 40..])
            .map_err(|_| UpkgError::Verify("database trailer is not valid hex".into()))?;
        let actual = to_hex(&expected);
        if trailer_hex != actual {
            return Err(UpkgError::Verify(format!(
                "database file `{}` is tampered (SHA-1 mismatch)",
                path.display()
            )));
        }
        let entry: DatabaseEntry = serde_json::from_slice(json_bytes)?;
        Ok(Some(entry))
    }

    /// Save the entry (JSON + SHA-1 trailer), creating the directory.
    pub fn save(&self) -> Result<()> {
        let dir = crate::paths::database_dir();
        std::fs::create_dir_all(&dir)
            .map_err(|e| UpkgError::io_context(e, "cannot create database directory"))?;
        let json = serde_json::to_vec_pretty(self)?;
        let mut out = json.clone();
        out.push(b'\n');
        out.extend_from_slice(to_hex(&sha1(&json)).as_bytes());
        let path = Self::file_path(&self.app_name);
        std::fs::write(&path, out)
            .map_err(|e| UpkgError::io_context(e, "cannot write database file"))
    }

    /// Remove the database file.
    pub fn remove(app_name: &str) -> Result<()> {
        let path = Self::file_path(app_name);
        if path.exists() {
            std::fs::remove_file(&path)
                .map_err(|e| UpkgError::io_context(e, "cannot remove database file"))?;
        }
        Ok(())
    }
}

/// List all installed entries (name-sorted).
pub fn list_installed() -> Result<Vec<DatabaseEntry>> {
    let dir = crate::paths::database_dir();
    let mut out = Vec::new();
    if !dir.exists() {
        return Ok(out);
    }
    for entry in std::fs::read_dir(&dir)
        .map_err(|e| UpkgError::io_context(e, "cannot read database directory"))?
    {
        let entry = entry.map_err(UpkgError::Io)?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let Some(name) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        if let Some(db) = DatabaseEntry::load(name)? {
            out.push(db);
        } else {
            eprintln!("warning: could not read database entry `{name}`");
        }
    }
    out.sort_by(|a, b| a.app_name.cmp(&b.app_name));
    Ok(out)
}

/// Find the installed entry whose install path equals `folder` (used by
/// `upkg verify <folder>` and `upkg repair <folder>` without `--package`).
pub fn find_by_install_path(folder: &Path) -> Result<Option<DatabaseEntry>> {
    let canonical = folder
        .canonicalize()
        .unwrap_or_else(|_| folder.to_path_buf());
    for entry in list_installed()? {
        let installed = PathBuf::from(&entry.install_path);
        let installed = installed.canonicalize().unwrap_or(installed);
        if installed == canonical {
            return Ok(Some(entry));
        }
    }
    Ok(None)
}

/// Split a database file into its JSON bytes and verify the SHA-1 trailer.
fn split_trailer(raw: &[u8]) -> Option<&[u8]> {
    if raw.len() < 41 {
        return None;
    }
    // Trailer: '\n' + 40 hex chars at the end.
    if raw[raw.len() - 41] != b'\n' {
        return None;
    }
    Some(&raw[..raw.len() - 41])
}

/// Complete an interrupted (status `unpacked`) entry: re-verify every file
/// against the stored hashes. Returns the entry.
pub fn complete_unpacked(entry: &DatabaseEntry) -> Result<()> {
    if entry.status == Status::Installed {
        return Ok(());
    }
    let base = PathBuf::from(&entry.install_path);
    for f in &entry.files {
        let path = base.join(&f.relative_path);
        let bytes = match std::fs::read(&path) {
            Ok(b) => b,
            Err(_) => {
                return Err(UpkgError::Verify(format!(
                    "interrupted install of `{}` is missing file `{}`; re-run `upkg install` or `upkg repair` with the package",
                    entry.app_name, f.relative_path
                )))
            }
        };
        let actual = to_hex(&sha1(&bytes));
        if actual != f.original_sha1 {
            return Err(UpkgError::Verify(format!(
                "interrupted install of `{}` has corrupt file `{}`; re-run `upkg install` or `upkg repair` with the package",
                entry.app_name, f.relative_path
            )));
        }
    }
    let mut entry = entry.clone();
    entry.status = Status::Installed;
    entry.save()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trailer_split() {
        let json = br#"{"a":1}"#;
        let mut file = json.to_vec();
        file.push(b'\n');
        file.extend_from_slice(to_hex(&sha1(json)).as_bytes());
        let j = split_trailer(&file).unwrap();
        assert_eq!(j, json);
        assert!(split_trailer(b"short").is_none());
    }

    #[test]
    fn entry_json_round_trip() {
        let entry = DatabaseEntry {
            app_name: "demo".into(),
            app_version: "1.0".into(),
            os: "linux".into(),
            os_version: "22.04".into(),
            install_path: "/tmp/demo".into(),
            files: vec![InstalledFile {
                relative_path: "bin/demo".into(),
                original_sha1: to_hex(&[1u8; 20]),
            }],
            shortcut: true,
            shortcut_path: Some("/tmp/demo.desktop".into()),
            dependencies: vec![],
            status: Status::Installed,
        };
        let json = serde_json::to_vec(&entry).unwrap();
        let back: DatabaseEntry = serde_json::from_slice(&json).unwrap();
        assert_eq!(back.app_name, "demo");
        assert_eq!(back.status, Status::Installed);
        assert_eq!(back.shortcut_path.as_deref(), Some("/tmp/demo.desktop"));
    }

    #[test]
    fn status_lowercase() {
        assert_eq!(serde_json::to_string(&Status::Unpacked).unwrap(), "\"unpacked\"");
        assert_eq!(serde_json::to_string(&Status::Installed).unwrap(), "\"installed\"");
    }
}
