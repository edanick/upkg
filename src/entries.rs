//! The entries tree (Sections 7.4 and 7.6 of the spec).
//!
//! The tree is a recursive UTF-8 text structure: one line (record) per entry,
//! records delimited by LF (`0x0A`), fields within a record delimited by NUL
//! (`0x00`). Field layout is a proposal:
//!
//! ```text
//! depth \0 kind \0 relative path \0 filename \0 original SHA-1 \0
//!   [post-compression SHA-1 \0 data start \0 data end \0]   <- per-file kind only
//!   size \0 [mode | attributes] \n                           <- mode/attributes when enabled
//! ```
//!
//! - `depth` is the nesting level (decimal); a folder's children have
//!   `depth + 1`; depth 0 entries are the package root entries.
//! - folders leave the hash/offset/size fields empty;
//! - `size` is the uncompressed size in bytes (u64, decimal);
//! - `mode` is the Unix permission bits as octal (`0755`), for linux/mac
//!   packages when `modes` is enabled;
//! - `attributes` is a `name=1/0` comma list for windows packages when
//!   `attributes` is enabled.
//!
//! Which optional fields are present depends on the package: `has_offsets`
//! (compression kind is `per-file`), `has_mode` (linux/mac + `modes`),
//! `has_attributes` (windows + `attributes`). With offsets, a line has 10
//! fields; without, 7 fields.

use std::path::{Component, Path, PathBuf};

use crate::error::{Result, UpkgError};
use crate::util::{from_sha1_hex, to_hex};

/// Entry kind: a folder or a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    Folder,
    File,
}

impl EntryKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            EntryKind::Folder => "folder",
            EntryKind::File => "file",
        }
    }
}

/// Windows file attributes (true/false flags) stored per entry when enabled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct EntryAttributes {
    pub readonly: bool,
    pub hidden: bool,
    pub archive: bool,
    pub system: bool,
}

impl EntryAttributes {
    fn encode(&self) -> String {
        format!(
            "readonly={},hidden={},archive={},system={}",
            self.readonly as u8, self.hidden as u8, self.archive as u8, self.system as u8
        )
    }

    fn decode(s: &str) -> Result<EntryAttributes> {
        let mut a = EntryAttributes::default();
        for pair in s.split(',') {
            let Some((name, val)) = pair.split_once('=') else {
                return Err(UpkgError::Format(format!("malformed attributes `{s}`")));
            };
            let b = match val {
                "1" => true,
                "0" => false,
                _ => return Err(UpkgError::Format(format!("malformed attributes `{s}`"))),
            };
            match name {
                "readonly" => a.readonly = b,
                "hidden" => a.hidden = b,
                "archive" => a.archive = b,
                "system" => a.system = b,
                _ => return Err(UpkgError::Format(format!("unknown attribute `{name}`"))),
            }
        }
        Ok(a)
    }
}

/// One node of the entries tree: a folder (with children) or a file.
#[derive(Debug, Clone)]
pub struct Entry {
    pub kind: EntryKind,
    /// Path relative to the package root, e.g. `bin/myapp`.
    pub relative_path: String,
    /// Base name of the entry.
    pub filename: String,
    /// Per-file hash of the raw (uncompressed) contents (files only).
    pub original_sha1: Option<[u8; 20]>,
    /// Per-file hash of the stored (possibly compressed) bytes (`per-file` only).
    pub post_compression_sha1: Option<[u8; 20]>,
    /// Absolute byte offset where this file's data begins (`per-file` only).
    pub data_start: Option<u64>,
    /// Absolute byte offset where this file's data ends (`per-file` only).
    pub data_end: Option<u64>,
    /// Uncompressed size in bytes (files only).
    pub size: Option<u64>,
    /// Unix permission bits (linux/mac, when `modes` is enabled).
    pub mode: Option<u32>,
    /// Windows file attributes (windows, when `attributes` is enabled).
    pub attributes: Option<EntryAttributes>,
    /// Child entries (folders only).
    pub children: Vec<Entry>,
}

impl Entry {
    pub fn new_folder(relative_path: &str, filename: &str) -> Entry {
        Entry {
            kind: EntryKind::Folder,
            relative_path: relative_path.to_string(),
            filename: filename.to_string(),
            original_sha1: None,
            post_compression_sha1: None,
            data_start: None,
            data_end: None,
            size: None,
            mode: None,
            attributes: None,
            children: Vec::new(),
        }
    }

    pub fn new_file(relative_path: &str, filename: &str, original_sha1: [u8; 20], size: u64) -> Entry {
        Entry {
            kind: EntryKind::File,
            relative_path: relative_path.to_string(),
            filename: filename.to_string(),
            original_sha1: Some(original_sha1),
            post_compression_sha1: None,
            data_start: None,
            data_end: None,
            size: Some(size),
            mode: None,
            attributes: None,
            children: Vec::new(),
        }
    }

    /// All file entries in depth-first order.
    pub fn files(&self) -> Vec<&Entry> {
        let mut out = Vec::new();
        self.collect_files(&mut out);
        out
    }

    fn collect_files<'a>(&'a self, out: &mut Vec<&'a Entry>) {
        if self.kind == EntryKind::File {
            out.push(self);
        }
        for child in &self.children {
            child.collect_files(out);
        }
    }

    /// Mutable file entries in depth-first order (test helper).
    #[cfg(test)]
    pub fn files_mut(&mut self) -> Vec<&mut Entry> {
        let mut out = Vec::new();
        self.collect_files_mut(&mut out);
        out
    }

    #[cfg(test)]
    fn collect_files_mut<'a>(&'a mut self, out: &mut Vec<&'a mut Entry>) {
        // Files never have children in this format.
        if self.kind == EntryKind::File {
            out.push(self);
        } else {
            for child in &mut self.children {
                child.collect_files_mut(out);
            }
        }
    }
}

/// Options that determine which optional fields are present in the tree.
#[derive(Debug, Clone, Copy)]
pub struct TreeFormat {
    pub has_offsets: bool,
    pub has_mode: bool,
    pub has_attributes: bool,
}

/// Serialize the tree to its stored UTF-8 bytes.
pub fn serialize(roots: &[Entry], fmt: &TreeFormat) -> Vec<u8> {
    let mut out = Vec::new();
    for root in roots {
        write_entry(root, 0, fmt, &mut out);
    }
    out
}

fn write_entry(entry: &Entry, depth: u32, fmt: &TreeFormat, out: &mut Vec<u8>) {
    out.extend_from_slice(depth.to_string().as_bytes());
    out.push(0x00);
    out.extend_from_slice(entry.kind.as_str().as_bytes());
    out.push(0x00);
    out.extend_from_slice(entry.relative_path.as_bytes());
    out.push(0x00);
    out.extend_from_slice(entry.filename.as_bytes());
    out.push(0x00);
    match entry.kind {
        EntryKind::Folder => {
            out.push(0x00); // original sha1 empty
            if fmt.has_offsets {
                out.push(0x00); // post sha1
                out.push(0x00); // data start
                out.push(0x00); // data end
            }
            out.push(0x00); // size empty
        }
        EntryKind::File => {
            out.extend_from_slice(
                entry
                    .original_sha1
                    .map(|h| to_hex(&h))
                    .unwrap_or_default()
                    .as_bytes(),
            );
            out.push(0x00);
            if fmt.has_offsets {
                out.extend_from_slice(
                    entry
                        .post_compression_sha1
                        .map(|h| to_hex(&h))
                        .unwrap_or_default()
                        .as_bytes(),
                );
                out.push(0x00);
                out.extend_from_slice(entry.data_start.unwrap_or(0).to_string().as_bytes());
                out.push(0x00);
                out.extend_from_slice(entry.data_end.unwrap_or(0).to_string().as_bytes());
                out.push(0x00);
            }
            out.extend_from_slice(entry.size.unwrap_or(0).to_string().as_bytes());
            out.push(0x00);
        }
    }
    // The mode/attributes field is the LAST field of the line: it carries
    // no trailing separator (the line ends with LF).
    if fmt.has_mode {
        match entry.kind {
            EntryKind::Folder => {}
            EntryKind::File => out.extend_from_slice(
                format!("{:04o}", entry.mode.unwrap_or(0o644)).as_bytes(),
            ),
        }
    } else if fmt.has_attributes {
        match entry.kind {
            EntryKind::Folder => {}
            EntryKind::File => out.extend_from_slice(
                entry
                    .attributes
                    .unwrap_or_default()
                    .encode()
                    .as_bytes(),
            ),
        }
    }
    out.push(0x0A);
    for child in &entry.children {
        write_entry(child, depth + 1, fmt, out);
    }
}

/// A parsed tree line (before nesting is reconstructed).
#[derive(Debug)]
struct RawEntry {
    depth: u32,
    kind: EntryKind,
    relative_path: String,
    filename: String,
    original_sha1: Option<[u8; 20]>,
    post_compression_sha1: Option<[u8; 20]>,
    data_start: Option<u64>,
    data_end: Option<u64>,
    size: Option<u64>,
    mode: Option<u32>,
    attributes: Option<EntryAttributes>,
}

/// Parse the tree from its stored bytes.
pub fn parse(bytes: &[u8], fmt: &TreeFormat) -> Result<Vec<Entry>> {
    let text = std::str::from_utf8(bytes)
        .map_err(|_| UpkgError::Format("entries tree is not valid UTF-8".into()))?;
    if text.is_empty() {
        return Ok(Vec::new());
    }
    let mut raw = Vec::new();
    for line in text.split('\n') {
        if line.is_empty() {
            continue;
        }
        raw.push(parse_line(line, fmt)?);
    }
    let mut pos = 0;
    let roots = build_children(&raw, &mut pos, -1)?;
    if pos != raw.len() {
        return Err(UpkgError::Format("entries tree has orphan entries".into()));
    }
    Ok(roots)
}

fn parse_line(line: &str, fmt: &TreeFormat) -> Result<RawEntry> {
    let fields: Vec<&str> = line.split('\0').collect();
    let expected = if fmt.has_offsets { 10 } else { 7 };
    if fields.len() != expected {
        return Err(UpkgError::Format(format!(
            "entry line has {} fields, expected {expected}",
            fields.len()
        )));
    }
    let depth: u32 = fields[0]
        .parse()
        .map_err(|_| UpkgError::Format(format!("invalid entry depth `{}`", fields[0])))?;
    let kind = match fields[1] {
        "folder" => EntryKind::Folder,
        "file" => EntryKind::File,
        other => return Err(UpkgError::Format(format!("unknown entry type `{other}`"))),
    };
    let relative_path = fields[2].to_string();
    let filename = fields[3].to_string();

    let idx = |n: usize| -> Option<&str> {
        if n < fields.len() {
            Some(fields[n])
        } else {
            None
        }
    };

    let empty = |s: &str| s.is_empty();

    let mut raw = RawEntry {
        depth,
        kind,
        relative_path,
        filename,
        original_sha1: None,
        post_compression_sha1: None,
        data_start: None,
        data_end: None,
        size: None,
        mode: None,
        attributes: None,
    };

    match kind {
        EntryKind::Folder => {
            // index 4 (orig sha1) must be empty
            if !empty(idx(4).unwrap_or("")) {
                return Err(UpkgError::Format(
                    "folder entry carries a SHA-1 field".into(),
                ));
            }
        }
        EntryKind::File => {
            let orig = idx(4).unwrap_or("");
            raw.original_sha1 = Some(from_sha1_hex(orig)?);
        }
    }

    let mut next = 5;
    if fmt.has_offsets {
        if kind == EntryKind::File {
            let post = idx(next).unwrap_or("");
            raw.post_compression_sha1 = Some(from_sha1_hex(post)?);
            let start = idx(next + 1).unwrap_or("");
            let end = idx(next + 2).unwrap_or("");
            raw.data_start = Some(
                start
                    .parse()
                    .map_err(|_| UpkgError::Format(format!("invalid data start `{start}`")))?,
            );
            raw.data_end = Some(
                end.parse()
                    .map_err(|_| UpkgError::Format(format!("invalid data end `{end}`")))?,
            );
            if raw.data_end < raw.data_start {
                return Err(UpkgError::Format("file data end before its start".into()));
            }
        }
        next += 3;
    }

    match kind {
        EntryKind::Folder => {
            if !empty(idx(next).unwrap_or("")) {
                return Err(UpkgError::Format("folder entry carries a size field".into()));
            }
        }
        EntryKind::File => {
            let size = idx(next).unwrap_or("");
            raw.size = Some(
                size.parse()
                    .map_err(|_| UpkgError::Format(format!("invalid file size `{size}`")))?,
            );
        }
    }
    next += 1;

    if fmt.has_mode {
        let v = idx(next).unwrap_or("");
        if kind == EntryKind::File && !empty(v) {
            raw.mode = Some(u32::from_str_radix(v, 8).map_err(|_| {
                UpkgError::Format(format!("invalid mode `{v}` (expected octal)"))
            })?);
        }
    } else if fmt.has_attributes {
        let v = idx(next).unwrap_or("");
        if kind == EntryKind::File && !empty(v) {
            raw.attributes = Some(EntryAttributes::decode(v)?);
        }
    }

    Ok(raw)
}

fn build_children(raw: &[RawEntry], pos: &mut usize, parent_depth: i64) -> Result<Vec<Entry>> {
    let mut children = Vec::new();
    while *pos < raw.len() && raw[*pos].depth as i64 > parent_depth {
        let r = &raw[*pos];
        if r.depth as i64 != parent_depth + 1 {
            return Err(UpkgError::Format(format!(
                "entry depth jumps from {parent_depth} to {}",
                r.depth
            )));
        }
        *pos += 1;
        let mut entry = Entry {
            kind: r.kind,
            relative_path: r.relative_path.clone(),
            filename: r.filename.clone(),
            original_sha1: r.original_sha1,
            post_compression_sha1: r.post_compression_sha1,
            data_start: r.data_start,
            data_end: r.data_end,
            size: r.size,
            mode: r.mode,
            attributes: r.attributes,
            children: Vec::new(),
        };
        if entry.kind == EntryKind::Folder {
            entry.children = build_children(raw, pos, r.depth as i64)?;
        }
        children.push(entry);
    }
    Ok(children)
}

// ---------------------------------------------------------------------------
// Path safety (revision 27, constraint 13)
// ---------------------------------------------------------------------------

/// True when `p` uses only forward slashes and has no `..`, no drive letter,
/// no leading `/`, no backslash, no empty components and no `.` components.
pub fn validate_relative_path(p: &str) -> Result<()> {
    if p.is_empty() {
        return Err(UpkgError::Config("empty relative path".into()));
    }
    if p.starts_with('/') {
        return Err(UpkgError::Config(format!("absolute path `{p}` is not allowed")));
    }
    if p.contains('\\') {
        return Err(UpkgError::Config(format!(
            "backslash in path `{p}` is not allowed (use forward slashes)"
        )));
    }
    if p.contains('\0') || p.contains('\n') || p.contains('\r') {
        return Err(UpkgError::Config(format!(
            "path `{p}` contains NUL or line-breaker bytes"
        )));
    }
    let first = p.split('/').next().unwrap_or("");
    if first.len() == 2 && first.chars().next().unwrap_or(' ').is_ascii_alphabetic()
        && first.chars().nth(1) == Some(':')
    {
        return Err(UpkgError::Config(format!("drive letter in path `{p}` is not allowed")));
    }
    for component in p.split('/') {
        if component.is_empty() {
            return Err(UpkgError::Config(format!(
                "empty path component in `{p}` (double slash?)"
            )));
        }
        if component == "." {
            return Err(UpkgError::Config(format!("`.` component in path `{p}` is not allowed")));
        }
        if component == ".." {
            return Err(UpkgError::Config(format!(
                "`..` component in path `{p}` is not allowed"
            )));
        }
    }
    Ok(())
}

/// Windows-only checks: reserved device names and trailing dots/spaces.
pub fn validate_windows_path(p: &str) -> Result<()> {
    for component in p.split('/') {
        let stem = component.split('.').next().unwrap_or("").to_uppercase();
        let reserved = [
            "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6",
            "COM7", "COM8", "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6",
            "LPT7", "LPT8", "LPT9",
        ];
        if reserved.contains(&stem.as_str()) {
            return Err(UpkgError::Config(format!(
                "reserved device name `{component}` in path `{p}` is not allowed on windows"
            )));
        }
        if component.ends_with('.') || component.ends_with(' ') {
            return Err(UpkgError::Config(format!(
                "trailing dot or space in path component `{component}` is not allowed on windows"
            )));
        }
        if component.contains(':') {
            return Err(UpkgError::Config(format!(
                "colon in path component `{component}` is not allowed on windows"
            )));
        }
    }
    Ok(())
}

/// Check that no two entries collide case-insensitively (windows packages).
pub fn check_case_collisions(entries: &[Entry]) -> Result<()> {
    let mut seen: Vec<String> = Vec::new();
    for e in entries {
        let lower = e.relative_path.to_lowercase();
        if seen.contains(&lower) {
            return Err(UpkgError::Config(format!(
                "case-insensitive path collision on windows: `{}`",
                e.relative_path
            )));
        }
        seen.push(lower);
        check_case_collisions(&e.children)?;
    }
    Ok(())
}

/// Validate path safety of every entry in the tree (used by create and install).
pub fn validate_tree_paths(entries: &[Entry], os: crate::header::Os) -> Result<()> {
    for e in entries {
        validate_relative_path(&e.relative_path)?;
        if e.filename.contains('\0') || e.filename.contains('\n') || e.filename.contains('\r') {
            return Err(UpkgError::Config(format!(
                "filename `{}` contains NUL or line-breaker bytes",
                e.filename
            )));
        }
        if os == crate::header::Os::Windows {
            validate_windows_path(&e.relative_path)?;
        }
        validate_tree_paths(&e.children, os)?;
    }
    Ok(())
}

/// Safely join a relative path onto a base folder, refusing any escape.
/// (zip-slip protection used at install time)
pub fn safe_join(base: &Path, relative: &str) -> Result<PathBuf> {
    let rel = Path::new(relative);
    let mut out = base.to_path_buf();
    for comp in rel.components() {
        match comp {
            Component::Normal(part) => out.push(part),
            _ => {
                return Err(UpkgError::Reject(format!(
                    "unsafe path component in `{relative}`"
                )))
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_tree() -> Vec<Entry> {
        let mut root = Entry::new_folder("bin", "bin");
        let file = Entry::new_file("bin/myapp", "myapp", [7u8; 20], 1234);
        root.children.push(file);
        let mut docs = Entry::new_folder("docs", "docs");
        docs.children.push(Entry::new_file(
            "docs/readme.txt",
            "readme.txt",
            [9u8; 20],
            99,
        ));
        vec![root, docs]
    }

    /// Give every file entry its per-file offsets + post hash + mode.
    fn with_offsets(tree: &mut [Entry], fmt_offsets: bool) {
        for file in tree.iter_mut().flat_map(|e| e.files_mut()) {
            if fmt_offsets {
                file.post_compression_sha1 = Some([1u8; 20]);
                file.data_start = Some(1000);
                file.data_end = Some(1100);
            }
            file.mode = Some(0o755);
            file.attributes = Some(EntryAttributes {
                readonly: true,
                hidden: false,
                archive: true,
                system: false,
            });
        }
    }

    #[test]
    fn round_trip_per_file() {
        let fmt = TreeFormat {
            has_offsets: true,
            has_mode: true,
            has_attributes: false,
        };
        let mut tree = sample_tree();
        with_offsets(&mut tree, true);
        let bytes = serialize(&tree, &fmt);
        let parsed = parse(&bytes, &fmt).unwrap();
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].filename, "bin");
        assert_eq!(parsed[0].children.len(), 1);
        let f = &parsed[0].children[0];
        assert_eq!(f.relative_path, "bin/myapp");
        assert_eq!(f.original_sha1, Some([7u8; 20]));
        assert_eq!(f.post_compression_sha1, Some([1u8; 20]));
        assert_eq!(f.data_start, Some(1000));
        assert_eq!(f.data_end, Some(1100));
        assert_eq!(f.size, Some(1234));
        assert_eq!(f.mode, Some(0o755));
    }

    #[test]
    fn round_trip_whole_archive() {
        let fmt = TreeFormat {
            has_offsets: false,
            has_mode: false,
            has_attributes: true,
        };
        let mut tree = sample_tree();
        with_offsets(&mut tree, false);
        let bytes = serialize(&tree, &fmt);
        let parsed = parse(&bytes, &fmt).unwrap();
        assert_eq!(parsed[1].children[0].size, Some(99));
        let attrs = parsed[1].children[0].attributes.unwrap();
        assert!(attrs.readonly);
        assert!(attrs.archive);
    }

    #[test]
    fn files_flat_list() {
        let tree = sample_tree();
        let files = tree.iter().flat_map(|e| e.files()).count();
        assert_eq!(files, 2);
    }

    #[test]
    fn path_safety() {
        assert!(validate_relative_path("bin/myapp").is_ok());
        assert!(validate_relative_path("a/b/c.txt").is_ok());
        assert!(validate_relative_path("../evil").is_err());
        assert!(validate_relative_path("a/../b").is_err());
        assert!(validate_relative_path("/abs").is_err());
        assert!(validate_relative_path("C:/x").is_err());
        assert!(validate_relative_path("a\\b").is_err());
        assert!(validate_relative_path("a/./b").is_err());
        assert!(validate_relative_path("a//b").is_err());
        assert!(validate_relative_path("").is_err());
    }

    #[test]
    fn windows_path_safety() {
        assert!(validate_windows_path("bin/myapp.exe").is_ok());
        assert!(validate_windows_path("CON/evil").is_err());
        assert!(validate_windows_path("NUL.txt").is_err());
        assert!(validate_windows_path("trailing.").is_err());
        assert!(validate_windows_path("trailing ").is_err());
        assert!(validate_windows_path("col:on").is_err());
    }

    #[test]
    fn safe_join_rejects_escape() {
        let base = Path::new("/tmp/target");
        assert!(safe_join(base, "bin/myapp").is_ok());
        assert!(safe_join(base, "../evil").is_err());
        assert!(safe_join(base, "a/../../evil").is_err());
    }
}
