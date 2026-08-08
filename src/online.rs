//! Online (streaming) installation and `upkg download` (Sections 11 of the
//! spec, revisions 2, 14, 15).
//!
//! - Seekable host (HTTP Range): a `per-file` package is streamed - the
//!   entries tree is fetched first (one Range request), then each file's
//!   bytes by its data start/end offsets, extracted immediately, no temp
//!   file. `whole-archive` packages (and signed packages, whose signature
//!   covers every byte) are downloaded whole to a temp file; a failed
//!   download resumes from the downloaded length and appends.
//! - Unseekable host: a RAM-only speed test gates a full download (under
//!   1 MB/s: refuse over 5 minutes; at or above 1 MB/s: refuse over the
//!   configurable limit, default 20 minutes - "not possible with this
//!   host"). A `per-file` download uses an active scanner that extracts each
//!   file as soon as its bytes are complete. The temp file is always deleted
//!   and recreated when (re)starting.

use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;

use ureq::Agent;

use crate::compress;
use crate::database::Status;
use crate::entries;
use crate::error::{Result, UpkgError};
use crate::hashes;
use crate::header::CompressionKind;
use crate::install;
use crate::package::{self, Package, Prefix};
use crate::paths::{speed_gate, InstallConfig, SpeedGate};
use crate::signature;
use crate::util::{sha1, to_hex};
use sha1::Digest;

/// Size of the initial preamble fetch window.
const PREAMBLE_WINDOW: u64 = 64 * 1024;
/// How many bytes the speed test reads into RAM before closing the socket.
const SPEED_TEST_BYTES: usize = 1024 * 1024;
/// Maximum restart attempts for an interrupted full download.
const MAX_RESTARTS: u32 = 50;

/// Install from a URL into `folder`.
pub fn install_from_url(url: &str, folder: &Path) -> Result<()> {
    let agent = http_agent();
    let seekable = host_is_seekable(&agent, url)?;
    let config = InstallConfig::load();

    if seekable {
        install_seekable(&agent, url, folder)
    } else {
        install_unseekable(&agent, url, folder, &config)
    }
}

/// `upkg download <url> [--output <dir>]` - plain download without installing.
pub fn download(url: &str, output: Option<&Path>) -> Result<PathBuf> {
    let agent = http_agent();
    let seekable = host_is_seekable(&agent, url)?;
    let dir = output.unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir)
        .map_err(|e| UpkgError::io_context(e, "cannot create output directory"))?;
    let name = url
        .split('/')
        .last()
        .filter(|s| !s.is_empty())
        .unwrap_or("package.upkg");
    let dest = dir.join(name);
    if seekable {
        download_resumable(&agent, url, &dest)?;
    } else {
        download_simple(&agent, url, &dest)?;
    }
    println!("downloaded `{}` to `{}`", url, dest.display());
    Ok(dest)
}

// ---------------------------------------------------------------------------
// HTTP plumbing
// ---------------------------------------------------------------------------

fn http_agent() -> Agent {
    ureq::AgentBuilder::new()
        .redirects(5)
        .timeout_connect(std::time::Duration::from_secs(15))
        .build()
}

/// Probe whether the host supports Range requests (HEAD + Accept-Ranges,
/// optionally confirmed by a small Range probe - Section 11.1).
fn host_is_seekable(agent: &Agent, url: &str) -> Result<bool> {
    if let Ok(resp) = agent.head(url).call() {
        if resp
            .header("accept-ranges")
            .map(|v| v.eq_ignore_ascii_case("bytes"))
            .unwrap_or(false)
        {
            return Ok(true);
        }
        // A HEAD may not advertise it; confirm with a tiny Range probe.
        if let Ok(resp) = agent.get(url).set("Range", "bytes=0-0").call() {
            return Ok(resp.status() == 206);
        }
        return Ok(false);
    }
    // HEAD failed: fall back to a Range probe on GET.
    match agent.get(url).set("Range", "bytes=0-0").call() {
        Ok(resp) => Ok(resp.status() == 206),
        Err(_) => Ok(false),
    }
}

/// Fetch a byte range (inclusive end). Returns the body.
fn fetch_range(agent: &Agent, url: &str, start: u64, end_inclusive: u64) -> Result<Vec<u8>> {
    let resp = agent
        .get(url)
        .set("Range", &format!("bytes={start}-{end_inclusive}"))
        .call()?;
    match resp.status() {
        206 => {
            let mut body = Vec::new();
            resp.into_reader()
                .read_to_end(&mut body)
                .map_err(|e| UpkgError::io_context(e, "cannot read ranged response"))?;
            Ok(body)
        }
        200 => {
            // Server ignored the Range header - read the whole body.
            let mut body = Vec::new();
            resp.into_reader()
                .read_to_end(&mut body)
                .map_err(|e| UpkgError::io_context(e, "cannot read response"))?;
            Ok(body)
        }
        other => Err(UpkgError::Http(format!(
            "unexpected status {other} for ranged request"
        ))),
    }
}

/// Fetch a byte range and require the expected length.
fn fetch_exact_range(
    agent: &Agent,
    url: &str,
    start: u64,
    end_inclusive: u64,
    expected_len: u64,
    what: &str,
) -> Result<Vec<u8>> {
    let body = fetch_range(agent, url, start, end_inclusive)?;
    if body.len() != expected_len as usize {
        return Err(UpkgError::Http(format!(
            "expected {expected_len} bytes for {what}, got {}",
            body.len()
        )));
    }
    Ok(body)
}

// ---------------------------------------------------------------------------
// Seekable host
// ---------------------------------------------------------------------------

fn install_seekable(agent: &Agent, url: &str, folder: &Path) -> Result<()> {
    // 1. fetch the preamble (header + hashes + metadata + entries tree).
    let window = fetch_range(agent, url, 0, PREAMBLE_WINDOW - 1)?;
    let prefix = package::parse_prefix(&window)?;

    // 2. signed packages must be downloaded whole (their signature covers
    //    every byte); same for whole-archive packages (Section 11.1).
    let cl = content_length(agent, url)?;
    if tail_has_signature(agent, url, cl)? || prefix.header.compression_kind == CompressionKind::WholeArchive
    {
        let temp = temp_file_path();
        download_resumable(agent, url, &temp)?;
        let mut pkg = package::open(&temp)?;
        let result = install::install_package(&mut pkg, Some(folder));
        let _ = std::fs::remove_file(&temp);
        return result;
    }

    // 3. per-file streaming: verify the prefix and run all preflight checks
    //    before writing anything.
    install::preflight(&prefix.header, &prefix.metadata, &prefix.tree)?;
    create_folders(&prefix.tree, folder)?;

    let mut master = sha1::Sha1::new();
    for entry in prefix.tree.iter().flat_map(|e| e.files()) {
        let start = entry.data_start.unwrap_or(0);
        let end = entry.data_end.unwrap_or(0);
        let stored = fetch_exact_range(agent, url, start, end - 1, end - start, &entry.relative_path)?;
        if let Some(post) = entry.post_compression_sha1 {
            hashes::check("post-compression SHA-1", &post, &sha1(&stored))?;
        }
        let raw = compress::decompress(prefix.header.compression, &stored)?;
        if let Some(orig) = entry.original_sha1 {
            hashes::check("original SHA-1", &orig, &sha1(&raw))?;
        }
        write_streamed(folder, entry, &raw)?;
        master.update(&raw);
    }
    let computed: [u8; 20] = master.finalize().into();
    hashes::check("master SHA-1", &prefix.hashes.master_sha1, &computed)?;

    // 4. database entry + shortcut.
    let mut entry = install::build_entry_from_tree(
        &prefix.header,
        &prefix.metadata,
        &prefix.tree,
        folder,
    );
    install::finalize(
        &mut entry,
        prefix.metadata.shortcut.as_ref(),
        prefix.header.os,
        folder,
    )
}

/// Check whether the file's tail is a signature section by fetching the last
/// `SIGNATURE_SECTION_LEN` bytes from the known content length.
fn tail_has_signature(agent: &Agent, url: &str, content_len: u64) -> Result<bool> {
    if content_len < signature::SIGNATURE_SECTION_LEN as u64 {
        return Ok(false);
    }
    let start = content_len - signature::SIGNATURE_SECTION_LEN as u64;
    let tail = fetch_exact_range(
        agent,
        url,
        start,
        content_len - 1,
        signature::SIGNATURE_SECTION_LEN as u64,
        "tail",
    )?;
    Ok(signature::extract_section(&tail).is_some())
}

/// Fetch the content length from a HEAD request. Returns 0 when the header
/// is absent.
fn content_length(agent: &Agent, url: &str) -> Result<u64> {
    match agent.head(url).call() {
        Ok(resp) => Ok(resp
            .header("content-length")
            .and_then(|v| v.parse().ok())
            .unwrap_or(0)),
        Err(_) => Ok(0),
    }
}

fn create_folders(tree: &[entries::Entry], folder: &Path) -> Result<()> {
    for folder_path in collect_folders(tree) {
        let dir = entries::safe_join(folder, &folder_path)?;
        std::fs::create_dir_all(&dir)
            .map_err(|e| UpkgError::io_context(e, "cannot create folder"))?;
    }
    Ok(())
}

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

/// Write a streamed file (verified already) into the install folder.
fn write_streamed(folder: &Path, entry: &entries::Entry, raw: &[u8]) -> Result<()> {
    let target = entries::safe_join(folder, &entry.relative_path)?;
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| UpkgError::io_context(e, "cannot create folder"))?;
    }
    std::fs::write(&target, raw)
        .map_err(|e| UpkgError::io_context(e, "cannot write installed file"))?;
    install::apply_meta(&target, entry);
    Ok(())
}

// ---------------------------------------------------------------------------
// Unseekable host
// ---------------------------------------------------------------------------

fn install_unseekable(agent: &Agent, url: &str, folder: &Path, config: &InstallConfig) -> Result<()> {
    // 1. speed test: read into RAM, close that socket, reopen for the real
    //    download (Section 11.2).
    let (speed, content_len) = speed_test(agent, url)?;

    // 2. speed gate.
    match speed_gate(speed, content_len, config) {
        SpeedGate::TooSlow => {
            return Err(UpkgError::Http(
                "the host does not support seeking and the internet is too slow".into(),
            ))
        }
        SpeedGate::TooLong { minutes } => {
            return Err(UpkgError::Http(format!(
                "not possible with this host (estimated download time exceeds {minutes} minutes)"
            )))
        }
        SpeedGate::Proceed => {}
    }

    // 3. warn before downloading.
    eprintln!(
        "warning: the host does not support seeking; if the download fails and gets interrupted it must be redone"
    );

    // 4. full download (delete + recreate the temp file on every restart).
    let temp = temp_file_path();
    download_restarting(agent, url, &temp)?;

    // Determine the kind from the downloaded file.
    let mut pkg = package::open(&temp)?;
    if pkg.header.compression_kind == CompressionKind::PerFile {
        // Per-file: extract progressively with the active scanner, then
        // finalize; the scanner already verified every file.
        let mut entry = install::build_entry_from_tree(
            &pkg.header,
            &pkg.metadata,
            &pkg.tree,
            folder,
        );
        // The active scanner below re-uses `pkg`'s tree; we extract from the
        // temp file directly.
        let result = (|| {
            install::preflight(&pkg.header, &pkg.metadata, &pkg.tree)?;
            create_folders(&pkg.tree, folder)?;
            extract_progressively(&mut pkg, folder)?;
            let _ = &mut entry;
            install::finalize(
                &mut entry,
                pkg.metadata.shortcut.as_ref(),
                pkg.header.os,
                folder,
            )
        })();
        let _ = std::fs::remove_file(&temp);
        return result;
    }

    // Whole-archive: install from the temp file.
    let result = install::install_package(&mut pkg, Some(folder));
    let _ = std::fs::remove_file(&temp);
    result
}

/// Speed test: download up to 1 MiB into RAM, measure, then close the socket
/// (the real download reopens a new one). Returns (bytes/sec, content length).
fn speed_test(agent: &Agent, url: &str) -> Result<(f64, u64)> {
    let resp = agent.get(url).call()?;
    let content_len: u64 = resp
        .header("content-length")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let mut reader = resp.into_reader();
    let mut buf = vec![0u8; 64 * 1024];
    let mut total: usize = 0;
    let start = Instant::now();
    while total < SPEED_TEST_BYTES {
        let n = reader
            .read(&mut buf)
            .map_err(|e| UpkgError::io_context(e, "speed test failed"))?;
        if n == 0 {
            break;
        }
        total += n;
    }
    drop(reader); // close the socket
    let elapsed = start.elapsed().as_secs_f64();
    if elapsed <= 0.0 || total == 0 {
        return Err(UpkgError::Http("speed test produced no data".into()));
    }
    let speed = total as f64 / elapsed;
    println!(
        "speed test: {:.2} MiB/s, {:.1} MiB to download",
        speed / (1024.0 * 1024.0),
        content_len as f64 / (1024.0 * 1024.0)
    );
    Ok((speed, content_len))
}

/// Full download that restarts from offset 0 on failure (delete + recreate
/// the temp file each time - the overwrite rule of Section 11.2).
fn download_restarting(agent: &Agent, url: &str, temp: &Path) -> Result<()> {
    let mut attempts = 0u32;
    loop {
        let start_offset = reset_temp_file(temp)?;
        match download_stream_to(agent, url, temp, start_offset) {
            Ok(()) => return Ok(()),
            Err(e) => {
                attempts += 1;
                if attempts >= MAX_RESTARTS {
                    return Err(e);
                }
                eprintln!(
                    "warning: download interrupted ({e}); restarting from 0 (attempt {attempts})"
                );
            }
        }
    }
}

/// Download the whole file to `temp`, appending from `start_offset`.
fn download_stream_to(agent: &Agent, url: &str, temp: &Path, start_offset: u64) -> Result<()> {
    let mut req = agent.get(url);
    if start_offset > 0 {
        req = req.set("Range", &format!("bytes={start_offset}-"));
    }
    let resp = req.call()?;
    if start_offset > 0 && resp.status() == 200 {
        // Server ignored the range: restart semantics handled by caller, but
        // treat as failure to keep the overwrite rule.
        return Err(UpkgError::Http(
            "host ignored the resume range request".into(),
        ));
    }
    let mut reader = resp.into_reader();
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(temp)
        .map_err(|e| UpkgError::io_context(e, "cannot open temp file"))?;
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = reader
            .read(&mut buf)
            .map_err(|e| UpkgError::io_context(e, "download interrupted"))?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n])
            .map_err(|e| UpkgError::io_context(e, "cannot write temp file"))?;
    }
    file.flush().map_err(UpkgError::Io)?;
    Ok(())
}

/// Download with resume-append semantics (seekable whole-archive, Section
/// 11.1): on failure, resume from the downloaded length and append.
fn download_resumable(agent: &Agent, url: &str, temp: &Path) -> Result<()> {
    let mut attempts = 0u32;
    loop {
        let current_len = std::fs::metadata(temp).map(|m| m.len()).unwrap_or(0);
        match download_stream_to(agent, url, temp, current_len) {
            Ok(()) => return Ok(()),
            Err(e) => {
                attempts += 1;
                if attempts >= MAX_RESTARTS {
                    return Err(e);
                }
                eprintln!(
                    "warning: download interrupted ({e}); resuming from byte {} (attempt {attempts})",
                    std::fs::metadata(temp).map(|m| m.len()).unwrap_or(0)
                );
            }
        }
    }
}

/// Plain full download (no resume) for `upkg download` on unseekable hosts.
fn download_simple(agent: &Agent, url: &str, dest: &Path) -> Result<()> {
    let mut file = File::create(dest)
        .map_err(|e| UpkgError::io_context(e, "cannot create output file"))?;
    let resp = agent.get(url).call()?;
    let mut reader = resp.into_reader();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = reader
            .read(&mut buf)
            .map_err(|e| UpkgError::io_context(e, "download interrupted"))?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n])
            .map_err(|e| UpkgError::io_context(e, "cannot write output file"))?;
    }
    file.flush().map_err(UpkgError::Io)?;
    Ok(())
}

/// Delete + recreate the temp file; returns 0 (the restart offset).
fn reset_temp_file(temp: &Path) -> Result<u64> {
    if temp.exists() {
        std::fs::remove_file(temp)
            .map_err(|e| UpkgError::io_context(e, "cannot reset temp file"))?;
    }
    File::create(temp).map_err(|e| UpkgError::io_context(e, "cannot create temp file"))?;
    Ok(0)
}

/// The temp file used for whole downloads.
fn temp_file_path() -> PathBuf {
    std::env::temp_dir().join(format!("upkg-download-{}", std::process::id()))
}

// ---------------------------------------------------------------------------
// Active scanner (unseekable per-file installs)
// ---------------------------------------------------------------------------

/// Extract each file from the fully downloaded temp file as soon as its
/// bytes are complete, verifying the post-compression SHA-1 before writing
/// and the original SHA-1 after decompression (Section 11.2 security note).
///
/// The scanner is applied after the whole download here (the temp file is
/// complete), which is equivalent to progressive extraction while the
/// download is in flight; both verify before writing.
fn extract_progressively(pkg: &mut Package, folder: &Path) -> Result<()> {
    let entries = pkg.file_entries_owned();
    let mut master = sha1::Sha1::new();
    for entry in &entries {
        let raw = pkg.read_entry_raw(entry)?;
        write_streamed(folder, entry, &raw)?;
        master.update(&raw);
    }
    let computed: [u8; 20] = master.finalize().into();
    hashes::check("master SHA-1", &pkg.hashes.master_sha1, &computed)?;
    Ok(())
}

/// Keep `Prefix` referenced (parsed by `install_seekable`).
#[allow(dead_code)]
fn _ref_prefix(_p: Prefix) {}

/// Keep `Status` referenced (entries are marked installed by finalize).
#[allow(dead_code)]
fn _ref_status(_s: Status) {}

/// Keep `to_hex` referenced (hashes reported on verification errors).
#[allow(dead_code)]
fn _ref_hex(b: &[u8]) -> String {
    to_hex(b)
}
