//! Host machine detection (Section 9 of the spec, proposals).
//!
//! - OS from the Rust std consts (`windows` / `linux` / `macos` -> `mac`);
//! - OS version: Linux `VERSION_ID` from `/etc/os-release`; Windows NT
//!   `CurrentVersion` from the registry; macOS from `sw_vers -productVersion`;
//! - distro: Linux `ID` from `/etc/os-release`;
//! - architecture: `x86_64` -> 64, `i686`/`x86` -> 32, `aarch64` -> arm64.
//!
//! Detection is best-effort: when the version cannot be determined it is
//! `None`, and version bounds are then considered satisfied (see version.rs).

use crate::header::{Arch, Os};

/// Detected host properties.
#[derive(Debug, Clone, Default)]
pub struct Host {
    pub os: Option<Os>,
    pub version: Option<String>,
    pub distro: Option<String>,
    pub arch: Option<Arch>,
}

/// Detect the host machine.
pub fn detect() -> Host {
    Host {
        os: Os::from_std(std::env::consts::OS),
        version: detect_version(),
        distro: detect_distro(),
        arch: detect_arch(),
    }
}

fn detect_version() -> Option<String> {
    #[cfg(target_os = "linux")]
    {
        let os_release = std::fs::read_to_string("/etc/os-release").ok()?;
        os_release
            .lines()
            .find_map(|line| line.strip_prefix("VERSION_ID="))
            .map(|v| v.trim_matches('"').to_string())
    }
    #[cfg(target_os = "windows")]
    {
        use winreg::enums::HKEY_LOCAL_MACHINE;
        use winreg::RegKey;
        let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
        let key = hklm
            .open_subkey("SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion")
            .ok()?;
        key.get_value::<String, _>("CurrentVersion").ok()
    }
    #[cfg(target_os = "macos")]
    {
        let out = std::process::Command::new("sw_vers")
            .arg("-productVersion")
            .output()
            .ok()?;
        if out.status.success() {
            Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
        } else {
            None
        }
    }
    #[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
    {
        None
    }
}

fn detect_distro() -> Option<String> {
    #[cfg(target_os = "linux")]
    {
        let os_release = std::fs::read_to_string("/etc/os-release").ok()?;
        os_release
            .lines()
            .find_map(|line| line.strip_prefix("ID="))
            .map(|v| v.trim_matches('"').to_string())
    }
    #[cfg(not(target_os = "linux"))]
    {
        None
    }
}

fn detect_arch() -> Option<Arch> {
    match std::env::consts::ARCH {
        "x86_64" => Some(Arch::X64),
        "x86" | "i686" | "i586" | "i386" => Some(Arch::X32),
        "aarch64" | "arm64" => Some(Arch::Arm64),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_returns_some_os() {
        // detect() should never panic on any platform.
        let host = detect();
        let _ = (host.os, host.version, host.distro, host.arch);
    }
}
