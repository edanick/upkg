//! Package metadata file (Section 6 of the spec).
//!
//! The metadata file is a dpkg-control-like UTF-8 text file embedded in the
//! package (layout section 3), bounded by header fields 14-15 and covered by
//! the metadata SHA-1. Each line is `key: value`; continuation lines (leading
//! space or tab) continue the previous value with a newline, mirroring dpkg
//! control files. The NUL byte is not allowed in any value.

use crate::error::{Result, UpkgError};
use crate::header::{Arch, Os, PackageType};

/// A dependency with optional inclusive version bounds (revision 29).
/// Bounds semantics match OS-version bounds (Section 7.2).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Dependency {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max: Option<String>,
}

impl Dependency {
    /// A dependency with no bounds.
    pub fn plain(name: &str) -> Dependency {
        Dependency {
            name: name.to_string(),
            min: None,
            max: None,
        }
    }

    /// dpkg-style rendering: `name`, `name (>= 1.0)`, `name (>= 1.0, <= 2.0)`.
    pub fn to_dpkg(&self) -> String {
        match (&self.min, &self.max) {
            (None, None) => self.name.clone(),
            (Some(m), None) => format!("{} (>= {})", self.name, m),
            (None, Some(m)) => format!("{} (<= {})", self.name, m),
            (Some(mn), Some(mx)) => format!("{} (>= {}, <= {})", self.name, mn, mx),
        }
    }

    /// Parse a dpkg-style dependency string.
    pub fn from_dpkg(s: &str) -> Result<Dependency> {
        let s = s.trim();
        if let Some(open) = s.find('(') {
            if !s.ends_with(')') {
                return Err(UpkgError::Format(format!("malformed dependency `{s}`")));
            }
            let name = s[..open].trim().to_string();
            if name.is_empty() {
                return Err(UpkgError::Format(format!("malformed dependency `{s}`")));
            }
            let inner = &s[open + 1..s.len() - 1];
            let mut min = None;
            let mut max = None;
            for part in inner.split(',') {
                let part = part.trim();
                if let Some(rest) = part.strip_prefix(">=") {
                    min = Some(rest.trim().to_string());
                } else if let Some(rest) = part.strip_prefix("<=") {
                    max = Some(rest.trim().to_string());
                } else {
                    return Err(UpkgError::Format(format!(
                        "unsupported dependency operator in `{s}` (allowed: >=, <=)"
                    )));
                }
            }
            Ok(Dependency { name, min, max })
        } else {
            if s.is_empty() {
                return Err(UpkgError::Format("empty dependency name".into()));
            }
            Ok(Dependency::plain(s))
        }
    }
}

/// Shortcut template (revision 26, Section 6). At most one per package.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShortcutTemplate {
    /// Adapts to the target OS.
    Universal {
        name: String,
        exec: String,
        icon: Option<String>,
        comment: Option<String>,
        working_directory: Option<String>,
    },
    /// A customizable freedesktop `.desktop` template for linux/mac.
    Desktop {
        name: String,
        comment: Option<String>,
        exec: String,
        icon: Option<String>,
        kind_type: Option<String>,
        categories: Option<String>,
        terminal: Option<bool>,
        mime_type: Option<String>,
        path: Option<String>,
        generic_name: Option<String>,
        keywords: Option<String>,
        no_display: Option<bool>,
    },
    /// A windows-only `.lnk` template.
    Lnk {
        target: String,
        arguments: Option<String>,
        working_directory: Option<String>,
        icon_location: Option<String>,
        icon_index: Option<i32>,
        description: Option<String>,
        window_style: Option<String>,
        hotkey: Option<String>,
        run_as_admin: Option<bool>,
    },
}

impl ShortcutTemplate {
    pub fn kind_name(&self) -> &'static str {
        match self {
            ShortcutTemplate::Universal { .. } => "universal",
            ShortcutTemplate::Desktop { .. } => "desktop",
            ShortcutTemplate::Lnk { .. } => "lnk",
        }
    }
}

/// The parsed metadata file.
#[derive(Debug, Clone)]
pub struct Metadata {
    pub app_name: String,
    pub app_version: String,
    pub os: Os,
    pub os_version: String,
    pub min: Option<String>,
    pub max: Option<String>,
    pub distro: Option<String>,
    pub dependencies: Vec<Dependency>,
    pub requirements: Vec<String>,
    pub strict: bool,
    pub attributes: bool,
    pub modes: bool,
    pub conflicts: Vec<String>,
    pub replaces: Vec<String>,
    pub signing: bool,
    pub package_type: PackageType,
    pub arch: Option<Arch>,
    pub description: Option<String>,
    pub homepage: Option<String>,
    pub author: Option<String>,
    pub license: Option<String>,
    pub shortcut: Option<ShortcutTemplate>,
}

impl Default for Metadata {
    fn default() -> Self {
        Metadata {
            app_name: String::new(),
            app_version: String::new(),
            os: Os::Linux,
            os_version: String::new(),
            min: None,
            max: None,
            distro: None,
            dependencies: Vec::new(),
            requirements: Vec::new(),
            strict: false,
            attributes: false,
            modes: false,
            conflicts: Vec::new(),
            replaces: Vec::new(),
            signing: false,
            package_type: PackageType::Application,
            arch: None,
            description: None,
            homepage: None,
            author: None,
            license: None,
            shortcut: None,
        }
    }
}

/// Serialize the metadata file to its stored bytes.
pub fn serialize(m: &Metadata) -> String {
    let mut out = String::new();
    let field = |key: &str, value: &str, out: &mut String| {
        out.push_str(key);
        out.push_str(": ");
        out.push_str(value);
        out.push('\n');
    };
    field("app-name", &m.app_name, &mut out);
    field("app-version", &m.app_version, &mut out);
    field("os", m.os.as_str(), &mut out);
    field("os-version", &m.os_version, &mut out);
    if let Some(min) = &m.min {
        field("min", min, &mut out);
    }
    if let Some(max) = &m.max {
        field("max", max, &mut out);
    }
    if let Some(distro) = &m.distro {
        field("distro", distro, &mut out);
    }
    if !m.dependencies.is_empty() {
        let joined: Vec<String> = m.dependencies.iter().map(|d| d.to_dpkg()).collect();
        field("dependencies", &joined.join(", "), &mut out);
    }
    if !m.requirements.is_empty() {
        field("requirements", &m.requirements.join(", "), &mut out);
    }
    field("strict", if m.strict { "true" } else { "false" }, &mut out);
    field("attributes", if m.attributes { "true" } else { "false" }, &mut out);
    field("modes", if m.modes { "true" } else { "false" }, &mut out);
    if !m.conflicts.is_empty() {
        field("conflicts", &m.conflicts.join(", "), &mut out);
    }
    if !m.replaces.is_empty() {
        field("replaces", &m.replaces.join(", "), &mut out);
    }
    field("signing", if m.signing { "true" } else { "false" }, &mut out);
    field("type", m.package_type.as_str(), &mut out);
    if let Some(arch) = &m.arch {
        field("arch", arch.as_str(), &mut out);
    }
    if let Some(d) = &m.description {
        field("description", d, &mut out);
    }
    if let Some(h) = &m.homepage {
        field("homepage", h, &mut out);
    }
    if let Some(a) = &m.author {
        field("author", a, &mut out);
    }
    if let Some(l) = &m.license {
        field("license", l, &mut out);
    }
    match &m.shortcut {
        None => {}
        Some(ShortcutTemplate::Universal {
            name,
            exec,
            icon,
            comment,
            working_directory,
        }) => {
            field("shortcut-kind", "universal", &mut out);
            field("shortcut-name", name, &mut out);
            field("shortcut-exec", exec, &mut out);
            if let Some(v) = icon {
                field("shortcut-icon", v, &mut out);
            }
            if let Some(v) = comment {
                field("shortcut-comment", v, &mut out);
            }
            if let Some(v) = working_directory {
                field("shortcut-working-directory", v, &mut out);
            }
        }
        Some(ShortcutTemplate::Desktop {
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
        }) => {
            field("shortcut-kind", "desktop", &mut out);
            field("shortcut-name", name, &mut out);
            if let Some(v) = comment {
                field("shortcut-comment", v, &mut out);
            }
            field("shortcut-exec", exec, &mut out);
            if let Some(v) = icon {
                field("shortcut-icon", v, &mut out);
            }
            if let Some(v) = kind_type {
                field("shortcut-type", v, &mut out);
            }
            if let Some(v) = categories {
                field("shortcut-categories", v, &mut out);
            }
            if let Some(v) = terminal {
                field("shortcut-terminal", if *v { "true" } else { "false" }, &mut out);
            }
            if let Some(v) = mime_type {
                field("shortcut-mime-type", v, &mut out);
            }
            if let Some(v) = path {
                field("shortcut-path", v, &mut out);
            }
            if let Some(v) = generic_name {
                field("shortcut-generic-name", v, &mut out);
            }
            if let Some(v) = keywords {
                field("shortcut-keywords", v, &mut out);
            }
            if let Some(v) = no_display {
                field("shortcut-no-display", if *v { "true" } else { "false" }, &mut out);
            }
        }
        Some(ShortcutTemplate::Lnk {
            target,
            arguments,
            working_directory,
            icon_location,
            icon_index,
            description,
            window_style,
            hotkey,
            run_as_admin,
        }) => {
            field("shortcut-kind", "lnk", &mut out);
            field("shortcut-target", target, &mut out);
            if let Some(v) = arguments {
                field("shortcut-arguments", v, &mut out);
            }
            if let Some(v) = working_directory {
                field("shortcut-working-directory", v, &mut out);
            }
            if let Some(v) = icon_location {
                field("shortcut-icon-location", v, &mut out);
            }
            if let Some(v) = icon_index {
                field("shortcut-icon-index", &v.to_string(), &mut out);
            }
            if let Some(v) = description {
                field("shortcut-description", v, &mut out);
            }
            if let Some(v) = window_style {
                field("shortcut-window-style", v, &mut out);
            }
            if let Some(v) = hotkey {
                field("shortcut-hotkey", v, &mut out);
            }
            if let Some(v) = run_as_admin {
                field("shortcut-run-as-admin", if *v { "true" } else { "false" }, &mut out);
            }
        }
    }
    out
}

/// Parse the metadata file from its stored bytes.
pub fn parse(bytes: &[u8]) -> Result<Metadata> {
    let text = std::str::from_utf8(bytes)
        .map_err(|_| UpkgError::Format("metadata file is not valid UTF-8".into()))?;
    if text.contains('\0') {
        return Err(UpkgError::Format(
            "metadata file contains a NUL byte (values must not contain 0x00 or 0x0A)".into(),
        ));
    }

    // Collect key/value pairs, handling dpkg-style continuation lines.
    let mut fields: Vec<(String, String)> = Vec::new();
    for raw_line in text.lines() {
        if raw_line.starts_with(' ') || raw_line.starts_with('\t') {
            if let Some(last) = fields.last_mut() {
                last.1.push('\n');
                last.1.push_str(raw_line.trim());
            }
            continue;
        }
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some(colon) = line.find(':') else {
            return Err(UpkgError::Format(format!(
                "malformed metadata line (expected `key: value`): `{line}`"
            )));
        };
        let key = line[..colon].trim().to_string();
        let value = line[colon + 1..].trim().to_string();
        fields.push((key, value));
    }

    let mut m = Metadata::default();
    let mut shortcut_fields: Vec<(String, String)> = Vec::new();

    for (key, value) in &fields {
        match key.as_str() {
            "app-name" => m.app_name = value.clone(),
            "app-version" => m.app_version = value.clone(),
            "os" => m.os = value.parse().map_err(UpkgError::Format)?,
            "os-version" => m.os_version = value.clone(),
            "min" => m.min = opt(value),
            "max" => m.max = opt(value),
            "distro" => m.distro = opt(value),
            "dependencies" => {
                m.dependencies = split_dependencies(value)
                    .into_iter()
                    .map(|s| Dependency::from_dpkg(&s))
                    .collect::<Result<Vec<_>>>()?;
            }
            "requirements" => m.requirements = split_list(value),
            "strict" => m.strict = parse_bool(key, value)?,
            "attributes" => m.attributes = parse_bool(key, value)?,
            "modes" => m.modes = parse_bool(key, value)?,
            "conflicts" => m.conflicts = split_list(value),
            "replaces" => m.replaces = split_list(value),
            "signing" => m.signing = parse_bool(key, value)?,
            "type" => {
                m.package_type = value.parse().map_err(UpkgError::Format)?;
            }
            "arch" => m.arch = opt(value).map(|a| a.parse()).transpose().map_err(UpkgError::Format)?,
            "description" => m.description = opt(value),
            "homepage" => m.homepage = opt(value),
            "author" => m.author = opt(value),
            "license" => m.license = opt(value),
            _ => {
                if key.starts_with("shortcut-") {
                    shortcut_fields.push((key.clone(), value.clone()));
                } else {
                    // Unknown informational fields are ignored (forward compat).
                }
            }
        }
    }

    m.shortcut = parse_shortcut(&shortcut_fields)?;

    if m.app_name.is_empty() {
        return Err(UpkgError::Format("metadata is missing required field `app-name`".into()));
    }
    if m.os_version.is_empty() {
        return Err(UpkgError::Format("metadata is missing required field `os-version`".into()));
    }
    Ok(m)
}

fn opt(value: &str) -> Option<String> {
    if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

fn parse_bool(key: &str, value: &str) -> Result<bool> {
    match value {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => Err(UpkgError::Format(format!(
            "field `{key}` must be true or false, got `{value}`"
        ))),
    }
}

/// Split a comma-separated list, trimming entries.
fn split_list(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// Split a dependency list, respecting the parentheses of version bounds
/// (dpkg syntax: `a (>= 1.0, <= 2.0), b`).
fn split_dependencies(value: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut depth = 0usize;
    let mut current = String::new();
    for c in value.chars() {
        match c {
            '(' => {
                depth += 1;
                current.push(c);
            }
            ')' => {
                depth = depth.saturating_sub(1);
                current.push(c);
            }
            ',' if depth == 0 => {
                let trimmed = current.trim();
                if !trimmed.is_empty() {
                    out.push(trimmed.to_string());
                }
                current.clear();
            }
            _ => current.push(c),
        }
    }
    let trimmed = current.trim();
    if !trimmed.is_empty() {
        out.push(trimmed.to_string());
    }
    out
}

fn parse_shortcut(fields: &[(String, String)]) -> Result<Option<ShortcutTemplate>> {
    if fields.is_empty() {
        return Ok(None);
    }
    let kind = fields
        .iter()
        .find(|(k, _)| k == "shortcut-kind")
        .map(|(_, v)| v.clone())
        .ok_or_else(|| UpkgError::Format("shortcut block is missing `shortcut-kind`".into()))?;

    let get = |name: &str| -> Option<String> {
        fields
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.clone())
            .filter(|v| !v.is_empty())
    };
    let get_bool = |name: &str| -> Result<Option<bool>> {
        match get(name) {
            None => Ok(None),
            Some(v) => parse_bool(name, &v).map(Some),
        }
    };

    let template = match kind.as_str() {
        "universal" => {
            let name = get("shortcut-name")
                .ok_or_else(|| UpkgError::Format("universal shortcut is missing `shortcut-name`".into()))?;
            let exec = get("shortcut-exec")
                .ok_or_else(|| UpkgError::Format("universal shortcut is missing `shortcut-exec`".into()))?;
            ShortcutTemplate::Universal {
                name,
                exec,
                icon: get("shortcut-icon"),
                comment: get("shortcut-comment"),
                working_directory: get("shortcut-working-directory"),
            }
        }
        "desktop" => {
            let name = get("shortcut-name")
                .ok_or_else(|| UpkgError::Format("desktop shortcut is missing `shortcut-name`".into()))?;
            let exec = get("shortcut-exec")
                .ok_or_else(|| UpkgError::Format("desktop shortcut is missing `shortcut-exec`".into()))?;
            ShortcutTemplate::Desktop {
                name,
                comment: get("shortcut-comment"),
                exec,
                icon: get("shortcut-icon"),
                kind_type: get("shortcut-type"),
                categories: get("shortcut-categories"),
                terminal: get_bool("shortcut-terminal")?,
                mime_type: get("shortcut-mime-type"),
                path: get("shortcut-path"),
                generic_name: get("shortcut-generic-name"),
                keywords: get("shortcut-keywords"),
                no_display: get_bool("shortcut-no-display")?,
            }
        }
        "lnk" => {
            let target = get("shortcut-target")
                .ok_or_else(|| UpkgError::Format("lnk shortcut is missing `shortcut-target`".into()))?;
            ShortcutTemplate::Lnk {
                target,
                arguments: get("shortcut-arguments"),
                working_directory: get("shortcut-working-directory"),
                icon_location: get("shortcut-icon-location"),
                icon_index: get("shortcut-icon-index")
                    .map(|v| v.parse().map_err(|_| UpkgError::Format(format!("invalid shortcut-icon-index `{v}`"))))
                    .transpose()?,
                description: get("shortcut-description"),
                window_style: get("shortcut-window-style"),
                hotkey: get("shortcut-hotkey"),
                run_as_admin: get_bool("shortcut-run-as-admin")?,
            }
        }
        other => {
            return Err(UpkgError::Format(format!(
                "unknown shortcut kind `{other}` (allowed: universal, desktop, lnk)"
            )))
        }
    };
    Ok(Some(template))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Metadata {
        let mut m = Metadata::default();
        m.app_name = "myapp".into();
        m.app_version = "1.0.0".into();
        m.os = Os::Linux;
        m.os_version = "22.04".into();
        m.min = Some("20.04".into());
        m.max = Some("24.04".into());
        m.distro = Some("ubuntu".into());
        m.dependencies = vec![
            Dependency::plain("libc"),
            Dependency {
                name: "foo".into(),
                min: Some("1.0".into()),
                max: Some("2.0".into()),
            },
        ];
        m.strict = true;
        m.modes = true;
        m.package_type = PackageType::Game;
        m.arch = Some(Arch::X64);
        m.description = Some("A test app".into());
        m.shortcut = Some(ShortcutTemplate::Universal {
            name: "My App".into(),
            exec: "myapp --flag".into(),
            icon: None,
            comment: Some("launch".into()),
            working_directory: None,
        });
        m
    }

    #[test]
    fn round_trip() {
        let m = sample();
        let bytes = serialize(&m);
        let parsed = parse(bytes.as_bytes()).unwrap();
        assert_eq!(parsed.app_name, "myapp");
        assert_eq!(parsed.app_version, "1.0.0");
        assert_eq!(parsed.os, Os::Linux);
        assert_eq!(parsed.os_version, "22.04");
        assert_eq!(parsed.min.as_deref(), Some("20.04"));
        assert_eq!(parsed.max.as_deref(), Some("24.04"));
        assert_eq!(parsed.distro.as_deref(), Some("ubuntu"));
        assert_eq!(parsed.dependencies.len(), 2);
        assert_eq!(parsed.dependencies[1].min.as_deref(), Some("1.0"));
        assert!(parsed.strict);
        assert!(parsed.modes);
        assert_eq!(parsed.package_type, PackageType::Game);
        assert_eq!(parsed.arch, Some(Arch::X64));
        assert_eq!(parsed.description.as_deref(), Some("A test app"));
        match parsed.shortcut {
            Some(ShortcutTemplate::Universal { name, exec, comment, .. }) => {
                assert_eq!(name, "My App");
                assert_eq!(exec, "myapp --flag");
                assert_eq!(comment.as_deref(), Some("launch"));
            }
            _ => panic!("expected universal shortcut"),
        }
    }

    #[test]
    fn dependency_dpkg() {
        let d = Dependency::from_dpkg("foo (>= 1.0, <= 2.0)").unwrap();
        assert_eq!(d.name, "foo");
        assert_eq!(d.min.as_deref(), Some("1.0"));
        assert_eq!(d.max.as_deref(), Some("2.0"));
        assert_eq!(Dependency::from_dpkg("bar").unwrap().name, "bar");
        assert!(Dependency::from_dpkg("baz (= 1.0)").is_err());
    }
}
