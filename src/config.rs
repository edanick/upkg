//! The user-facing config file for `upkg create` (Section 6 of the spec).
//!
//! TOML format (proposal). Field names follow the spec's metadata field
//! names. `source` and `output` are proposals: the spec does not define how
//! `create` learns which folder to package, so `source` (the folder whose
//! contents become the package) is required here; `output` optionally selects
//! the output directory.

use std::path::PathBuf;

use serde::Deserialize;

use crate::error::{Result, UpkgError};
use crate::header::{Arch, Compression, CompressionKind, Os, PackageType};
use crate::metadata::{Dependency, ShortcutTemplate};

/// Raw TOML shape (all strings; validation happens in `parse`).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct RawConfig {
    app_name: String,
    app_version: Option<String>,
    // Optional so `upkg create` can warn and default to the host OS
    // (Section 5: missing components produce warnings, not silent failure).
    os: Option<String>,
    os_version: Option<String>,
    min: Option<String>,
    max: Option<String>,
    distro: Option<String>,
    strict: Option<bool>,
    attributes: Option<bool>,
    modes: Option<bool>,
    #[serde(default)]
    dependencies: Vec<RawDependency>,
    #[serde(default)]
    requirements: Vec<String>,
    #[serde(default)]
    conflicts: Vec<String>,
    #[serde(default)]
    replaces: Vec<String>,
    signing: Option<String>,
    #[serde(rename = "type")]
    package_type: Option<String>,
    arch: Option<String>,
    description: Option<String>,
    homepage: Option<String>,
    author: Option<String>,
    license: Option<String>,
    compression: Option<String>,
    compression_kind: Option<String>,
    compression_level: Option<u32>,
    source: String,
    output: Option<String>,
    shortcut: Option<RawShortcut>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum RawDependency {
    Plain(String),
    Bounded {
        name: String,
        #[serde(default)]
        min: Option<String>,
        #[serde(default)]
        max: Option<String>,
    },
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct RawShortcut {
    kind: String,
    name: Option<String>,
    exec: Option<String>,
    icon: Option<String>,
    comment: Option<String>,
    #[serde(rename = "working-directory")]
    working_directory: Option<String>,
    #[serde(rename = "type")]
    kind_type: Option<String>,
    categories: Option<String>,
    terminal: Option<bool>,
    #[serde(rename = "mime-type")]
    mime_type: Option<String>,
    path: Option<String>,
    #[serde(rename = "generic-name")]
    generic_name: Option<String>,
    keywords: Option<String>,
    #[serde(rename = "no-display")]
    no_display: Option<bool>,
    target: Option<String>,
    arguments: Option<String>,
    #[serde(rename = "icon-location")]
    icon_location: Option<String>,
    #[serde(rename = "icon-index")]
    icon_index: Option<i32>,
    description: Option<String>,
    #[serde(rename = "window-style")]
    window_style: Option<String>,
    hotkey: Option<String>,
    #[serde(rename = "run-as-admin")]
    run_as_admin: Option<bool>,
}

/// The validated create config.
#[derive(Debug, Clone)]
pub struct CreateConfig {
    pub app_name: String,
    pub app_version: String,
    pub os: Os,
    pub os_version: String,
    pub min: Option<String>,
    pub max: Option<String>,
    pub distro: Option<String>,
    pub strict: bool,
    pub attributes: bool,
    pub modes: bool,
    pub dependencies: Vec<Dependency>,
    pub requirements: Vec<String>,
    pub conflicts: Vec<String>,
    pub replaces: Vec<String>,
    /// Path to the ed25519 private key seed file (optional).
    pub signing: Option<PathBuf>,
    pub package_type: PackageType,
    pub arch: Option<Arch>,
    pub description: Option<String>,
    pub homepage: Option<String>,
    pub author: Option<String>,
    pub license: Option<String>,
    pub compression: Compression,
    pub compression_kind: CompressionKind,
    pub compression_level: u32,
    /// Folder whose contents become the package (proposal).
    pub source: PathBuf,
    /// Optional output directory (proposal).
    pub output: Option<PathBuf>,
    pub shortcut: Option<ShortcutTemplate>,
}

impl CreateConfig {
    /// Load and validate a config from a TOML file.
    pub fn load(path: &std::path::Path) -> Result<CreateConfig> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| UpkgError::Config(format!("cannot read config `{}`: {e}", path.display())))?;
        let raw: RawConfig = toml::from_str(&text)?;
        Self::from_raw(raw)
    }

    fn from_raw(raw: RawConfig) -> Result<CreateConfig> {
        if raw.app_name.is_empty() {
            return Err(UpkgError::Config(
                "required field `app-name` is missing or empty".into(),
            ));
        }
        let os: Os = match &raw.os {
            None => {
                // Warned by `create` (Section 5); default to the host OS.
                match crate::host::detect().os {
                    Some(h) => h,
                    None => {
                        return Err(UpkgError::Config(
                            "field `os` is required (could not detect the host OS either)".into(),
                        ))
                    }
                }
            }
            Some(s) => s
                .parse()
                .map_err(|e| UpkgError::Config(format!("field `os`: {e}")))?,
        };
        let package_type: PackageType = match &raw.package_type {
            None => PackageType::Application,
            Some(s) => s
                .parse()
                .map_err(|e| UpkgError::Config(format!("field `type`: {e}")))?,
        };
        let arch = match &raw.arch {
            None => None,
            Some(s) => Some(
                s.parse()
                    .map_err(|e| UpkgError::Config(format!("field `arch`: {e}")))?,
            ),
        };
        let compression: Compression = match &raw.compression {
            None => Compression::Zstd,
            Some(s) => s
                .parse()
                .map_err(|e| UpkgError::Config(format!("field `compression`: {e}")))?,
        };
        let compression_kind: CompressionKind = match &raw.compression_kind {
            None => CompressionKind::PerFile,
            Some(s) => s
                .parse()
                .map_err(|e| UpkgError::Config(format!("field `compression-kind`: {e}")))?,
        };
        let compression_level = raw.compression_level.unwrap_or(3);
        if !compression.valid_level(compression_level) {
            return Err(UpkgError::Config(format!(
                "invalid compression level {compression_level} for algorithm `{}`",
                compression.as_str()
            )));
        }

        let attributes = raw.attributes.unwrap_or(false);
        let modes = raw.modes.unwrap_or(false);
        if attributes && os != Os::Windows {
            return Err(UpkgError::Config(
                "`attributes: true` is only allowed for windows packages".into(),
            ));
        }
        if modes && os == Os::Windows {
            return Err(UpkgError::Config(
                "`modes: true` is only allowed for linux/mac packages".into(),
            ));
        }
        if raw.distro.is_some() && os != Os::Linux {
            return Err(UpkgError::Config(
                "`distro` is only allowed for linux packages".into(),
            ));
        }

        let dependencies = raw
            .dependencies
            .into_iter()
            .map(|d| match d {
                RawDependency::Plain(s) => Dependency::plain(&s),
                RawDependency::Bounded { name, min, max } => Dependency { name, min, max },
            })
            .collect();

        let shortcut = parse_shortcut(raw.shortcut, os)?;

        let signing = raw.signing.map(PathBuf::from);

        let source = PathBuf::from(&raw.source);
        if !source.is_dir() {
            return Err(UpkgError::Config(format!(
                "`source` folder `{}` does not exist or is not a directory",
                source.display()
            )));
        }

        Ok(CreateConfig {
            app_name: raw.app_name,
            app_version: raw.app_version.unwrap_or_default(),
            os,
            os_version: raw.os_version.unwrap_or_default(),
            min: raw.min,
            max: raw.max,
            distro: raw.distro,
            strict: raw.strict.unwrap_or(false),
            attributes,
            modes,
            dependencies,
            requirements: raw.requirements,
            conflicts: raw.conflicts,
            replaces: raw.replaces,
            signing,
            package_type,
            arch,
            description: raw.description,
            homepage: raw.homepage,
            author: raw.author,
            license: raw.license,
            compression,
            compression_kind,
            compression_level,
            source,
            output: raw.output.map(PathBuf::from),
            shortcut,
        })
    }
}

/// Validate the shortcut template against the target OS and its core fields
/// (constraint 12: mismatches are rejected at create time, before any work).
fn parse_shortcut(raw: Option<RawShortcut>, os: Os) -> Result<Option<ShortcutTemplate>> {
    let Some(raw) = raw else {
        return Ok(None);
    };
    let template = match raw.kind.as_str() {
        "universal" => {
            let name = required(&raw.name, "universal shortcut requires `name`")?;
            let exec = required(&raw.exec, "universal shortcut requires `exec`")?;
            ShortcutTemplate::Universal {
                name,
                exec,
                icon: raw.icon,
                comment: raw.comment,
                working_directory: raw.working_directory,
            }
        }
        "desktop" => {
            if os == Os::Windows {
                return Err(UpkgError::Config(
                    "desktop shortcut is not allowed for windows packages".into(),
                ));
            }
            let name = required(&raw.name, "desktop shortcut requires `name`")?;
            let exec = required(&raw.exec, "desktop shortcut requires `exec`")?;
            ShortcutTemplate::Desktop {
                name,
                comment: raw.comment,
                exec,
                icon: raw.icon,
                kind_type: raw.kind_type,
                categories: raw.categories,
                terminal: raw.terminal,
                mime_type: raw.mime_type,
                path: raw.path,
                generic_name: raw.generic_name,
                keywords: raw.keywords,
                no_display: raw.no_display,
            }
        }
        "lnk" => {
            if os != Os::Windows {
                return Err(UpkgError::Config(format!(
                    "lnk shortcut is only allowed for windows packages (package targets `{}`)",
                    os.as_str()
                )));
            }
            let target = required(&raw.target, "lnk shortcut requires `target`")?;
            ShortcutTemplate::Lnk {
                target,
                arguments: raw.arguments,
                working_directory: raw.working_directory,
                icon_location: raw.icon_location,
                icon_index: raw.icon_index,
                description: raw.description,
                window_style: raw.window_style,
                hotkey: raw.hotkey,
                run_as_admin: raw.run_as_admin,
            }
        }
        other => {
            return Err(UpkgError::Config(format!(
                "unknown shortcut kind `{other}` (allowed: universal, desktop, lnk)"
            )))
        }
    };
    Ok(Some(template))
}

fn required(value: &Option<String>, msg: &str) -> Result<String> {
    match value {
        Some(v) if !v.is_empty() => Ok(v.clone()),
        _ => Err(UpkgError::Config(msg.into())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_full_config() {
        let toml = r#"
app-name = "myapp"
app-version = "1.0.0"
os = "linux"
os-version = "22.04"
min = "20.04"
max = "24.04"
distro = "ubuntu"
strict = true
modes = true
dependencies = ["libc", { name = "foo", min = "1.0", max = "2.0" }]
conflicts = ["old-app"]
replaces = ["legacy-app"]
type = "game"
arch = "64"
compression = "zstd"
compression-kind = "per-file"
compression-level = 5
source = "tests/fixtures/app"
"#;
        let raw: RawConfig = toml::from_str(toml).unwrap();
        let cfg = CreateConfig::from_raw(raw).unwrap();
        assert_eq!(cfg.app_name, "myapp");
        assert_eq!(cfg.os, Os::Linux);
        assert!(cfg.strict);
        assert!(cfg.modes);
        assert_eq!(cfg.dependencies.len(), 2);
        assert_eq!(cfg.dependencies[1].min.as_deref(), Some("1.0"));
        assert_eq!(cfg.package_type, PackageType::Game);
        assert_eq!(cfg.arch, Some(Arch::X64));
        assert_eq!(cfg.compression_level, 5);
    }

    #[test]
    fn rejects_lnk_on_linux() {
        let toml = r#"
app-name = "x"
os = "linux"
source = "tests/fixtures/app"
[shortcut]
kind = "lnk"
target = "C:\\x.exe"
"#;
        let raw: RawConfig = toml::from_str(toml).unwrap();
        assert!(CreateConfig::from_raw(raw).is_err());
    }

    #[test]
    fn rejects_desktop_on_windows() {
        let toml = r#"
app-name = "x"
os = "windows"
source = "tests/fixtures/app"
[shortcut]
kind = "desktop"
name = "X"
exec = "x"
"#;
        let raw: RawConfig = toml::from_str(toml).unwrap();
        assert!(CreateConfig::from_raw(raw).is_err());
    }

    #[test]
    fn rejects_universal_without_exec() {
        let toml = r#"
app-name = "x"
os = "windows"
source = "tests/fixtures/app"
[shortcut]
kind = "universal"
name = "X"
"#;
        let raw: RawConfig = toml::from_str(toml).unwrap();
        assert!(CreateConfig::from_raw(raw).is_err());
    }
}
