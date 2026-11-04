//! The MCP surface: one JSON-RPC exchange over a real stdio pipe.

use serde_json::{Value, json};
use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

/// One project directory with a cited file and an initialised pool.
fn project() -> tempfile::TempDir {
    let directory = tempfile::tempdir().expect("temp dir");
    fs::create_dir_all(directory.path().join("src")).expect("src");
    fs::write(directory.path().join("src/limiter.rs"), "fn limit() {}\n").expect("cited file");
    let init = Command::new(env!("CARGO_BIN_EXE_wayfinder"))
        .args(["init"])
        .current_dir(directory.path())
        .output()
        .expect("init");
    assert!(
        init.status.success(),
        "{}",
        String::from_utf8_lossy(&init.stderr)
    );
    directory
}

/// Send one batch of JSON-RPC messages to a server started with `args`.
fn exchange_args(directory: &Path, args: &[&str], requests: &[Value]) -> Vec<Value> {
    let mut child = Command::new(env!("CARGO_BIN_EXE_wayfinder"))
        .args(args)
        .current_dir(directory)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn server");
    {
        let stdin = child.stdin.as_mut().expect("stdin");
        for request in requests {
            writeln!(stdin, "{request}").expect("write request");
        }
    }
    // Dropping stdin closes the pipe, which ends the server loop.
    drop(child.stdin.take());
    let output = child.wait_with_output().expect("wait");
    assert!(
        output.status.success(),
        "server failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).expect("valid JSON answer"))
        .collect()
}

/// Send one batch of JSON-RPC messages to the default server.
fn exchange(directory: &Path, requests: &[Value]) -> Vec<Value> {
    exchange_args(directory, &["mcp"], requests)
}

/// One `tools/call` request.
fn call(id: i64, name: &str, arguments: Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "tools/call",
        "params": { "name": name, "arguments": arguments },
    })
}

/// Text of a tool answer.
fn tool_text(answer: &Value) -> String {
    answer["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or_default()
        .to_owned()
}

#[test]
fn initialize_announces_tools_and_how_to_use_them() {
    let directory = project();
    let answers = exchange(
        directory.path(),
        &[json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": { "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": { "name": "test", "version": "0" } },
        })],
    );
    assert_eq!(answers.len(), 1);
    let result = &answers[0]["result"];
    assert_eq!(result["protocolVersion"], "2025-06-18");
    assert_eq!(result["serverInfo"]["name"], "wayfinder");
    assert!(result["capabilities"]["tools"].is_object());
    let instructions = result["instructions"].as_str().expect("instructions");
    assert!(instructions.contains("memory_search"), "{instructions}");
    assert!(instructions.contains("memory_save"), "{instructions}");
}

#[test]
fn the_catalog_lists_every_tool_with_a_schema() {
    let directory = project();
    let answers = exchange(
        directory.path(),
        &[json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" })],
    );
    let tools = answers[0]["result"]["tools"].as_array().expect("tools");
    let names: Vec<&str> = tools
        .iter()
        .map(|tool| tool["name"].as_str().expect("name"))
        .collect();
    for expected in [
        "memory_search",
        "memory_read",
        "memory_save",
        "memory_list",
        "memory_check",
        "memory_digest",
        "memory_graph",
    ] {
        assert!(names.contains(&expected), "missing {expected}: {names:?}");
    }
    for tool in tools {
        assert!(
            tool["description"]
                .as_str()
                .is_some_and(|text| !text.is_empty())
        );
        assert_eq!(tool["inputSchema"]["type"], "object");
    }
    // A harness that already owns `memory_*` asks for the other prefix, and the
    // same catalog answers under it.
    let renamed = exchange_args(
        directory.path(),
        &["mcp", "--prefix", "wayfinder"],
        &[
            json!({ "jsonrpc": "2.0", "id": 3, "method": "tools/list" }),
            json!({ "jsonrpc": "2.0", "id": 4, "method": "tools/call", "params": { "name": "wayfinder_list", "arguments": {} } }),
        ],
    );
    let renamed_names: Vec<&str> = renamed[0]["result"]["tools"]
        .as_array()
        .expect("tools")
        .iter()
        .map(|tool| tool["name"].as_str().expect("name"))
        .collect();
    assert!(
        renamed_names.contains(&"wayfinder_search"),
        "{renamed_names:?}"
    );
    assert!(!renamed_names.iter().any(|name| name.starts_with("memory_")));
    assert_eq!(renamed[1]["result"]["isError"], false);
    assert!(tool_text(&renamed[1]).contains("pool is empty"));
}

#[test]
fn a_save_then_a_search_read_and_check_round_trip() {
    let directory = project();
    let answers = exchange(
        directory.path(),
        &[
            call(
                10,
                "memory_save",
                json!({
                    "title": "Rate limits",
                    "body": "## Throughput\nThe limiter allows 100 per second.\n\n## Where\nIn the limiter module.",
                    "kind": "fact",
                    "tags": ["memory"],
                    "subjects": ["package:widget"],
                    "anchors": ["src/limiter.rs:42"],
                }),
            ),
            call(11, "memory_search", json!({ "query": "100 per second" })),
            call(
                12,
                "memory_read",
                json!({ "id": "rate-limits", "section": "throughput" }),
            ),
            call(13, "memory_check", json!({ "id": "rate-limits" })),
            call(14, "memory_list", json!({})),
            call(15, "memory_digest", json!({})),
            call(16, "memory_graph", json!({ "format": "mermaid" })),
        ],
    );
    assert_eq!(answers.len(), 7, "{answers:#?}");

    let saved = tool_text(&answers[0]);
    assert!(
        saved.contains("Saved \"Rate limits\" (id: rate-limits, rev: "),
        "{saved}"
    );
    assert!(!answers[0]["result"]["isError"].as_bool().unwrap_or(false));

    let found = tool_text(&answers[1]);
    assert!(found.contains("Rate limits (id: rate-limits"), "{found}");

    let section = tool_text(&answers[2]);
    assert!(section.contains("100 per second"), "{section}");
    assert!(!section.contains("In the limiter module"), "{section}");

    let checked = tool_text(&answers[3]);
    assert!(
        checked.contains("still matches memory page \"rate-limits\""),
        "{checked}"
    );

    let listed = tool_text(&answers[4]);
    assert!(
        listed.contains("- Rate limits (id: rate-limits"),
        "{listed}"
    );

    let digest = tool_text(&answers[5]);
    assert!(digest.contains("1 memory page"), "{digest}");

    let graph = tool_text(&answers[6]);
    assert!(graph.starts_with("flowchart LR"), "{graph}");
}

#[test]
fn a_stale_save_reports_a_conflict_instead_of_overwriting() {
    let directory = project();
    let answers = exchange(
        directory.path(),
        &[
            call(
                20,
                "memory_save",
                json!({ "title": "Rate limits", "body": "v2" }),
            ),
            call(
                21,
                "memory_save",
                json!({ "title": "Rate limits", "body": "stale", "expectedRev": "0000000000000000" }),
            ),
        ],
    );
    let refused = &answers[1]["result"];
    assert_eq!(refused["isError"], true);
    assert_eq!(refused["_meta"]["code"], 409);
    assert!(tool_text(&answers[1]).contains("changed since it was read"));

    let page = fs::read_to_string(directory.path().join(".wayfinder/memory/rate-limits.md"))
        .expect("page");
    assert!(page.contains("v2"), "{page}");
}

#[test]
fn an_unknown_tool_and_an_unknown_method_answer_errors() {
    let directory = project();
    let answers = exchange(
        directory.path(),
        &[
            call(30, "memory_nonsense", json!({})),
            json!({ "jsonrpc": "2.0", "id": 31, "method": "does/not/exist" }),
        ],
    );
    assert_eq!(answers[0]["result"]["isError"], true);
    assert_eq!(answers[1]["error"]["code"], -32601);
}

#[test]
fn a_reading_a_missing_page_is_a_tool_error_not_a_crash() {
    let directory = project();
    let answers = exchange(
        directory.path(),
        &[call(40, "memory_read", json!({ "id": "nope" }))],
    );
    let result = &answers[0]["result"];
    assert_eq!(result["isError"], true);
    assert_eq!(result["_meta"]["code"], 32602);
    assert!(tool_text(&answers[0]).contains("no memory page with id \"nope\""));
}

#[test]
fn a_notification_gets_no_answer() {
    let directory = project();
    let answers = exchange(
        directory.path(),
        &[
            // No id: a notification the server must not answer.
            json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
            json!({ "jsonrpc": "2.0", "id": 50, "method": "ping" }),
        ],
    );
    assert_eq!(answers.len(), 1, "{answers:#?}");
    assert_eq!(answers[0]["id"], 50);
}

#[test]
fn a_client_can_pull_the_digest_as_a_prompt() {
    let directory = project();
    let answers = exchange(
        directory.path(),
        &[
            json!({ "jsonrpc": "2.0", "id": 60, "method": "prompts/list" }),
            json!({ "jsonrpc": "2.0", "id": 61, "method": "prompts/get", "params": { "name": "workspace-memory" } }),
            call(
                62,
                "memory_save",
                json!({ "title": "Rate limits", "body": "100 per second" }),
            ),
            json!({ "jsonrpc": "2.0", "id": 63, "method": "prompts/get", "params": { "name": "workspace-memory" } }),
            json!({ "jsonrpc": "2.0", "id": 64, "method": "prompts/get", "params": { "name": "nope" } }),
        ],
    );
    let prompts = answers[0]["result"]["prompts"].as_array().expect("prompts");
    assert_eq!(prompts[0]["name"], "workspace-memory");
    // An empty pool still answers, so a client can surface it unconditionally.
    let empty = answers[1]["result"]["messages"][0]["content"]["text"]
        .as_str()
        .expect("text");
    assert!(empty.contains("pool is empty"), "{empty}");

    let filled = answers[3]["result"]["messages"][0]["content"]["text"]
        .as_str()
        .expect("text");
    assert!(filled.contains("1 memory page"), "{filled}");
    assert!(filled.contains("Rate limits"), "{filled}");
    // An unknown prompt is an invalid-parameter error, not a prompt.
    assert_eq!(answers[4]["error"]["code"], -32602);
}
