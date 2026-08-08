//! UPKG - Universal Package Format & CLI (Section 9 of the spec).
//!
//! Commands:
//! - `upkg create <config>`            build a package from a TOML config
//! - `upkg install <file.upkg>`        install from a local file
//! - `upkg install <url> <folder>`     online (streaming) install
//! - `upkg verify <file.upkg>`         verify the package file itself
//! - `upkg verify <folder> --package`  verify an extracted folder
//! - `upkg repair <folder> --package`  restore corrupt/missing files
//! - `upkg info <file.upkg>|<app>`     print package or installed-app info
//! - `upkg remove <app>`               remove an installed application
//! - `upkg download <url> [--output]`  download a package without installing
//! - `upkg list`                       list installed packages
//! - `upkg keygen <output>`            generate an ed25519 signing key (proposal)

mod compress;
mod config;
mod create;
mod database;
mod entries;
mod error;
mod hashes;
mod header;
mod host;
mod install;
mod metadata;
mod online;
mod package;
mod paths;
mod prompt;
mod repair;
mod shortcut;
mod signature;
mod util;
mod verify;
mod version;

use std::path::PathBuf;

use clap::{Parser, Subcommand};

use crate::error::Result;

#[derive(Parser)]
#[command(
    name = "upkg",
    version,
    about = "UPKG - Universal Package Format & CLI"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Build a .upkg package from a TOML config file
    Create { config: PathBuf },

    /// Install a package from a local file or a URL (URLs take a folder)
    Install {
        /// A local `.upkg` file or an `http(s)://` URL
        target: String,
        /// Target folder (required for URL installs)
        folder: Option<PathBuf>,
    },

    /// Verify a package file or an extracted folder
    Verify {
        /// A `.upkg` file or an extracted folder
        target: PathBuf,
        /// When verifying a folder: the package to check against
        #[arg(long)]
        package: Option<PathBuf>,
    },

    /// Restore corrupt and missing files in an extracted folder
    Repair {
        /// The extracted folder
        folder: PathBuf,
        /// The package to restore from (defaults to the database)
        #[arg(long)]
        package: Option<PathBuf>,
    },

    /// Print a package's metadata and header, or an installed app's info
    Info { target: Option<String> },

    /// Remove an installed application (files, database entry, shortcut)
    Remove { app: String },

    /// Download a `.upkg` to disk without installing
    Download {
        url: String,
        /// Output directory (default: the current directory)
        #[arg(long)]
        output: Option<PathBuf>,
    },

    /// List installed packages
    List,

    /// Generate an ed25519 signing key (proposal - not in the spec)
    Keygen { output: PathBuf },
}

fn main() {
    let cli = Cli::parse();
    if let Err(e) = run(cli) {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

fn run(cli: Cli) -> Result<()> {
    match cli.command {
        Command::Create { config } => {
            let cfg = config::CreateConfig::load(&config)?;
            let path = create::create(&cfg)?;
            println!("package written to `{}`", path.display());
        }
        Command::Install { target, folder } => {
            if target.starts_with("http://") || target.starts_with("https://") {
                let folder = folder.ok_or_else(|| {
                    error::UpkgError::Config(
                        "online install requires a target folder: `upkg install <url> <folder>`"
                            .into(),
                    )
                })?;
                online::install_from_url(&target, &folder)?;
            } else {
                let path = std::path::Path::new(&target);
                match folder {
                    Some(folder) => {
                        let mut pkg = package::open(path)?;
                        install::install_package(&mut pkg, Some(&folder))?;
                    }
                    None => install::install_local(path)?,
                }
            }
        }
        Command::Verify { target, package } => {
            if target.is_dir() {
                verify_folder(&target, package.as_deref())?;
            } else if target.is_file() {
                verify::verify_package_file(&target)?;
            } else {
                return Err(error::UpkgError::Config(format!(
                    "`{}` does not exist (expected a .upkg file or a folder)",
                    target.display()
                )));
            }
        }
        Command::Repair { folder, package } => {
            if !folder.is_dir() {
                return Err(error::UpkgError::Config(format!(
                    "folder `{}` does not exist",
                    folder.display()
                )));
            }
            match package {
                Some(pkg_path) => {
                    let mut pkg = package::open(&pkg_path)?;
                    repair::repair_folder(&folder, &mut pkg)?;
                    repair::complete_after_repair(&folder)?;
                }
                None => {
                    // Use the database: complete an interrupted install or
                    // verify the folder against the recorded hashes.
                    let entry = database::find_by_install_path(&folder)?.ok_or_else(|| {
                        error::UpkgError::Config(format!(
                            "no installed package found at `{}` (pass --package)",
                            folder.display()
                        ))
                    })?;
                    if entry.status == database::Status::Unpacked {
                        repair::complete_entry(&entry)?;
                        eprintln!("note: repair requires the package to restore corrupt files");
                    } else {
                        let check = verify::verify_folder_against_entry(&folder, &entry)?;
                        verify::report_folder(&folder, &check, "the package database")?;
                    }
                }
            }
        }
        Command::Info { target } => match target {
            None => list_installed()?,
            Some(t) => info(&t)?,
        },
        Command::Remove { app } => {
            let entry = database::DatabaseEntry::load(&app)?
                .ok_or_else(|| error::UpkgError::Config(format!("`{app}` is not installed")))?;
            install::remove_entry(&entry)?;
        }
        Command::Download { url, output } => {
            online::download(&url, output.as_deref())?;
        }
        Command::List => list_installed()?,
        Command::Keygen { output } => {
            keygen(&output)?;
        }
    }
    Ok(())
}

/// Verify a folder against a package or the database.
fn verify_folder(folder: &std::path::Path, package: Option<&std::path::Path>) -> Result<()> {
    if let Some(pkg_path) = package {
        let pkg = package::open(pkg_path)?;
        let check = verify::verify_folder_against_package(folder, &pkg)?;
        verify::report_folder(folder, &check, &format!("`{}`", pkg_path.display()))
    } else {
        let entry = database::find_by_install_path(folder)?.ok_or_else(|| {
            error::UpkgError::Config(format!(
                "no installed package found at `{}` (pass --package)",
                folder.display()
            ))
        })?;
        let check = verify::verify_folder_against_entry(folder, &entry)?;
        verify::report_folder(folder, &check, &format!("`{}`", entry.app_name))
    }
}

/// `upkg info` for a package file or an installed app.
fn info(target: &str) -> Result<()> {
    let path = std::path::Path::new(target);
    if path.is_file() {
        let pkg = package::open(path)?;
        print!("{}", package::describe(&pkg));
        return Ok(());
    }
    // Not a file: treat it as an installed app name.
    let entry = database::DatabaseEntry::load(target)?
        .ok_or_else(|| error::UpkgError::Config(format!("`{target}` is neither a file nor an installed package")))?;
    print_db_entry(&entry);
    Ok(())
}

fn print_db_entry(entry: &database::DatabaseEntry) {
    println!("app-name:       {}", entry.app_name);
    println!("app-version:    {}", entry.app_version);
    println!("os:             {}", entry.os);
    println!("os-version:     {}", entry.os_version);
    println!("install-path:   {}", entry.install_path);
    println!("files:          {}", entry.files.len());
    println!("shortcut:       {}", entry.shortcut);
    if let Some(sp) = &entry.shortcut_path {
        println!("shortcut-path:  {sp}");
    }
    if !entry.dependencies.is_empty() {
        let deps: Vec<String> = entry.dependencies.iter().map(|d| d.to_dpkg()).collect();
        println!("dependencies:   {}", deps.join(", "));
    }
    println!("status:         {}", entry.status.as_str());
}

fn list_installed() -> Result<()> {
    let entries = database::list_installed()?;
    if entries.is_empty() {
        println!("no packages installed");
        return Ok(());
    }
    println!("{:<24} {:<16} {:<10} {:<10} {}", "app", "version", "os", "status", "install path");
    for e in entries {
        println!(
            "{:<24} {:<16} {:<10} {:<10} {}",
            e.app_name,
            e.app_version,
            e.os,
            e.status.as_str(),
            e.install_path
        );
    }
    Ok(())
}

/// Generate an ed25519 signing key (proposal): writes the 32-byte seed as
/// hex, prints the matching public key.
fn keygen(output: &std::path::Path) -> Result<()> {
    let seed = signature::generate_seed();
    let section = signature::sign(b"", &seed);
    std::fs::write(output, format!("{}\n", util::to_hex(&seed)))
        .map_err(|e| error::UpkgError::io_context(e, "cannot write key file"))?;
    println!(
        "wrote private key to `{}` (public key: {})",
        output.display(),
        util::to_hex(&section.public_key)
    );
    Ok(())
}
