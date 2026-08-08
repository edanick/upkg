//! Install locations (Section 13 of the spec) and the install config.
//!
//! Default install root per OS (proposal):
//! - linux:   `~/.local/share/upkg/`
//! - windows: `%LOCALAPPDATA%\\upkg\\`
//! - mac:     `~/Library/Application Support/upkg/`
//!
//! The package database lives in `<root>/packages/` (Section 12).
//!
//! The install config (TOML, proposal) lives at:
//! - linux:   `~/.config/upkg/install.toml`
//! - windows: `%APPDATA%\\upkg\\install.toml`
//! - mac:     `~/Library/Application Support/upkg/install.toml`
//!
//! It may remap type folders to custom locations and set the speed-gate
//! download time limit:
//! ```toml
//! [folders]
//! games = "D:/Games"
//! max-download-minutes = 25
//! ```

use std::collections::HashMap;
use std::path::PathBuf;

use crate::header::PackageType;

/// The install root per OS.
///
/// The `UPKG_ROOT` environment variable overrides the default root
/// (proposal: useful for portable installs and testing).
pub fn install_root() -> PathBuf {
    if let Ok(root) = std::env::var("UPKG_ROOT") {
        if !root.is_empty() {
            return PathBuf::from(root);
        }
    }
    let base = dirs::data_dir().unwrap_or_else(|| PathBuf::from("."));
    base.join("upkg")
}

/// Directory holding one JSON file per installed package.
pub fn database_dir() -> PathBuf {
    install_root().join("packages")
}

/// Path of the user's install config.
pub fn install_config_path() -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        dirs::config_dir()
            .unwrap_or_else(|| install_root().clone())
            .join("upkg")
            .join("install.toml")
    }
    #[cfg(target_os = "macos")]
    {
        dirs::data_dir()
            .unwrap_or_else(|| install_root().clone())
            .join("upkg")
            .join("install.toml")
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        dirs::config_dir()
            .unwrap_or_else(|| install_root().clone())
            .join("upkg")
            .join("install.toml")
    }
}

/// Parsed install config.
#[derive(Debug, Clone, Default)]
pub struct InstallConfig {
    /// Remapped type folders: type name -> custom folder.
    pub folders: HashMap<String, PathBuf>,
    /// Configurable speed-gate limit in minutes (default 20, Section 11.2).
    pub max_download_minutes: Option<u64>,
}

impl InstallConfig {
    /// Load (an empty default when the file does not exist).
    pub fn load() -> InstallConfig {
        let path = install_config_path();
        let Ok(text) = std::fs::read_to_string(&path) else {
            return InstallConfig::default();
        };
        #[derive(serde::Deserialize)]
        struct Raw {
            #[serde(default)]
            folders: Option<HashMap<String, PathBuf>>,
            #[serde(rename = "max-download-minutes")]
            max_download_minutes: Option<u64>,
        }
        match toml::from_str::<Raw>(&text) {
            Ok(raw) => InstallConfig {
                folders: raw.folders.unwrap_or_default(),
                max_download_minutes: raw.max_download_minutes,
            },
            Err(_) => {
                eprintln!("warning: ignoring invalid install config at `{}`", path.display());
                InstallConfig::default()
            }
        }
    }

    /// Resolve the folder for a package type, honoring user remaps.
    pub fn folder_for(&self, package_type: PackageType) -> PathBuf {
        let type_name = package_type.as_str();
        self.folders
            .get(type_name)
            .cloned()
            .unwrap_or_else(|| PathBuf::from(package_type.default_folder()))
    }

    /// The final default install path: `<root>/<folder>/<app name>`
    /// (Section 13: `<root>/<type folder>/<app name>`).
    pub fn resolve_install_path(&self, package_type: PackageType, app_name: &str) -> PathBuf {
        install_root().join(self.folder_for(package_type)).join(app_name)
    }

    /// Speed-gate limit: the config value or the 20-minute default.
    pub fn max_download_minutes(&self) -> u64 {
        self.max_download_minutes.unwrap_or(20)
    }
}

/// The speed-gate fixed limit for slow connections (Section 11.2): refuse if
/// the estimated download time exceeds 5 minutes.
pub const SLOW_SPEED_GATE_MINUTES: u64 = 5;

/// Result of the speed gate decision (Section 11.2).
pub enum SpeedGate {
    /// Proceed with the full download.
    Proceed,
    /// Refuse: speed under 1 MB/s and estimated time above 5 minutes.
    TooSlow,
    /// Refuse: speed 1 MB/s or more and estimated time above the configured
    /// limit ("not possible with this host").
    TooLong { minutes: u64 },
}

/// Decide whether a full download should proceed given the measured speed
/// (bytes per second) and the total size in bytes.
pub fn speed_gate(speed_bytes_per_sec: f64, total_bytes: u64, config: &InstallConfig) -> SpeedGate {
    let seconds = total_bytes as f64 / speed_bytes_per_sec.max(1.0);
    let minutes = seconds / 60.0;
    let one_mib = 1024.0 * 1024.0;
    if speed_bytes_per_sec < one_mib {
        if minutes > SLOW_SPEED_GATE_MINUTES as f64 {
            SpeedGate::TooSlow
        } else {
            SpeedGate::Proceed
        }
    } else if minutes > config.max_download_minutes() as f64 {
        SpeedGate::TooLong {
            minutes: config.max_download_minutes(),
        }
    } else {
        SpeedGate::Proceed
    }
}

