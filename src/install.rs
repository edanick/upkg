//! Install (Section 9 of the spec).
//!
//! Order of operations (revision 29 install transaction):
//! 1. signature check (reject on invalid), OS compatibility, path safety,
//!    conflicts, dependencies - all before any file is written;
//! 2. write all files;
//! 3. record the database entry with status `unpacked`;
//! 4. verify every file against its original SHA-1 (rewriting mismatched or
//!    missing files);
//! 5. mark the entry `installed`;
//! 6. generate the desktop shortcut (if the package carries one).
//!
//! The `preflight` and `finalize` pieces are shared with online install
//! (Section 11), which obtains file bytes differently.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::database::{DatabaseEntry, InstalledFile, Status};
use crate::entries;
use crate::error::{Result, UpkgError};
use crate::header::{CompressionKind, Header, Os};
use crate::host;
use crate::metadata::{Metadata, ShortcutTemplate};
use crate::package::{self, Package};
use crate::paths::InstallConfig;
use crate::prompt;
use crate::shortcut;
use crate::util::{sha1, to_hex};
use crate::version;
use sha1::Digest;

/// Install a local `.upkg` file.
pub fn install_local(package_path: &Path) -> Result<()> {
    let mut pkg = package::open(package_path)?;
    install_package(&mut pkg, None)
}

/// The full local install flow. `explicit_folder` is used by online install
/// (`upkg install <url> <folder>`); local install resolves the type-based
/// default path (Section 13).
pub fn install_package(pkg: &mut Package, explicit_folder: Option<&Path>) -> Result<()> {
    // --- preflight: nothing is written before all checks pass ---
    pkg.verify_signature()?;
    preflight(&pkg.header, &pkg.metadata, &pkg.tree)?;

    // --- resolve the install path ---
    let install_config = InstallConfig::load();
    let install_path = match explicit_folder {
        Some(f) => f.to_path_buf(),
        None => install_config.resolve_install_path(pkg.header.package_type, &pkg.metadata.app_name),
    };

    // Complete any interrupted install of the same app (revision 29).
    if let Ok(Some(entry)) = DatabaseEntry::load(&pkg.metadata.app_name) {
        if entry.status == Status::Unpacked {
            eprintln!(
                "note: found an interrupted install of `{}` - completing verification",
                pkg.metadata.app_name
            );
            if crate::database::complete_unpacked(&entry).is_err() {
                eprintln!("note: interrupted install is incomplete; re-installing now");
            }
        }
    }

    // --- write all files ---
    extract_all(pkg, &install_path)?;

    // --- transaction: record `unpacked`, verify (rewriting mismatched or
    // missing files from the package), mark `installed` ---
    let mut entry = build_entry_from_tree(&pkg.header, &pkg.metadata, &pkg.tree, &install_path);
    entry.status = Status::Unpacked;
    entry.save()?;

    verify_written_files(pkg, &install_path)?;

    entry.status = Status::Installed;
    entry.save()?;
    println!(
        "installed `{}` {} to `{}`",
        entry.app_name, entry.app_version, install_path.display()
    );

    generate_shortcut_for(&mut entry, pkg.metadata.shortcut.as_ref(), pkg.header.os, &install_path)
}

/// All install-time checks that must pass before any file is written
/// (signature is checked separately by the caller, which may download the
/// whole file first).
pub fn preflight(header: &Header, metadata: &Metadata, tree: &[entries::Entry]) -> Result<()> {
    check_compatibility(header, metadata)?;
    entries::validate_tree_paths(tree, header.os)?;
    check_conflicts_and_apply_replaces(metadata)?;
    handle_dependencies(metadata)?;
    Ok(())
}

/// Record the database entry (status `unpacked`), verify the files on disk
/// and mark the entry `installed`; then generate the shortcut. Used by
/// online install, where files were already verified while streaming.
pub fn finalize(
    entry: &mut DatabaseEntry,
    template: Option<&ShortcutTemplate>,
    os: Os,
    install_path: &Path,
) -> Result<()> {
    entry.status = Status::Unpacked;
    entry.save()?;

    // Transaction step 3: verify every file against its SHA-1.
    if !verify_installed_files(entry, install_path)? {
        return Err(UpkgError::Verify(format!(
            "installed files of `{}` failed verification",
            entry.app_name
        )));
    }

    entry.status = Status::Installed;
    entry.save()?;

    println!(
        "installed `{}` {} to `{}`",
        entry.app_name, entry.app_version, install_path.display()
    );

    generate_shortcut_for(entry, template, os, install_path)
}

/// Generate the desktop shortcut after the files are installed (Section 9)
/// and record it in the database entry.
pub fn generate_shortcut_for(
    entry: &mut DatabaseEntry,
    template: Option<&ShortcutTemplate>,
    os: Os,
    install_path: &Path,
) -> Result<()> {
    if let Some(template) = template {
        let (path, bytes) = shortcut::generate(template, os, install_path)?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| UpkgError::io_context(e, "cannot create shortcut folder"))?;
        }
        std::fs::write(&path, &bytes)
            .map_err(|e| UpkgError::io_context(e, "cannot write shortcut"))?;
        make_executable(&path);
        entry.shortcut = true;
        entry.shortcut_path = Some(path.display().to_string());
        entry.save()?;
        println!("created desktop shortcut `{}`", path.display());
    }
    Ok(())
}

/// OS/version/distro/arch compatibility (Section 9, revisions 6/25/28).
pub fn check_compatibility(header: &Header, metadata: &Metadata) -> Result<()> {
    let host = host::detect();
    let Some(host_os) = host.os else {
        // Cannot determine the host OS - proceed (best effort).
        return Ok(());
    };

    if header.os != host_os {
        return Err(UpkgError::Reject(format!(
            "package targets `{}` but this host is `{}`",
            header.os.as_str(),
            host_os.as_str()
        )));
    }

    // Version mismatch: the host version falls outside the declared range.
    if let Some(host_version) = &host.version {
        let in_range = version::in_range(
            host_version,
            header.min.as_deref(),
            header.max.as_deref(),
        );
        if !in_range {
            if header.strict {
                return Err(UpkgError::Reject(format!(
                    "package requires {} version in range [{}, {}] but this host is version `{host_version}` (strict)",
                    header.os.as_str(),
                    header.min.clone().unwrap_or_else(|| "any".into()),
                    header.max.clone().unwrap_or_else(|| "any".into()),
                )));
            }
            eprintln!(
                "warning: package targets {} version {}, this host is `{host_version}` - installing anyway",
                header.os.as_str(),
                metadata.os_version
            );
        }
    }

    // Distro mismatch (linux packages with a declared distro).
    if let Some(pkg_distro) = &header.distro {
        if let Some(host_distro) = &host.distro {
            if !pkg_distro.eq_ignore_ascii_case(host_distro) {
                if header.strict {
                    return Err(UpkgError::Reject(format!(
                        "package targets distro `{pkg_distro}` but this host is `{host_distro}` (strict)"
                    )));
                }
                eprintln!(
                    "warning: package targets distro `{pkg_distro}` but this host is `{host_distro}` - installing anyway"
                );
            }
        }
    }

    // Architecture mismatch.
    if let Some(pkg_arch) = &header.arch {
        if let Some(host_arch) = &host.arch {
            if pkg_arch != host_arch {
                if header.strict {
                    return Err(UpkgError::Reject(format!(
                        "package targets arch `{}` but this host is `{}` (strict)",
                        pkg_arch.as_str(),
                        host_arch.as_str()
                    )));
                }
                eprintln!(
                    "warning: package targets arch `{}` but this host is `{}` - installing anyway",
                    pkg_arch.as_str(),
                    host_arch.as_str()
                );
            }
        }
    }
    Ok(())
}

/// Conflicts and replaces (constraint 15): a conflict with an installed
/// package rejects; a replaced package is removed first.
pub fn check_conflicts_and_apply_replaces(metadata: &Metadata) -> Result<()> {
    for installed in crate::database::list_installed()? {
        if installed.app_name == metadata.app_name {
            continue;
        }
        if metadata.conflicts.contains(&installed.app_name) {
            return Err(UpkgError::Reject(format!(
                "package conflicts with installed package `{}`",
                installed.app_name
            )));
        }
    }
    for replaced in &metadata.replaces {
        if let Some(entry) = DatabaseEntry::load(replaced)? {
            if entry.app_name == metadata.app_name {
                continue;
            }
            eprintln!("replacing installed package `{replaced}`");
            remove_entry(&entry)?;
        }
    }
    Ok(())
}

/// Missing-dependency flow (revisions 16-17).
pub fn handle_dependencies(metadata: &Metadata) -> Result<()> {
    let missing = missing_dependencies(&metadata.dependencies)?;
    if missing.is_empty() {
        return Ok(());
    }

    eprintln!("warning: missing dependencies: {}", missing.join(", "));

    match host::detect().os {
        Some(Os::Windows) | Some(Os::Mac) => {
            // Windows (and mac, by proposal) warn and continue.
            eprintln!("warning: continuing without the missing dependencies");
            Ok(())
        }
        _ => {
            // Linux: interactive flow.
            eprintln!("warning: installing the extra dependencies may cause conflicts and system issues");
            if prompt::ask_yes_no("try installing the missing dependencies?")? {
                if let Some(ok) = run_system_install(&missing)? {
                    if ok {
                        eprintln!("note: system install finished");
                    } else {
                        eprintln!("warning: system install command failed");
                    }
                } else {
                    eprintln!("warning: no supported package manager found - cannot install dependencies");
                }
                Ok(())
            } else {
                eprintln!("warning: installing the package anyway might not work at all or properly");
                if prompt::ask_yes_no("install the package anyway without the dependencies?")? {
                    Ok(())
                } else {
                    Err(UpkgError::Aborted)
                }
            }
        }
    }
}

/// Which declared dependencies are missing (not installed, or installed at a
/// version outside the bounds - revision 29).
pub fn missing_dependencies(deps: &[crate::metadata::Dependency]) -> Result<Vec<String>> {
    let installed = crate::database::list_installed()?;
    let mut missing = Vec::new();
    for dep in deps {
        let satisfied = installed.iter().any(|e| {
            e.app_name == dep.name
                && version::in_range(&e.app_version, dep.min.as_deref(), dep.max.as_deref())
        });
        if !satisfied {
            missing.push(dep.to_dpkg());
        }
    }
    Ok(missing)
}

/// Run the system package manager for the missing dependencies (best effort,
/// distro detection via which package managers exist on PATH).
fn run_system_install(deps: &[String]) -> Result<Option<bool>> {
    let deps_ref: Vec<&str> = deps.iter().map(|s| s.as_str()).collect();
    let (manager, install_args): (&str, &[&str]) = if command_exists("apt-get") {
        ("apt-get", &["install", "-y"])
    } else if command_exists("dnf") {
        ("dnf", &["install", "-y"])
    } else if command_exists("pacman") {
        ("pacman", &["-S", "--noconfirm"])
    } else if command_exists("zypper") {
        ("zypper", &["install", "-y"])
    } else {
        return Ok(None);
    };
    let status = Command::new("sudo")
        .arg(manager)
        .args(install_args)
        .args(&deps_ref)
        .status()
        .map_err(|e| UpkgError::io_context(e, "cannot run the system package manager"))?;
    Ok(Some(status.success()))
}

fn command_exists(name: &str) -> bool {
    Command::new("sh")
        .arg("-c")
        .arg(format!("command -v {name} >/dev/null 2>&1"))
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Write every file (and folder) of the package under `install_path`.
pub fn extract_all(pkg: &mut Package, install_path: &Path) -> Result<()> {
    // Create folder entries first (idempotent).
    for folder in collect_folders(&pkg.tree) {
        let dir = entries::safe_join(install_path, &folder)?;
        std::fs::create_dir_all(&dir)
            .map_err(|e| UpkgError::io_context(e, "cannot create folder"))?;
    }

    match pkg.header.compression_kind {
        CompressionKind::PerFile => {
            let entries = pkg.file_entries_owned();
            for entry in &entries {
                let raw = pkg.read_entry_raw(entry)?;
                write_one(install_path, entry, &raw)?;
            }
        }
        CompressionKind::WholeArchive => {
            let extracted = package::whole_archive_extract(pkg)?;
            for (relative, bytes) in extracted {
                let entry = pkg
                    .file_entries()
                    .into_iter()
                    .find(|e| e.relative_path == relative)
                    .ok_or_else(|| UpkgError::Format("entry vanished from tree".into()))?;
                write_one(install_path, entry, &bytes)?;
            }
        }
    }
    Ok(())
}

fn write_one(install_path: &Path, entry: &entries::Entry, raw: &[u8]) -> Result<()> {
    let target = entries::safe_join(install_path, &entry.relative_path)?;
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| UpkgError::io_context(e, "cannot create folder"))?;
    }
    std::fs::write(&target, raw)
        .map_err(|e| UpkgError::io_context(e, "cannot write installed file"))?;
    apply_meta(&target, entry);
    Ok(())
}

/// Verify every file of a database entry on disk against its stored hash.
/// Returns true when all files verify.
pub fn verify_installed_files(entry: &DatabaseEntry, install_path: &Path) -> Result<bool> {
    for f in &entry.files {
        let path = install_path.join(&f.relative_path);
        let current = file_sha1(&path);
        if current.as_deref() != Some(f.original_sha1.as_str()) {
            eprintln!("warning: file `{}` failed verification", f.relative_path);
            return Ok(false);
        }
    }
    Ok(true)
}

/// Build the database entry from a package (file hashes come from the tree).
pub fn build_entry_from_tree(
    header: &Header,
    metadata: &Metadata,
    tree: &[entries::Entry],
    install_path: &Path,
) -> DatabaseEntry {
    let files = tree
        .iter()
        .flat_map(|e| e.files())
        .map(|e| InstalledFile {
            relative_path: e.relative_path.clone(),
            original_sha1: to_hex(&e.original_sha1.unwrap_or_default()),
        })
        .collect();
    DatabaseEntry {
        app_name: metadata.app_name.clone(),
        app_version: metadata.app_version.clone(),
        os: header.os.as_str().to_string(),
        os_version: header.os_version.clone(),
        install_path: install_path.display().to_string(),
        files,
        shortcut: false,
        shortcut_path: None,
        dependencies: metadata.dependencies.clone(),
        status: Status::Unpacked,
    }
}

/// Remove an installed package: its files, its database entry and its
/// generated shortcut (`upkg remove <app>`, Section 9).
pub fn remove_entry(entry: &DatabaseEntry) -> Result<()> {
    let base = PathBuf::from(&entry.install_path);
    for f in &entry.files {
        let path = base.join(&f.relative_path);
        if path.exists() {
            let _ = std::fs::remove_file(&path);
        }
    }
    // Remove empty parent folders left behind (best effort).
    let mut dirs: Vec<PathBuf> = entry
        .files
        .iter()
        .filter_map(|f| {
            let p = base.join(&f.relative_path);
            p.parent().map(|d| d.to_path_buf())
        })
        .collect();
    dirs.sort_by(|a, b| b.components().count().cmp(&a.components().count()));
    for d in dirs {
        let _ = std::fs::remove_dir(&d);
    }
    let _ = std::fs::remove_dir(&base);

    if let Some(sp) = &entry.shortcut_path {
        let sp = PathBuf::from(sp);
        if sp.exists() {
            std::fs::remove_file(&sp)
                .map_err(|e| UpkgError::io_context(e, "cannot remove shortcut"))?;
            println!("removed shortcut `{}`", sp.display());
        }
    }
    DatabaseEntry::remove(&entry.app_name)?;
    println!(
        "removed `{}` {} from `{}`",
        entry.app_name, entry.app_version, entry.install_path
    );
    Ok(())
}

/// Collect the relative paths of all folder entries.
fn collect_folders(tree: &[entries::Entry]) -> Vec<String> {
    let mut out = Vec::new();
    for e in tree {
        if e.kind == entries::EntryKind::Folder {
            out.push(e.relative_path.clone());
            out.extend(collect_folders(&e.children));
        }
    }
    out
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
pub fn apply_meta(path: &Path, entry: &entries::Entry) {
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

/// Make a shortcut executable on unix (the mac `.command` files).
#[allow(unused_variables)]
fn make_executable(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = std::fs::metadata(path) {
            let mut perms = meta.permissions();
            perms.set_mode(perms.mode() | 0o755);
            let _ = std::fs::set_permissions(path, perms);
        }
    }
}

/// Verify every file of the *package* (with package bytes available),
/// rewriting mismatched or missing files, then check the master SHA-1
/// (revision 29 transaction step 3, local installs).
pub fn verify_written_files(pkg: &mut Package, install_path: &Path) -> Result<()> {
    match pkg.header.compression_kind {
        CompressionKind::PerFile => {
            let entries = pkg.file_entries_owned();
            let mut master = sha1::Sha1::new();
            for entry in &entries {
                let target = entries::safe_join(install_path, &entry.relative_path)?;
                let expected = to_hex(&entry.original_sha1.unwrap_or_default());
                let actual = file_sha1(&target);
                if actual.as_deref() != Some(expected.as_str()) {
                    eprintln!("note: rewriting `{}`", entry.relative_path);
                    let raw = pkg.read_entry_raw(entry)?;
                    write_one(install_path, entry, &raw)?;
                }
                let raw = pkg.read_entry_raw(entry)?;
                master.update(&raw);
            }
            let computed: [u8; 20] = master.finalize().into();
            crate::hashes::check("master SHA-1", &pkg.hashes.master_sha1, &computed)?;
        }
        CompressionKind::WholeArchive => {
            let extracted = package::whole_archive_extract(pkg)?;
            for (relative, bytes) in extracted {
                let target = entries::safe_join(install_path, &relative)?;
                if file_sha1(&target).as_deref() != Some(&to_hex(&sha1(&bytes))) {
                    eprintln!("note: rewriting `{relative}`");
                    let entry = pkg
                        .file_entries()
                        .into_iter()
                        .find(|e| e.relative_path == relative)
                        .ok_or_else(|| UpkgError::Format("entry vanished from tree".into()))?;
                    write_one(install_path, entry, &bytes)?;
                }
            }
        }
    }
    Ok(())
}
