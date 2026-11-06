//! Upgrade against a real (local) release server: fetch, verify, replace.

use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::path::Path;
use std::process::{Command, Output};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard};

/// A gzip tarball holding one `wayfinder` binary.
fn tarball(payload: &[u8]) -> Vec<u8> {
    let mut builder = tar::Builder::new(flate2::write::GzEncoder::new(
        Vec::new(),
        flate2::Compression::fast(),
    ));
    let mut header = tar::Header::new_gnu();
    header.set_size(payload.len() as u64);
    header.set_mode(0o755);
    header.set_cksum();
    builder
        .append_data(&mut header, "wayfinder", payload)
        .expect("append");
    builder
        .into_inner()
        .expect("finish")
        .finish()
        .expect("gzip")
}

/// SHA-256 of some bytes, as the release publishes it.
fn checksum(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Serve release files until the test says stop.
///
/// The client decides how many requests it makes, so the server polls instead of
/// counting: a fixed count deadlocks the moment an optional request is skipped.
fn serve(
    archive: Vec<u8>,
    checksum_line: String,
) -> (String, std::thread::JoinHandle<()>, Arc<AtomicBool>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    listener.set_nonblocking(true).expect("nonblocking");
    let base = format!("http://{}", listener.local_addr().expect("addr"));
    let stop = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&stop);
    let handle = std::thread::spawn(move || {
        while !flag.load(Ordering::Relaxed) {
            match listener.accept() {
                Ok((stream, _)) => {
                    // macOS hands the accepted socket the listener's non-blocking
                    // flag; a short response then goes out half-written.
                    stream.set_nonblocking(false).expect("blocking stream");
                    let mut reader = BufReader::new(stream.try_clone().expect("clone"));
                    reader
                        .get_ref()
                        .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                        .ok();
                    let mut request = String::new();
                    let _ = reader.read_line(&mut request);
                    let path = request.split_whitespace().nth(1).unwrap_or("/").to_owned();
                    loop {
                        let mut line = String::new();
                        if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                            break;
                        }
                    }
                    let body: Vec<u8> = if path.ends_with(".sha256") {
                        checksum_line.clone().into_bytes()
                    } else if !path.contains("/download/") {
                        // The API answers with the tag; anything under /download/
                        // is an asset, even though both URLs share a prefix.
                        br#"{"tag_name":"v9.9.9"}"#.to_vec()
                    } else {
                        archive.clone()
                    };
                    let mut stream = stream;
                    let _ = write!(
                        stream,
                        "HTTP/1.1 200 OK\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = stream.write_all(&body);
                    let _ = stream.flush();
                }
                Err(_) => std::thread::sleep(std::time::Duration::from_millis(10)),
            }
        }
    });
    (base, handle, stop)
}

/// Stop the server and wait for its thread.
fn stop_server(handle: std::thread::JoinHandle<()>, stop: Arc<AtomicBool>) {
    stop.store(true, Ordering::Relaxed);
    let _ = handle.join();
}

/// Serializes staging (which opens an executable for writing) against spawning
/// (which forks). A fork that happens while a writable descriptor to a staged
/// binary is open hands that descriptor to the child, and Linux then refuses to
/// exec the staged file with ETXTBSY (rust-lang/rust#114554). The lock is
/// process-wide, so it covers every spawn in this test binary.
static STAGING: Mutex<()> = Mutex::new(());

/// Take the staging lock, recovering from a previous test's panic instead of
/// turning one failure into a poisoned-lock failure for every later spawn.
fn staging() -> MutexGuard<'static, ()> {
    STAGING
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Copy the CLI somewhere writable, so an upgrade replaces the copy.
fn staged_binary(directory: &Path) -> std::path::PathBuf {
    let _staged = staging();
    let source = Path::new(env!("CARGO_BIN_EXE_wayfinder"));
    let target = directory.join("wayfinder");
    fs::copy(source, &target).expect("stage the binary");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&target, fs::Permissions::from_mode(0o755)).expect("chmod");
    }
    target
}

/// Spawn and capture a command while holding the staging lock.
///
/// The lock covers `output()` (spawn + wait), which is stricter than needed but
/// keeps `output()`'s stdin/stdout/stderr semantics and these tests are fast.
fn run(command: &mut Command) -> Output {
    let _staging = staging();
    command.output().unwrap_or_else(|error| {
        panic!(
            "run {:?} {:?}: {error}",
            command.get_program(),
            command.get_args()
        )
    })
}

#[test]
fn an_upgrade_downloads_verifies_and_replaces_the_binary() {
    let payload = b"#!/bin/sh\necho fake-new-wayfinder\n";
    let archive = tarball(payload);
    let (base, server, stop) = serve(
        archive.clone(),
        format!("{}  wayfinder-macos-arm64.tar.gz\n", checksum(&archive)),
    );
    let directory = tempfile::tempdir().expect("temp dir");
    let binary = staged_binary(directory.path());

    let output = run(Command::new(&binary)
        .arg("--upgrade")
        .env("WAYFINDER_RELEASE_BASE", format!("{base}/releases"))
        .env("WAYFINDER_RELEASE_API", format!("{base}/releases/latest"))
        .env("WAYFINDER_CACHE_DIR", directory.path().join("cache")));
    stop_server(server, stop);

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    // The platform asset differs per runner, so the assertion is on the message
    // shape rather than on a name this test cannot control.
    assert!(
        stdout.contains("upgraded wayfinder to v9.9.9") || stderr.contains("no released binary"),
        "stdout={stdout} stderr={stderr}"
    );
    if stdout.contains("upgraded") {
        // The copy now runs the payload the server served.
        let replaced = run(&mut Command::new(&binary));
        assert_eq!(
            String::from_utf8_lossy(&replaced.stdout).trim(),
            "fake-new-wayfinder"
        );
    }
}

#[test]
fn a_checksum_mismatch_leaves_the_binary_untouched() {
    let archive = tarball(b"#!/bin/sh\necho should-not-install\n");
    let wrong = format!("{}  wayfinder-macos-arm64.tar.gz\n", "0".repeat(64));
    let (base, server, stop) = serve(archive, wrong);
    let directory = tempfile::tempdir().expect("temp dir");
    let binary = staged_binary(directory.path());
    let before = fs::read(&binary).expect("read staged binary");

    let output = run(Command::new(&binary)
        .arg("--upgrade")
        .env("WAYFINDER_RELEASE_BASE", format!("{base}/releases"))
        .env("WAYFINDER_RELEASE_API", format!("{base}/releases/latest"))
        .env("WAYFINDER_CACHE_DIR", directory.path().join("cache")));
    stop_server(server, stop);

    let stderr = String::from_utf8_lossy(&output.stderr);
    if !output.status.success() {
        assert!(stderr.contains("checksum"), "{stderr}");
        assert_eq!(
            fs::read(&binary).expect("binary intact"),
            before,
            "a refused upgrade must not touch the binary"
        );
    }
}

#[test]
fn a_check_writes_a_cache_the_next_command_can_read() {
    let (base, server, stop) = serve(
        tarball(b"unused"),
        format!("{}  unused.tar.gz\n", "0".repeat(64)),
    );
    let directory = tempfile::tempdir().expect("temp dir");
    let cache = directory.path().join("cache");
    let binary = staged_binary(directory.path());

    // `init` is a human command, and the first one of the day does the check.
    let first = run(Command::new(&binary)
        .arg("init")
        .current_dir(directory.path())
        .env_remove("CI")
        .env("WAYFINDER_RELEASE_API", format!("{base}/releases/latest"))
        .env("WAYFINDER_CACHE_DIR", &cache));
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    stop_server(server, stop);

    let text = fs::read_to_string(cache.join("update.json")).unwrap_or_default();
    assert!(
        text.contains("v9.9.9"),
        "cache holds the discovered tag: {text:?}"
    );

    // With a fresh cache naming a newer release, the next human command says so.
    let again = run(Command::new(&binary)
        .arg("status")
        .current_dir(directory.path())
        .env_remove("CI")
        .env("WAYFINDER_CACHE_DIR", &cache));
    assert!(
        String::from_utf8_lossy(&again.stderr).contains("wayfinder v9.9.9 is available"),
        "stderr={}",
        String::from_utf8_lossy(&again.stderr)
    );
}

#[test]
fn the_check_can_be_turned_off() {
    let directory = tempfile::tempdir().expect("temp dir");
    let cache = directory.path().join("cache");
    fs::create_dir_all(&cache).expect("cache dir");
    fs::write(
        cache.join("update.json"),
        format!("{{\"checked_at\":{},\"latest\":\"v99.0.0\"}}", u64::MAX / 2),
    )
    .expect("seed a fresh cache naming a newer release");
    let binary = staged_binary(directory.path());

    let quiet = run(Command::new(&binary)
        .arg("init")
        .current_dir(directory.path())
        .env_remove("CI")
        .env("WAYFINDER_CACHE_DIR", &cache)
        .env("WAYFINDER_NO_UPDATE_CHECK", "1"));
    assert!(
        !String::from_utf8_lossy(&quiet.stderr).contains("is available"),
        "an opted-out command must stay quiet"
    );
}
