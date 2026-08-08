//! Desktop shortcut generation (Section 6 of the spec, revision 26).
//!
//! A package may carry at most one shortcut template. At install time the
//! tool converts it into an OS-native shortcut:
//!
//! | template  | windows               | linux                          | mac                  |
//! |-----------|-----------------------|--------------------------------|----------------------|
//! | universal | `.lnk` on Desktop     | `.desktop` on `~/Desktop` (fallback `~/.local/share/applications`) | `.command` on `~/Desktop` |
//! | desktop   | rejected at create    | `.desktop`, same placement     | `.desktop` on `~/Desktop` |
//! | lnk       | `.lnk` on Desktop     | rejected at create             | rejected at create   |
//!
//! The generated file is named `<name>` plus the OS extension; invalid
//! filename characters in `name` are sanitized. The `.lnk` writer emits a
//! minimal Shell Link binary (MS-SHLLINK) with LinkInfo + StringData.

use std::path::{Path, PathBuf};

use crate::error::{Result, UpkgError};
use crate::header::Os;
use crate::metadata::ShortcutTemplate;

/// Sanitize a shortcut name into a safe file stem (proposal: invalid
/// filename characters are replaced with `_`).
pub fn sanitize_name(name: &str) -> String {
    let mut out = String::new();
    for c in name.chars() {
        let ok = !c.is_control()
            && c != '/'
            && c != '\\'
            && c != ':'
            && c != '*'
            && c != '?'
            && c != '"'
            && c != '<'
            && c != '>'
            && c != '|'
            && c != '\0';
        out.push(if ok { c } else { '_' });
    }
    if out.is_empty() {
        out.push_str("shortcut");
    }
    out
}

/// Where the desktop folder is on this host (best effort).
fn desktop_dir() -> Option<PathBuf> {
    dirs::desktop_dir()
}

/// The directory where shortcuts are placed on linux (Section 6).
fn linux_shortcut_dir() -> PathBuf {
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
    let desktop = home.join("Desktop");
    if desktop.is_dir() {
        desktop
    } else {
        home.join(".local/share/applications")
    }
}

/// Generate the shortcut file for an installed package.
///
/// `install_dir` is where the package files were placed (used by the
/// `desktop` template's `path` field and to resolve relative `exec` targets
/// for universal shortcuts on linux/mac).
pub fn generate(
    template: &ShortcutTemplate,
    target_os: Os,
    install_dir: &Path,
) -> Result<(PathBuf, Vec<u8>)> {
    let name = match template {
        ShortcutTemplate::Universal { name, .. }
        | ShortcutTemplate::Desktop { name, .. } => name.clone(),
        ShortcutTemplate::Lnk { .. } => String::new(),
    };
    match (template, target_os) {
        (ShortcutTemplate::Universal { .. }, Os::Linux) => {
            let dir = linux_shortcut_dir();
            let path = dir.join(format!("{}.desktop", sanitize_name(&name)));
            let content = universal_desktop(template, install_dir);
            Ok((path, content.into_bytes()))
        }
        (ShortcutTemplate::Desktop { .. }, Os::Linux) => {
            let dir = linux_shortcut_dir();
            let path = dir.join(format!("{}.desktop", sanitize_name(&name)));
            let content = desktop_entry(template);
            Ok((path, content.into_bytes()))
        }
        (ShortcutTemplate::Universal { .. }, Os::Mac) => {
            let dir = desktop_dir().unwrap_or_else(|| {
                dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")).join("Desktop")
            });
            let path = dir.join(format!("{}.command", sanitize_name(&name)));
            let exec = match template {
                ShortcutTemplate::Universal { exec, .. } => exec,
                _ => unreachable!(),
            };
            let content = format!("#!/bin/sh\nexec {exec}\n");
            Ok((path, content.into_bytes()))
        }
        (ShortcutTemplate::Desktop { .. }, Os::Mac) => {
            let dir = desktop_dir().unwrap_or_else(|| {
                dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")).join("Desktop")
            });
            let path = dir.join(format!("{}.desktop", sanitize_name(&name)));
            let content = desktop_entry(template);
            Ok((path, content.into_bytes()))
        }
        (ShortcutTemplate::Universal { .. }, Os::Windows) => {
            let dir = desktop_dir()
                .ok_or_else(|| UpkgError::Reject("cannot locate the user's desktop folder".into()))?;
            let path = dir.join(format!("{}.lnk", sanitize_name(&name)));
            let (target, arguments) = match template {
                ShortcutTemplate::Universal { exec, .. } => split_command_line(exec),
                _ => unreachable!(),
            };
            let bytes = write_lnk(
                &target,
                arguments.as_deref(),
                None,
                None,
                0,
                None,
                "normal",
                false,
            );
            Ok((path, bytes))
        }
        (ShortcutTemplate::Lnk { .. }, Os::Windows) => {
            let dir = desktop_dir()
                .ok_or_else(|| UpkgError::Reject("cannot locate the user's desktop folder".into()))?;
            let name = match template {
                ShortcutTemplate::Lnk { target, .. } => {
                    Path::new(target)
                        .file_stem()
                        .map(|s| s.to_string_lossy().to_string())
                        .unwrap_or_else(|| "shortcut".to_string())
                }
                _ => unreachable!(),
            };
            let path = dir.join(format!("{}.lnk", sanitize_name(&name)));
            let ShortcutTemplate::Lnk {
                target,
                arguments,
                working_directory,
                icon_location,
                icon_index,
                description,
                window_style,
                run_as_admin,
                ..
            } = template
            else {
                unreachable!()
            };
            let bytes = write_lnk(
                target,
                arguments.as_deref(),
                working_directory.as_deref(),
                icon_location.as_deref(),
                icon_index.unwrap_or(0),
                description.as_deref(),
                window_style.as_deref().unwrap_or("normal"),
                run_as_admin.unwrap_or(false),
            );
            Ok((path, bytes))
        }
        // Template/OS mismatches are rejected at create time and never reach
        // install (constraint 12), so these arms are unreachable.
        (ShortcutTemplate::Lnk { .. }, Os::Linux)
        | (ShortcutTemplate::Lnk { .. }, Os::Mac)
        | (ShortcutTemplate::Desktop { .. }, Os::Windows) => {
            Err(UpkgError::Reject(format!(
                "shortcut template `{}` is not allowed for OS `{}`",
                template.kind_name(),
                target_os.as_str()
            )))
        }
    }
}

/// Build a `.desktop` file from a universal template.
fn universal_desktop(template: &ShortcutTemplate, install_dir: &Path) -> String {
    let ShortcutTemplate::Universal {
        name,
        exec,
        icon,
        comment,
        working_directory,
    } = template
    else {
        unreachable!()
    };
    let mut out = String::new();
    out.push_str("[Desktop Entry]\n");
    out.push_str("Type=Application\n");
    out.push_str(&format!("Name={name}\n"));
    // Exec must be an absolute path or a command found in PATH; resolve a
    // relative first token against the install dir when it exists there.
    let first = exec.split_whitespace().next().unwrap_or("");
    let resolved = if !first.starts_with('/') {
        let candidate = install_dir.join(first);
        if candidate.exists() {
            let mut parts = exec.splitn(2, char::is_whitespace);
            match (parts.next(), parts.next()) {
                (Some(_), Some(rest)) => format!("{} {}", candidate.display(), rest),
                _ => candidate.display().to_string(),
            }
        } else {
            exec.clone()
        }
    } else {
        exec.clone()
    };
    out.push_str(&format!("Exec={resolved}\n"));
    if let Some(icon) = icon {
        out.push_str(&format!("Icon={icon}\n"));
    }
    if let Some(comment) = comment {
        out.push_str(&format!("Comment={comment}\n"));
    }
    if let Some(wd) = working_directory {
        out.push_str(&format!("Path={wd}\n"));
    }
    out
}

/// Build a `.desktop` file from a desktop template (full field set).
fn desktop_entry(template: &ShortcutTemplate) -> String {
    let ShortcutTemplate::Desktop {
        name,
        comment,
        exec,
        icon,
        kind_type,
        categories,
        terminal,
        mime_type,
        path,
        generic_name,
        keywords,
        no_display,
    } = template
    else {
        unreachable!()
    };
    let mut out = String::new();
    out.push_str("[Desktop Entry]\n");
    let type_value = kind_type.as_deref().unwrap_or("Application");
    out.push_str(&format!("Type={type_value}\n"));
    out.push_str(&format!("Name={name}\n"));
    if let Some(g) = generic_name {
        out.push_str(&format!("GenericName={g}\n"));
    }
    if let Some(c) = comment {
        out.push_str(&format!("Comment={c}\n"));
    }
    out.push_str(&format!("Exec={exec}\n"));
    if let Some(icon) = icon {
        out.push_str(&format!("Icon={icon}\n"));
    }
    if let Some(c) = categories {
        out.push_str(&format!("Categories={c}\n"));
    }
    if let Some(t) = terminal {
        out.push_str(&format!("Terminal={}\n", if *t { "true" } else { "false" }));
    }
    if let Some(m) = mime_type {
        out.push_str(&format!("MimeType={m}\n"));
    }
    if let Some(p) = path {
        out.push_str(&format!("Path={p}\n"));
    }
    if let Some(k) = keywords {
        out.push_str(&format!("Keywords={k}\n"));
    }
    if let Some(n) = no_display {
        out.push_str(&format!("NoDisplay={}\n", if *n { "true" } else { "false" }));
    }
    out
}

/// Split a command line into (first token, remaining arguments), the
/// universal-to-lnk conversion proposal.
pub fn split_command_line(exec: &str) -> (String, Option<String>) {
    let mut parts = exec.split_whitespace();
    let first = parts.next().unwrap_or("").to_string();
    let rest: Vec<&str> = parts.collect();
    let args = if rest.is_empty() {
        None
    } else {
        Some(rest.join(" "))
    };
    (first, args)
}

// ---------------------------------------------------------------------------
// Minimal .lnk (Shell Link) writer - MS-SHLLINK [MS-SHLLINK]
// ---------------------------------------------------------------------------

/// Windows `ShowCommand` values.
const SW_SHOWNORMAL: u32 = 1;
const SW_SHOWMAXIMIZED: u32 = 3;
const SW_SHOWMINIMIZED: u32 = 7;

/// Link flags we set.
const HAS_LINK_INFO: u32 = 0x0000_0002;
const HAS_NAME: u32 = 0x0000_0004;
const HAS_WORKING_DIR: u32 = 0x0000_0010;
const HAS_ARGUMENTS: u32 = 0x0000_0020;
const HAS_ICON_LOCATION: u32 = 0x0000_0040;
const IS_UNICODE: u32 = 0x0000_0080;
const RUN_AS_USER: u32 = 0x0000_2000;

fn write_utf16(out: &mut Vec<u8>, s: &str) {
    for unit in s.encode_utf16() {
        out.extend_from_slice(&unit.to_le_bytes());
    }
}

/// Write a StringData item: count (u16, includes the terminating NUL) + the
/// UTF-16 string with its NUL terminator.
fn write_string_data(out: &mut Vec<u8>, value: &str) {
    let count = value.encode_utf16().count() as u16 + 1;
    out.extend_from_slice(&count.to_le_bytes());
    write_utf16(out, value);
    out.extend_from_slice(&0u16.to_le_bytes());
}

/// Write a `.lnk` binary (Shell Link). All strings are UTF-16LE (IsUnicode).
pub fn write_lnk(
    target: &str,
    arguments: Option<&str>,
    working_directory: Option<&str>,
    icon_location: Option<&str>,
    icon_index: i32,
    description: Option<&str>,
    window_style: &str,
    run_as_admin: bool,
) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::new();

    // ---- ShellLinkHeader (76 bytes) ----
    out.extend_from_slice(&0x0000_004Cu32.to_le_bytes()); // HeaderSize
    out.extend_from_slice(&[
        0x01, 0x14, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0xC0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x46,
    ]); // LinkCLSID
    let mut flags = HAS_LINK_INFO | HAS_NAME | IS_UNICODE;
    if arguments.is_some() {
        flags |= HAS_ARGUMENTS;
    }
    if working_directory.is_some() {
        flags |= HAS_WORKING_DIR;
    }
    if icon_location.is_some() {
        flags |= HAS_ICON_LOCATION;
    }
    if run_as_admin {
        flags |= RUN_AS_USER;
    }
    out.extend_from_slice(&flags.to_le_bytes());
    out.extend_from_slice(&0x0000_0020u32.to_le_bytes()); // FileAttributes: ARCHIVE
    out.extend_from_slice(&0u64.to_le_bytes()); // CreationTime
    out.extend_from_slice(&0u64.to_le_bytes()); // AccessTime
    out.extend_from_slice(&0u64.to_le_bytes()); // WriteTime
    out.extend_from_slice(&0u32.to_le_bytes()); // FileSize
    out.extend_from_slice(&icon_index.to_le_bytes()); // IconIndex
    let show = match window_style {
        "maximized" => SW_SHOWMAXIMIZED,
        "minimized" => SW_SHOWMINIMIZED,
        _ => SW_SHOWNORMAL,
    };
    out.extend_from_slice(&show.to_le_bytes()); // ShowCommand
    out.extend_from_slice(&0u16.to_le_bytes()); // HotKey
    out.extend_from_slice(&0u16.to_le_bytes()); // Reserved1
    out.extend_from_slice(&0u32.to_le_bytes()); // Reserved2
    out.extend_from_slice(&0u32.to_le_bytes()); // Reserved3
    debug_assert_eq!(out.len(), 76);

    // ---- LinkInfo ----
    let link_info_start = out.len();
    let local_base_path = to_utf16_bytes(target);
    let common_suffix: Vec<u8> = to_utf16_bytes("");
    let header_size: u32 = 28;
    let local_base_path_offset = header_size;
    let common_path_suffix_offset = header_size + local_base_path.len() as u32;
    let link_info_size = common_path_suffix_offset + common_suffix.len() as u32;

    out.extend_from_slice(&link_info_size.to_le_bytes());
    out.extend_from_slice(&header_size.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes()); // LinkInfoFlags: 0 = local base path only
    out.extend_from_slice(&0u32.to_le_bytes()); // VolumeIDOffset (absent)
    out.extend_from_slice(&local_base_path_offset.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes()); // CommonNetworkRelativeLinkOffset
    out.extend_from_slice(&common_path_suffix_offset.to_le_bytes());
    debug_assert_eq!(out.len() - link_info_start, 28);
    out.extend_from_slice(&local_base_path);
    out.extend_from_slice(&common_suffix);
    debug_assert_eq!(out.len() - link_info_start, link_info_size as usize);

    // ---- StringData (IsUnicode -> UTF-16, count includes the NUL) ----
    if let Some(d) = description {
        write_string_data(&mut out, d);
    }
    if let Some(wd) = working_directory {
        write_string_data(&mut out, wd);
    }
    if let Some(args) = arguments {
        write_string_data(&mut out, args);
    }
    if let Some(icon) = icon_location {
        write_string_data(&mut out, icon);
    }

    out
}

/// UTF-16LE bytes including the trailing NUL.
fn to_utf16_bytes(s: &str) -> Vec<u8> {
    let mut out = Vec::new();
    write_utf16(&mut out, s);
    out.extend_from_slice(&0u16.to_le_bytes());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize() {
        assert_eq!(sanitize_name("My App"), "My App");
        assert_eq!(sanitize_name("a/b:c*?"), "a_b_c__");
        assert_eq!(sanitize_name(""), "shortcut");
    }

    #[test]
    fn split_exec() {
        assert_eq!(
            split_command_line("C:\\Apps\\myapp.exe --flag -x"),
            ("C:\\Apps\\myapp.exe".to_string(), Some("--flag -x".to_string()))
        );
        assert_eq!(split_command_line("myapp"), ("myapp".to_string(), None));
    }

    #[test]
    fn lnk_structure() {
        let bytes = write_lnk(
            "C:\\Apps\\myapp.exe",
            Some("--flag"),
            Some("C:\\Apps"),
            Some("C:\\Apps\\myapp.exe"),
            0,
            Some("My App"),
            "normal",
            false,
        );
        // Header size
        assert_eq!(&bytes[0..4], &0x4Cu32.to_le_bytes());
        // Flags contain IsUnicode
        let flags = u32::from_le_bytes(bytes[20..24].try_into().unwrap());
        assert_ne!(flags & IS_UNICODE, 0);
        assert_ne!(flags & HAS_ARGUMENTS, 0);
        // LinkInfo present
        assert_ne!(flags & HAS_LINK_INFO, 0);
        // Total length sanity
        assert!(bytes.len() > 200);
    }
}
