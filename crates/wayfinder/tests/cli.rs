//! The terminal surface: every command against a real pool on disk.

use std::fs;
use std::path::Path;
use std::process::{Command, Output};

/// Run the built binary inside one project directory.
fn wayfinder(directory: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_wayfinder"))
        .args(args)
        .current_dir(directory)
        .output()
        .expect("run wayfinder")
}

/// One project directory with a cited file and an initialised pool.
fn project() -> tempfile::TempDir {
    let directory = tempfile::tempdir().expect("temp dir");
    fs::create_dir_all(directory.path().join("src")).expect("src");
    fs::write(directory.path().join("src/limiter.rs"), "fn limit() {}\n").expect("cited file");
    let init = wayfinder(directory.path(), &["init"]);
    assert!(
        init.status.success(),
        "{}",
        String::from_utf8_lossy(&init.stderr)
    );
    directory
}

/// Standard output of a successful command.
fn stdout(output: &Output) -> String {
    assert!(
        output.status.success(),
        "command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).to_string()
}

#[test]
fn init_creates_the_pool_and_its_index() {
    let directory = tempfile::tempdir().expect("temp dir");
    let output = wayfinder(directory.path(), &["init"]);
    let text = stdout(&output);
    assert!(text.contains("pool at"));
    assert!(directory.path().join(".wayfinder/memory").is_dir());
    assert!(directory.path().join(".wayfinder/index.db").is_file());
}

#[test]
fn a_saved_page_is_found_read_and_listed() {
    let directory = project();
    let saved = stdout(&wayfinder(
        directory.path(),
        &[
            "save",
            "--title",
            "Rate limits",
            "--body",
            "## Throughput\nThe limiter allows 100 per second.\n\n## Where\nIn the limiter module.",
            "--tags",
            "memory",
            "--kind",
            "fact",
            "--subject",
            "package:widget",
            "--anchor",
            "src/limiter.rs:42",
        ],
    ));
    assert!(
        saved.contains("Saved \"Rate limits\" (id: rate-limits, rev: "),
        "{saved}"
    );

    let found = stdout(&wayfinder(directory.path(), &["search", "100 per second"]));
    assert!(found.contains("Rate limits (id: rate-limits"), "{found}");

    let read = stdout(&wayfinder(
        directory.path(),
        &["read", "rate-limits", "--section", "throughput"],
    ));
    assert!(read.contains("100 per second"), "{read}");
    assert!(!read.contains("In the limiter module"), "{read}");

    let listed = stdout(&wayfinder(directory.path(), &["list"]));
    assert!(listed.contains("Rate limits (id: rate-limits"), "{listed}");
}

#[test]
fn saving_from_stdin_reads_the_body() {
    let directory = project();
    use std::io::Write;
    let mut child = Command::new(env!("CARGO_BIN_EXE_wayfinder"))
        .args(["save", "--title", "From stdin", "--body", "-"])
        .current_dir(directory.path())
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .expect("spawn");
    child
        .stdin
        .as_mut()
        .expect("stdin")
        .write_all(b"## Notes\nbody from a pipe\n")
        .expect("write");
    let output = child.wait_with_output().expect("wait");
    assert!(output.status.success());
    let read = stdout(&wayfinder(directory.path(), &["read", "from-stdin"]));
    assert!(read.contains("body from a pipe"), "{read}");
}

#[test]
fn check_passes_on_a_fresh_anchor_and_fails_once_the_file_changes() {
    let directory = project();
    stdout(&wayfinder(
        directory.path(),
        &[
            "save",
            "--title",
            "Rate limits",
            "--body",
            "cited",
            "--anchor",
            "src/limiter.rs",
        ],
    ));
    let fresh = wayfinder(directory.path(), &["check"]);
    assert!(fresh.status.success());
    assert!(stdout(&fresh).contains("still matches memory page \"rate-limits\""));

    fs::write(
        directory.path().join("src/limiter.rs"),
        "fn limit() { /* moved */ }\n",
    )
    .expect("edit");
    let drifted = wayfinder(directory.path(), &["check"]);
    assert_eq!(
        drifted.status.code(),
        Some(1),
        "drift must fail the command"
    );
    // A drift report is a successful answer with a failing exit code, so read
    // the stdout directly instead of going through the success helper.
    let text = String::from_utf8_lossy(&drifted.stdout);
    assert!(text.contains("changed src/limiter.rs"), "{text}");
}

#[test]
fn a_stale_revision_is_refused_from_the_terminal() {
    let directory = project();
    stdout(&wayfinder(
        directory.path(),
        &["save", "--title", "Rate limits", "--body", "v1"],
    ));
    let page = fs::read_to_string(directory.path().join(".wayfinder/memory/rate-limits.md"))
        .expect("page");
    assert!(page.contains("title: Rate limits"));

    let stale = wayfinder(
        directory.path(),
        &[
            "save",
            "--title",
            "Rate limits",
            "--body",
            "stale",
            "--expected-rev",
            "0000000000000000",
        ],
    );
    assert_eq!(stale.status.code(), Some(1));
    let message = String::from_utf8_lossy(&stale.stderr);
    assert!(message.contains("changed since it was read"), "{message}");
    let unchanged = stdout(&wayfinder(directory.path(), &["read", "rate-limits"]));
    assert!(unchanged.contains("v1"), "{unchanged}");
}

#[test]
fn digest_status_and_graph_answer_for_a_real_pool() {
    let directory = project();
    stdout(&wayfinder(
        directory.path(),
        &[
            "save",
            "--title",
            "Rate limits",
            "--body",
            "100 per second",
            "--subject",
            "package:widget",
        ],
    ));

    let digest = stdout(&wayfinder(directory.path(), &["digest"]));
    assert!(digest.contains("1 memory page"), "{digest}");
    assert!(digest.contains("wayfinder search"), "{digest}");

    let status = stdout(&wayfinder(directory.path(), &["status", "--json"]));
    assert!(status.contains("\"pages\": 1"), "{status}");

    let svg_path = directory.path().join("graph.svg");
    let graph = stdout(&wayfinder(
        directory.path(),
        &[
            "graph",
            "--format",
            "svg",
            "--out",
            svg_path.to_str().expect("utf8"),
        ],
    ));
    assert!(graph.contains("wrote"), "{graph}");
    let svg = fs::read_to_string(&svg_path).expect("svg");
    assert!(svg.starts_with("<svg"));
    assert!(svg.contains("Rate limits"));

    let json = stdout(&wayfinder(directory.path(), &["graph", "--format", "json"]));
    assert!(json.contains("\"subject:package:widget\""), "{json}");
}

#[test]
fn a_missing_pool_points_at_init() {
    let directory = tempfile::tempdir().expect("temp dir");
    let output = wayfinder(directory.path(), &["search", "anything"]);
    assert_eq!(output.status.code(), Some(1));
    let message = String::from_utf8_lossy(&output.stderr);
    assert!(message.contains("run `wayfinder init`"), "{message}");
}

#[test]
fn help_lists_the_commands() {
    let directory = tempfile::tempdir().expect("temp dir");
    let output = wayfinder(directory.path(), &["--help"]);
    let text = stdout(&output);
    for command in [
        "init", "save", "read", "search", "list", "check", "digest", "graph", "status",
    ] {
        assert!(
            text.contains(command),
            "help must mention {command}:\n{text}"
        );
    }
}
