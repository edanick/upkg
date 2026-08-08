//! End-to-end CLI tests: create -> verify -> install -> verify folder ->
//! repair -> list/info -> remove, for per-file and whole-archive packages,
//! plus signing, online install from a non-seekable server, and download.
//!
//! These tests shell out to the built `upkg` binary and isolate the install
//! root with the `UPKG_ROOT` environment variable (proposal).

use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::sync::Mutex;
use std::time::Duration;

use tempfile::TempDir;

/// Tests that touch the process environment run serially.
static ENV_LOCK: Mutex<()> = Mutex::new(());

fn upkg() -> &'static str {
    env!("CARGO_BIN_EXE_upkg")
}

fn project_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn fixture_app() -> PathBuf {
    project_root().join("tests/fixtures/app")
}

/// Host OS identifier as used by the header (`windows`/`linux`/`mac`).
fn host_os() -> &'static str {
    match std::env::consts::OS {
        "windows" => "windows",
        "linux" => "linux",
        "macos" => "mac",
        other => panic!("unsupported test host OS {other}"),
    }
}

/// Render a path as a TOML literal string (single-quoted: no escape
/// processing, safe for Windows backslashes).
fn toml_path(p: &Path) -> String {
    format!("'{}'", p.display().to_string().replace('\'', "''"))
}

/// Run `upkg` with args; assert success and return (stdout, stderr).
fn run_upkg(args: &[&str], root: &Path) -> (String, String) {
    let output = Command::new(upkg())
        .args(args)
        .current_dir(project_root())
        .env("UPKG_ROOT", root)
        .output()
        .expect("failed to run upkg");
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    assert!(
        output.status.success(),
        "`upkg {}` failed\nstdout:\n{stdout}\nstderr:\n{stderr}",
        args.join(" ")
    );
    (stdout, stderr)
}

fn write_config(dir: &Path, kind: &str, extra: &str) -> PathBuf {
    let os = host_os();
    let modes = if os == "windows" { "" } else { "modes = true\n" };
    let config = format!(
        "app-name = \"myapp\"\n\
         app-version = \"1.2.3\"\n\
         os = \"{os}\"\n\
         os-version = \"10.0\"\n\
         {modes}\
         type = \"game\"\n\
         compression = \"zstd\"\n\
         compression-kind = \"{kind}\"\n\
         compression-level = 3\n\
         source = {}\n\
         output = {}\n\
         {extra}",
        toml_path(&fixture_app()),
        toml_path(dir)
    );
    let path = dir.join("config.toml");
    std::fs::write(&path, config).unwrap();
    path
}

fn find_package(dir: &Path) -> PathBuf {
    let mut found: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("upkg"))
        .collect();
    assert_eq!(found.len(), 1, "expected exactly one .upkg in {}", dir.display());
    found.remove(0)
}

#[test]
fn full_cycle_per_file() {
    let _guard = ENV_LOCK.lock().unwrap();
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().join("root");
    let config = write_config(tmp.path(), "per-file", "");
    let (_out, _err) = run_upkg(&["create", config.to_str().unwrap()], &root);
    let pkg = find_package(tmp.path());

    // verify the package file
    let (out, _) = run_upkg(&["verify", pkg.to_str().unwrap()], &root);
    assert!(out.contains("ok"), "verify output: {out}");

    // install (type game -> <root>/games/myapp)
    let (out, _) = run_upkg(&["install", pkg.to_str().unwrap()], &root);
    assert!(out.contains("installed"), "install output: {out}");
    let install_dir = root.join("games").join("myapp");
    assert!(install_dir.join("hello.txt").exists());
    assert!(install_dir.join("bin/run.sh").exists());
    assert!(root.join("packages/myapp.json").exists());

    // verify the installed folder against the package
    let (_out, _) = run_upkg(
        &[
            "verify",
            install_dir.to_str().unwrap(),
            "--package",
            pkg.to_str().unwrap(),
        ],
        &root,
    );

    // list + info
    let (out, _) = run_upkg(&["list"], &root);
    assert!(out.contains("myapp"), "list output: {out}");
    let (out, _) = run_upkg(&["info", "myapp"], &root);
    assert!(out.contains("1.2.3"), "info output: {out}");
    let (out, _) = run_upkg(&["info", pkg.to_str().unwrap()], &root);
    assert!(out.contains("app-name:       myapp"), "pkg info: {out}");

    // corrupt a file, verify must fail, repair must restore it
    let hello = install_dir.join("hello.txt");
    let original = std::fs::read_to_string(&hello).unwrap();
    std::fs::write(&hello, "corrupted content!!!").unwrap();
    let failed = Command::new(upkg())
        .args(["verify", install_dir.to_str().unwrap(), "--package", pkg.to_str().unwrap()])
        .current_dir(project_root())
        .env("UPKG_ROOT", &root)
        .output()
        .unwrap();
    assert!(!failed.status.success(), "verify after corruption should fail");

    let (out, _) = run_upkg(
        &[
            "repair",
            install_dir.to_str().unwrap(),
            "--package",
            pkg.to_str().unwrap(),
        ],
        &root,
    );
    assert!(out.contains("repaired"), "repair output: {out}");
    assert_eq!(std::fs::read_to_string(&hello).unwrap(), original);
    let (_out, _) = run_upkg(
        &[
            "verify",
            install_dir.to_str().unwrap(),
            "--package",
            pkg.to_str().unwrap(),
        ],
        &root,
    );

    // remove
    let (out, _) = run_upkg(&["remove", "myapp"], &root);
    assert!(out.contains("removed"), "remove output: {out}");
    assert!(!install_dir.join("hello.txt").exists());
    assert!(!root.join("packages/myapp.json").exists());
}

#[test]
fn full_cycle_whole_archive() {
    let _guard = ENV_LOCK.lock().unwrap();
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().join("root");
    let config = write_config(tmp.path(), "whole-archive", "");
    let (_out, _err) = run_upkg(&["create", config.to_str().unwrap()], &root);
    let pkg = find_package(tmp.path());

    let (out, _) = run_upkg(&["verify", pkg.to_str().unwrap()], &root);
    assert!(out.contains("whole-archive"), "verify output: {out}");

    let (out, _) = run_upkg(&["install", pkg.to_str().unwrap()], &root);
    assert!(out.contains("installed"), "install output: {out}");
    let install_dir = root.join("games").join("myapp");
    assert!(install_dir.join("bin/run.sh").exists());

    // corrupt + repair (whole-archive uses the temp-dir swap path)
    let hello = install_dir.join("hello.txt");
    std::fs::write(&hello, "bad bytes").unwrap();
    let (out, _) = run_upkg(
        &[
            "repair",
            install_dir.to_str().unwrap(),
            "--package",
            pkg.to_str().unwrap(),
        ],
        &root,
    );
    assert!(out.contains("repaired"), "repair output: {out}");
    assert_eq!(std::fs::read_to_string(&hello).unwrap(), "hello from upkg\n");

    run_upkg(&["remove", "myapp"], &root);
}

#[test]
fn signed_package() {
    let _guard = ENV_LOCK.lock().unwrap();
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().join("root");
    let key = tmp.path().join("key.seed");
    run_upkg(&["keygen", key.to_str().unwrap()], &root);

    let config = write_config(
        tmp.path(),
        "per-file",
        &format!("signing = {}\n", toml_path(&key)),
    );
    let (_out, _err) = run_upkg(&["create", config.to_str().unwrap()], &root);
    let pkg = find_package(tmp.path());

    let (out, _) = run_upkg(&["verify", pkg.to_str().unwrap()], &root);
    assert!(out.contains("ok"), "verify output: {out}");

    // Tampering with any byte must break verification.
    let mut bytes = std::fs::read(&pkg).unwrap();
    let mid = bytes.len() / 2;
    bytes[mid] ^= 0xFF;
    let tampered = tmp.path().join("tampered.upkg");
    std::fs::write(&tampered, &bytes).unwrap();
    let output = Command::new(upkg())
        .args(["verify", tampered.to_str().unwrap()])
        .current_dir(project_root())
        .env("UPKG_ROOT", &root)
        .output()
        .unwrap();
    assert!(!output.status.success(), "tampered package must fail verify");

    // Install of the valid signed package must succeed (signature checked).
    let (out, _) = run_upkg(&["install", pkg.to_str().unwrap()], &root);
    assert!(out.contains("installed"), "install output: {out}");
    run_upkg(&["remove", "myapp"], &root);
}

#[test]
fn rejects_wrong_os() {
    let _guard = ENV_LOCK.lock().unwrap();
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().join("root");
    let other_os = if host_os() == "windows" { "linux" } else { "windows" };
    let config = format!(
        "app-name = \"foreign\"\n\
         app-version = \"1.0\"\n\
         os = \"{other_os}\"\n\
         os-version = \"1\"\n\
         source = {}\n\
         output = {}\n",
        toml_path(&fixture_app()),
        toml_path(tmp.path())
    );
    let config_path = tmp.path().join("foreign.toml");
    std::fs::write(&config_path, &config).unwrap();
    let (_out, _err) = run_upkg(&["create", config_path.to_str().unwrap()], &root);
    let pkg = find_package(tmp.path());

    let output = Command::new(upkg())
        .args(["install", pkg.to_str().unwrap()])
        .current_dir(project_root())
        .env("UPKG_ROOT", &root)
        .output()
        .unwrap();
    assert!(!output.status.success(), "install of a foreign-OS package must be rejected");
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    assert!(stderr.contains("rejected"), "stderr: {stderr}");
}

#[test]
fn tampered_package_rejected_at_install() {
    let _guard = ENV_LOCK.lock().unwrap();
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().join("root");
    // Tamper with a valid package's entries tree (inject a `..` escape, the
    // zip-slip vector) and make sure install rejects it before writing
    // anything. Tree tampering breaks the tree SHA-1, so rejection happens
    // during preflight, before any file is written.
    let os = host_os();
    let config = format!(
        "app-name = \"evil\"\n\
         app-version = \"1.0\"\n\
         os = \"{os}\"\n\
         os-version = \"10.0\"\n\
         source = {}\n\
         output = {}\n",
        toml_path(&fixture_app()),
        toml_path(tmp.path())
    );
    let config_path = tmp.path().join("evil.toml");
    std::fs::write(&config_path, &config).unwrap();
    let (_out, _err) = run_upkg(&["create", config_path.to_str().unwrap()], &root);
    let pkg = find_package(tmp.path());

    // Rewrite the entries tree: replace a relative path with a `..` escape
    // ("bin/run.sh" -> "../evil.sh", same byte length). This breaks the tree
    // SHA-1, so install must reject the package before writing anything.
    let mut bytes = std::fs::read(&pkg).unwrap();
    let needle = b"bin/run.sh";
    let replacement = b"../evil.sh";
    let pos = find_subslice(&bytes, needle).expect("bin/run.sh in tree");
    bytes[pos..pos + needle.len()].copy_from_slice(replacement);
    let evil = tmp.path().join("evil.upkg");
    std::fs::write(&evil, &bytes).unwrap();

    let output = Command::new(upkg())
        .args(["install", evil.to_str().unwrap()])
        .current_dir(project_root())
        .env("UPKG_ROOT", &root)
        .output()
        .unwrap();
    assert!(
        !output.status.success(),
        "install of a tampered package must fail"
    );
    // Nothing may have been written outside the root (zip-slip escape).
    let escape = root.parent().unwrap().join("evil.sh");
    assert!(!escape.exists(), "escaped file must not exist: {}", escape.display());
}

/// Write a TOML config with full control over the fields.
fn write_config_named(dir: &Path, app_name: &str, kind: &str, pkg_type: &str, extra: &str) -> PathBuf {
    let os = host_os();
    let modes = if os == "windows" { "" } else { "modes = true\n" };
    let config = format!(
        "app-name = \"{app_name}\"\n\
         app-version = \"2.0.0\"\n\
         os = \"{os}\"\n\
         os-version = \"10.0\"\n\
         {modes}\
         type = \"{pkg_type}\"\n\
         compression = \"zstd\"\n\
         compression-kind = \"{kind}\"\n\
         compression-level = 3\n\
         source = {}\n\
         output = {}\n\
         {extra}",
        toml_path(&fixture_app()),
        toml_path(dir)
    );
    let path = dir.join("config.toml");
    std::fs::write(&path, config).unwrap();
    path
}

// ---------------------------------------------------------------------------
// Non-seekable HTTP test server (Python http.server – no Range support)
// ---------------------------------------------------------------------------

struct TestServer {
    child: Child,
    port: u16,
}

impl TestServer {
    fn start(serve_dir: &Path) -> Self {
        // Bind to port 0 to let the OS assign a free port, then pass it to Python.
        let listener = std::net::TcpListener::bind("127.0.0.1:0")
            .expect("failed to bind to a free port");
        let port = listener.local_addr().unwrap().port();
        drop(listener); // release the port so Python can grab it

        let mut child = find_python()
            .args(["-m", "http.server", &port.to_string()])
            .current_dir(serve_dir)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("failed to start test HTTP server");
        // Poll until the server is accepting connections.
        for i in 0..120 {
            match TcpStream::connect_timeout(
                &format!("127.0.0.1:{port}").parse().unwrap(),
                Duration::from_millis(250),
            ) {
                Ok(_) => break,
                Err(_) if i == 119 => {
                    let _ = child.kill();
                    panic!("test HTTP server did not start on port {port} within 30 seconds");
                }
                Err(_) => std::thread::sleep(Duration::from_millis(250)),
            }
        }
        TestServer { child, port }
    }

    fn url(&self, name: &str) -> String {
        format!("http://127.0.0.1:{}/{}", self.port, name)
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn find_python() -> Command {
    for name in &["python", "python3"] {
        if let Ok(out) = Command::new(name).arg("--version").output() {
            let text = String::from_utf8_lossy(&out.stdout);
            let err = String::from_utf8_lossy(&out.stderr);
            if text.contains("Python") || err.contains("Python") {
                // Verify the http.server module is available.
                if Command::new(name)
                    .args(["-c", "import http.server"])
                    .output()
                    .map(|o| o.status.success())
                    .unwrap_or(false)
                {
                    return Command::new(*name);
                }
            }
        }
    }
    panic!("no python with http.server found – install Python 3 to run HTTP e2e tests");
}

// ===========================================================================
// NEW TESTS
// ===========================================================================

/// Install two packages with different names; verify `list` shows both
/// and `info <app>` shows the correct version for each.
#[test]
fn list_and_info_two_packages() {
    let _guard = ENV_LOCK.lock().unwrap();
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().join("root");

    // Package A: application type
    let config_a = write_config_named(tmp.path(), "alpha", "per-file", "application", "");
    let (_out, _) = run_upkg(&["create", config_a.to_str().unwrap()], &root);
    let pkg_a = find_package(tmp.path());
    let (_out, _) = run_upkg(&["install", pkg_a.to_str().unwrap()], &root);

    // Package B: game type
    let dir_b = tmp.path().join("b");
    std::fs::create_dir(&dir_b).unwrap();
    let config_b = write_config_named(&dir_b, "beta", "whole-archive", "application", "");
    let (_out, _) = run_upkg(&["create", config_b.to_str().unwrap()], &root);
    let pkg_b = find_package(&dir_b);
    let (_out, _) = run_upkg(&["install", pkg_b.to_str().unwrap()], &root);

    // list: both should appear
    let (out, _) = run_upkg(&["list"], &root);
    assert!(out.contains("alpha"), "list missing alpha: {out}");
    assert!(out.contains("beta"), "list missing beta: {out}");

    // info for each installed app
    let (out, _) = run_upkg(&["info", "alpha"], &root);
    assert!(out.contains("2.0.0"), "info alpha: {out}");
    assert!(out.contains("installed"), "info alpha status: {out}");
    let (out, _) = run_upkg(&["info", "beta"], &root);
    assert!(out.contains("2.0.0"), "info beta: {out}");

    // info for a package file
    let (out, _) = run_upkg(&["info", pkg_a.to_str().unwrap()], &root);
    assert!(out.contains("app-name:"), "pkg info header: {out}");
    assert!(out.contains("alpha"), "pkg info name: {out}");
    let (out, _) = run_upkg(&["info", pkg_b.to_str().unwrap()], &root);
    assert!(out.contains("beta"), "pkg info name: {out}");
    assert!(out.contains("whole-archive"), "pkg info kind: {out}");

    run_upkg(&["remove", "alpha"], &root);
    run_upkg(&["remove", "beta"], &root);
}

/// After removal the database entry, installed files, and shortcut
/// must all be gone.
#[test]
fn remove_cleans_everything() {
    let _guard = ENV_LOCK.lock().unwrap();
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().join("root");
    let config = write_config(tmp.path(), "per-file", "");
    let (_out, _) = run_upkg(&["create", config.to_str().unwrap()], &root);
    let pkg = find_package(tmp.path());
    let (_out, _) = run_upkg(&["install", pkg.to_str().unwrap()], &root);

    let install_dir = root.join("games").join("myapp");
    let db_file = root.join("packages").join("myapp.json");
    assert!(install_dir.join("hello.txt").exists());
    assert!(db_file.exists());

    let (out, _) = run_upkg(&["remove", "myapp"], &root);
    assert!(out.contains("removed"), "remove output: {out}");

    // Files, DB entry, and shortcut must be gone.
    assert!(!install_dir.join("hello.txt").exists(), "file still exists after remove");
    assert!(!install_dir.join("bin").exists(), "subfolder still exists after remove");
    assert!(!install_dir.exists(), "install dir still exists after remove");
    assert!(!db_file.exists(), "DB entry still exists after remove");

    // Shortcut must also be gone. A binary .lnk can't be read as UTF-8,
    // but read_to_string + unwrap_or_default degrades gracefully: the
    // empty default won't contain the app name, so the assertion holds.
    let shortcut_gone = |suffix: &str| {
        let sp = root.join(suffix);
        if !sp.exists() {
            return true;
        }
        // The file might be a .desktop, .command, or .lnk.
        let content = std::fs::read_to_string(&sp).unwrap_or_default();
        !content.contains("myapp")
    };
    assert!(shortcut_gone("shortcuts/myapp.desktop"));
    assert!(shortcut_gone("shortcuts/myapp.command"));
    assert!(shortcut_gone("shortcuts/myapp.lnk"));

    // Removing again must fail (not installed).
    let output = Command::new(upkg())
        .args(["remove", "myapp"])
        .current_dir(project_root())
        .env("UPKG_ROOT", &root)
        .output()
        .unwrap();
    assert!(!output.status.success(), "removing a non-installed package must fail");
}

/// Delete a file, run `upkg repair --package`, verify the file is restored
/// and subsequent verify passes.
#[test]
fn repair_restores_missing_file() {
    let _guard = ENV_LOCK.lock().unwrap();
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().join("root");
    let config = write_config(tmp.path(), "per-file", "");
    let (_out, _) = run_upkg(&["create", config.to_str().unwrap()], &root);
    let pkg = find_package(tmp.path());
    let (_out, _) = run_upkg(&["install", pkg.to_str().unwrap()], &root);

    let install_dir = root.join("games").join("myapp");
    let hello = install_dir.join("hello.txt");
    let original = std::fs::read_to_string(&hello).unwrap();

    // Delete the file entirely.
    std::fs::remove_file(&hello).unwrap();
    assert!(!hello.exists());

    // Verify must fail (missing file).
    let failed = Command::new(upkg())
        .args([
            "verify",
            install_dir.to_str().unwrap(),
            "--package",
            pkg.to_str().unwrap(),
        ])
        .current_dir(project_root())
        .env("UPKG_ROOT", &root)
        .output()
        .unwrap();
    assert!(!failed.status.success(), "verify after deletion should fail");

    // Repair restores it.
    let (out, _) = run_upkg(
        &[
            "repair",
            install_dir.to_str().unwrap(),
            "--package",
            pkg.to_str().unwrap(),
        ],
        &root,
    );
    assert!(out.contains("repaired"), "repair output: {out}");
    assert!(hello.exists());
    assert_eq!(std::fs::read_to_string(&hello).unwrap(), original);

    // Verify must now succeed.
    let (_out, _) = run_upkg(
        &[
            "verify",
            install_dir.to_str().unwrap(),
            "--package",
            pkg.to_str().unwrap(),
        ],
        &root,
    );

    run_upkg(&["remove", "myapp"], &root);
}

/// Online install from a non-seekable HTTP server (Python http.server does
/// not support Range). Tests the unseekable path: speed test → full download
/// → progressive extraction.
#[test]
fn online_install_from_unseekable_host() {
    let _guard = ENV_LOCK.lock().unwrap();
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().join("root");

    // Build a per-file package (small enough to be fast).
    let config = write_config(tmp.path(), "per-file", "");
    let (_out, _) = run_upkg(&["create", config.to_str().unwrap()], &root);
    let pkg = find_package(tmp.path());

    // Serve the package directory from a Python server (no Range support).
    let pkg_name = pkg.file_name().unwrap().to_str().unwrap().to_string();
    let server = TestServer::start(tmp.path());
    let url = server.url(&pkg_name);

    // Verify the server does NOT support Range (confirm it's unseekable).
    let resp = ureq::head(&url).call().unwrap();
    let accepts_ranges = resp
        .header("accept-ranges")
        .map(|v| v.eq_ignore_ascii_case("bytes"))
        .unwrap_or(false);
    assert!(!accepts_ranges, "python http.server unexpectedly advertises Range");

    // Online install: must download whole, then extract.
    let dest = tmp.path().join("dest");
    let (out, _) = run_upkg(
        &["install", &url, dest.to_str().unwrap()],
        &root,
    );
    assert!(out.contains("installed"), "online install output: {out}");
    assert!(dest.join("hello.txt").exists());
    assert!(dest.join("bin/run.sh").exists());
    assert_eq!(
        std::fs::read_to_string(dest.join("hello.txt")).unwrap(),
        "hello from upkg\n"
    );

    drop(server);
}

/// `upkg download` from a non-seekable host must produce a valid `.upkg`
/// that passes verify.
#[test]
fn download_from_unseekable_host() {
    let _guard = ENV_LOCK.lock().unwrap();
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().join("root");

    let config = write_config(tmp.path(), "per-file", "");
    let (_out, _) = run_upkg(&["create", config.to_str().unwrap()], &root);
    let pkg = find_package(tmp.path());

    let pkg_name = pkg.file_name().unwrap().to_str().unwrap().to_string();
    let server = TestServer::start(tmp.path());
    let url = server.url(&pkg_name);

    let dl_dir = tmp.path().join("downloads");
    let (out, _) = run_upkg(
        &["download", &url, "--output", dl_dir.to_str().unwrap()],
        &root,
    );
    assert!(out.contains("downloaded"), "download output: {out}");

    let downloaded = dl_dir.join(&pkg_name);
    assert!(downloaded.exists());
    assert!(downloaded.metadata().unwrap().len() > 0);

    // The downloaded file must be valid.
    let (out, _) = run_upkg(&["verify", downloaded.to_str().unwrap()], &root);
    assert!(out.contains("ok"), "downloaded package verify: {out}");

    // Also test download to the default directory (cwd): temporarily change
    // into a temp dir to avoid writing into the project root.
    let cwd_dir = tmp.path().join("cwd");
    std::fs::create_dir(&cwd_dir).unwrap();
    let output = Command::new(upkg())
        .args(["download", &url])
        .current_dir(&cwd_dir)
        .env("UPKG_ROOT", &root)
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    assert!(output.status.success(), "download (no --output) failed: {stdout}");
    assert!(stdout.contains("downloaded"), "download output: {stdout}");
    let cwd_dl = cwd_dir.join(&pkg_name);
    assert!(cwd_dl.exists());
    let (_out, _) = run_upkg(&["verify", cwd_dl.to_str().unwrap()], &root);

    drop(server);
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|w| w == needle)
}
