//! MCP over stdio: the same pool operations, exposed as tools a harness calls.
//!
//! Messages are newline-delimited JSON-RPC 2.0, one object per line, which is
//! what MCP says on a stdio transport. Every tool answers with text content; a
//! failing tool answers with `isError: true` and the reason, never a crash.

use anyhow::Result;
use serde_json::{Value, json};
use std::io::{BufRead, Write};
use wayfinder_core::{
    Anchor, CheckReport, DigestOptions, GraphBounds, GraphFormat, Kind, SaveRequest, SearchFilters,
    Store, build_graph, check_page, digest, render_graph,
};

/// Protocol revision this server echoes when the client names none.
const PROTOCOL_VERSION: &str = "2025-06-18";

/// How many pages one tool answer may print.
const TOOL_PAGE_LIMIT: usize = 12;

/// Tool-name prefix, so a harness that already owns `memory_*` can ask for
/// `wayfinder_*` instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Prefix {
    /// `memory_save`, matching the Factored Harness tool surface.
    Memory,
    /// `wayfinder_save`, for a harness that has its own memory tools.
    Wayfinder,
}

impl Prefix {
    /// Parse a `--prefix` value.
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "memory" => Some(Self::Memory),
            "wayfinder" => Some(Self::Wayfinder),
            _ => None,
        }
    }

    /// Root of every tool name.
    fn root(self) -> &'static str {
        match self {
            Self::Memory => "memory",
            Self::Wayfinder => "wayfinder",
        }
    }

    /// Full tool name for one operation.
    fn tool(self, operation: &str) -> String {
        format!("{}_{operation}", self.root())
    }
}

/// Serve the pool on stdin/stdout until the client closes the pipe.
/// @param store - the pool this server answers about
/// @param prefix - tool-name root the harness expects
pub fn serve(store: Store, prefix: Prefix) -> Result<()> {
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let request: Value = match serde_json::from_str(&line) {
            Ok(value) => value,
            Err(error) => {
                let response = json!({
                    "jsonrpc": "2.0",
                    "id": Value::Null,
                    "error": { "code": -32700, "message": format!("parse error: {error}") },
                });
                writeln!(stdout, "{response}")?;
                stdout.flush()?;
                continue;
            }
        };
        if let Some(response) = handle(&store, prefix, &request) {
            writeln!(stdout, "{response}")?;
            stdout.flush()?;
        }
    }
    Ok(())
}

/// Answer one JSON-RPC request. A notification (no `id`) produces no response.
/// @param store - the pool this server answers about
/// @param prefix - tool-name root
/// @param request - one parsed message
pub fn handle(store: &Store, prefix: Prefix, request: &Value) -> Option<Value> {
    let id = request.get("id").cloned()?;
    let method = request
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let params = request.get("params").cloned().unwrap_or(Value::Null);
    let response = match method {
        "initialize" => {
            let requested = params
                .get("protocolVersion")
                .and_then(Value::as_str)
                .unwrap_or(PROTOCOL_VERSION);
            Ok(json!({
                "protocolVersion": requested,
                "capabilities": { "tools": { "listChanged": false } },
                "serverInfo": { "name": "wayfinder", "version": env!("CARGO_PKG_VERSION") },
                "instructions": instructions(prefix),
            }))
        }
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({ "tools": tool_list(prefix) })),
        "tools/call" => call_tool(store, prefix, &params),
        "resources/list" => Ok(json!({ "resources": [] })),
        "prompts/list" => Ok(json!({ "prompts": [] })),
        other => Err((-32601, format!("unknown method {other:?}"))),
    };
    Some(match response {
        Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
        Err((code, message)) => json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": { "code": code, "message": message },
        }),
    })
}

/// Text a harness may paste into its system prompt once, returned from
/// `initialize` so a client that shows it needs no extra wiring.
fn instructions(prefix: Prefix) -> String {
    format!(
        "Wayfinder keeps this workspace's memory. Before re-deriving how something works, call \
`{search}`; when you learn a durable fact, call `{save}` with the files it rests on; call \
`{check}` before trusting a page whose cited files may have moved.",
        search = prefix.tool("search"),
        save = prefix.tool("save"),
        check = prefix.tool("check"),
    )
}

/// The tool catalog, with the JSON Schema each call validates against.
fn tool_list(prefix: Prefix) -> Vec<Value> {
    let name = |operation: &str| prefix.tool(operation);
    vec![
        json!({
            "name": name("search"),
            "description": "Search this workspace's memory pages: findings, decisions, and gotchas earlier sessions wrote. Results are widened one hop over declared relations, so a page that never names the query can still answer it. Use it before reading files that may already be summarized.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "What to look for; empty lists the most recently updated pages." },
                    "limit": { "type": "integer", "description": "Maximum pages to return (default 8)." },
                    "kind": { "type": "string", "enum": ["fact", "decision", "gotcha", "pointer"], "description": "Only pages of this kind." },
                    "subject": { "type": "string", "description": "Only pages declaring this subject, e.g. package:my-crate." }
                }
            },
        }),
        json!({
            "name": name("read"),
            "description": "Read one memory page, or one section of it. Returns the page revision, which save accepts as expectedRev to detect a concurrent edit.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": { "type": "string", "description": "Page id from a search result." },
                    "section": { "type": "string", "description": "Return only the heading whose text matches, case-insensitively." }
                },
                "required": ["id"]
            },
        }),
        json!({
            "name": name("save"),
            "description": "Save a durable finding as a memory page so a later session reads it instead of re-deriving it. Cite every file the fact rests on with anchors; the store fingerprints them so a later check can tell whether the page still holds.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "title": { "type": "string", "description": "Page title; the id derives from it." },
                    "body": { "type": "string", "description": "Markdown body. Use headings so a later read can return one section." },
                    "id": { "type": "string", "description": "Override the derived page id." },
                    "tags": { "type": "array", "items": { "type": "string" }, "description": "Labels to merge into the page." },
                    "kind": { "type": "string", "enum": ["fact", "decision", "gotcha", "pointer"], "description": "What the page is for; pointer ranks below the rest." },
                    "subjects": { "type": "array", "items": { "type": "string" }, "description": "What the page is about, e.g. package:my-crate or area:auth." },
                    "depends": { "type": "array", "items": { "type": "string" }, "description": "Packages this page depends on, one edge each." },
                    "supersedes": { "type": "string", "description": "Id of the page this one replaces." },
                    "anchors": { "type": "array", "items": { "type": "string" }, "description": "Files the fact rests on: path, path:line, or path@sha." },
                    "source": { "type": "string", "description": "Where the page came from, e.g. session:<id>." },
                    "expectedRev": { "type": "string", "description": "Revision from read; the save is refused if the page moved." }
                },
                "required": ["title", "body"]
            },
        }),
        json!({
            "name": name("list"),
            "description": "List memory pages, newest first, with ids and update dates.",
            "inputSchema": {
                "type": "object",
                "properties": { "limit": { "type": "integer", "description": "Maximum pages to list (default 20)." } }
            },
        }),
        json!({
            "name": name("check"),
            "description": "Verify the files a memory page cites. Reports fresh, changed, or missing per anchor; re-read anything changed before trusting the page.",
            "inputSchema": {
                "type": "object",
                "properties": { "id": { "type": "string", "description": "Page to check; absent checks every page." } }
            },
        }),
        json!({
            "name": name("digest"),
            "description": "The block that summarizes this workspace's memory for a prompt: how many pages exist, the most recent ones, and how to search them.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "pages": { "type": "integer", "description": "Maximum pages to list (default 8)." },
                    "tokens": { "type": "integer", "description": "Token budget for the block (default 500)." }
                }
            },
        }),
        json!({
            "name": name("graph"),
            "description": "This workspace's memory as a knowledge graph: pages, the subjects and cited paths they declare, and the relations between them.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "format": { "type": "string", "enum": ["json", "mermaid", "dot"], "description": "Rendering to return (default mermaid)." },
                    "subject": { "type": "string", "description": "Draw only this subject's neighbourhood." }
                }
            },
        }),
    ]
}

/// Dispatch one `tools/call`.
fn call_tool(
    store: &Store,
    prefix: Prefix,
    params: &Value,
) -> std::result::Result<Value, (i64, String)> {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let arguments = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let root = format!("{}_", prefix.root());
    let operation = name
        .strip_prefix(&root)
        .ok_or((-32602, format!("unknown tool {name:?}")))?;
    let outcome = match operation {
        "search" => tool_search(store, &arguments),
        "read" => tool_read(store, &arguments),
        "save" => tool_save(store, &arguments),
        "list" => tool_list_pages(store, &arguments),
        "check" => tool_check(store, &arguments),
        "digest" => tool_digest(store, &arguments),
        "graph" => tool_graph(store, &arguments),
        other => Err((32602, format!("unknown tool {other:?}"))),
    };
    Ok(match outcome {
        Ok(text) => json!({ "content": [{ "type": "text", "text": text }], "isError": false }),
        Err((code, message)) => json!({
            "content": [{ "type": "text", "text": message }],
            "isError": true,
            "_meta": { "code": code },
        }),
    })
}

/// One string argument.
fn string_arg(arguments: &Value, key: &str) -> Option<String> {
    arguments
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .filter(|value| !value.is_empty())
}

/// One list-of-strings argument.
fn list_arg(arguments: &Value, key: &str) -> Vec<String> {
    arguments
        .get(key)
        .and_then(Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

/// One integer argument.
fn number_arg(arguments: &Value, key: &str) -> Option<usize> {
    arguments
        .get(key)
        .and_then(Value::as_u64)
        .map(|value| value as usize)
}

/// Kind argument.
fn kind_arg(arguments: &Value) -> Option<Kind> {
    arguments
        .get("kind")
        .and_then(Value::as_str)
        .and_then(Kind::parse)
}

/// A tool answers with text, or with a code and a message it can print.
type ToolResult = std::result::Result<String, (i64, String)>;

/// `memory_search`.
fn tool_search(store: &Store, arguments: &Value) -> ToolResult {
    let query = string_arg(arguments, "query").unwrap_or_default();
    let filters = SearchFilters {
        kind: kind_arg(arguments),
        subject: string_arg(arguments, "subject"),
        limit: Some(
            number_arg(arguments, "limit")
                .unwrap_or(8)
                .min(TOOL_PAGE_LIMIT),
        ),
    };
    let outcome = store.search(&query, &filters).map_err(internal)?;
    if outcome.pages.is_empty() {
        return Ok(format!(
            "No memory pages matched {query:?}. Save what you learn with the save tool so a later session can find it."
        ));
    }
    let mut text = String::new();
    for (index, hit) in outcome.pages.iter().enumerate() {
        let mut meta = vec![format!("id: {}", hit.id)];
        if let Some(kind) = hit.kind {
            meta.push(format!("kind: {}", kind.as_str()));
        }
        if let Some(updated) = &hit.updated {
            meta.push(format!("updated: {}", updated.get(..10).unwrap_or(updated)));
        }
        meta.push(format!("rev: {}", hit.rev));
        if let Some(via) = &hit.via {
            meta.push(format!("via: {via}"));
        }
        text.push_str(&format!(
            "{}. {} ({})\n",
            index + 1,
            hit.title,
            meta.join(", ")
        ));
        for line in hit.snippet.lines().take(6) {
            text.push_str(&format!("   {line}\n"));
        }
    }
    if outcome.expanded > 0 {
        text.push_str(&format!(
            "\n{} of these came from a declared relation rather than the query itself.\n",
            outcome.expanded
        ));
    }
    Ok(text)
}

/// `memory_read`.
fn tool_read(store: &Store, arguments: &Value) -> ToolResult {
    let id = string_arg(arguments, "id").ok_or((32602, "id is required".to_owned()))?;
    let section = string_arg(arguments, "section");
    let outcome = store
        .read_section(&id, section.as_deref())
        .map_err(invalid)?;
    let page = &outcome.page;
    let mut meta = vec![format!("id: {}", page.id)];
    if !page.front.tags.is_empty() {
        meta.push(format!("tags: {}", page.front.tags.join(", ")));
    }
    if let Some(kind) = page.front.kind {
        meta.push(format!("kind: {}", kind.as_str()));
    }
    if let Some(updated) = &page.front.updated {
        meta.push(format!("updated: {updated}"));
    }
    meta.push(format!("rev: {}", page.rev()));
    let mut text = format!(
        "Read memory page \"{}\" ({})\n\n",
        page.front.title,
        meta.join(", ")
    );
    if section.is_some() && !outcome.matched {
        text.push_str(&format!(
            "No section matched {:?}; the whole page follows.\n\n",
            section.unwrap_or_default()
        ));
    }
    text.push_str(&outcome.section.text);
    text.push('\n');
    Ok(text)
}

/// `memory_save`.
fn tool_save(store: &Store, arguments: &Value) -> ToolResult {
    let title = string_arg(arguments, "title").ok_or((32602, "title is required".to_owned()))?;
    let body = string_arg(arguments, "body").ok_or((32602, "body is required".to_owned()))?;
    let request = SaveRequest {
        id: string_arg(arguments, "id"),
        title: title.clone(),
        body,
        tags: list_arg(arguments, "tags"),
        kind: kind_arg(arguments),
        subjects: list_arg(arguments, "subjects"),
        depends: list_arg(arguments, "depends"),
        supersedes: string_arg(arguments, "supersedes"),
        source: string_arg(arguments, "source"),
        anchors: list_arg(arguments, "anchors")
            .iter()
            .map(|spec| Anchor::parse(spec))
            .collect(),
        expected_rev: string_arg(arguments, "expectedRev"),
        force: false,
    };
    let result = store.save(&request).map_err(|error| match error {
        wayfinder_core::Error::StaleRevision { .. } => (409, error.to_string()),
        other => (32603, other.to_string()),
    })?;
    let mut text = format!(
        "{} \"{}\" (id: {}, rev: {})\n",
        if result.created { "Saved" } else { "Updated" },
        title,
        result.id,
        result.rev,
    );
    if !result.unresolved_anchors.is_empty() {
        text.push_str(&format!(
            "No fingerprint recorded for: {}\n",
            result.unresolved_anchors.join(", ")
        ));
    }
    Ok(text)
}

/// `memory_list`.
fn tool_list_pages(store: &Store, arguments: &Value) -> ToolResult {
    let index = store.index().map_err(internal)?;
    index.sync(store.paths()).map_err(internal)?;
    let limit = number_arg(arguments, "limit").unwrap_or(20).min(200);
    let hits = index
        .list(&SearchFilters {
            kind: None,
            subject: None,
            limit: Some(limit),
        })
        .map_err(internal)?;
    if hits.is_empty() {
        return Ok("The pool is empty. The save tool writes the first page.".to_owned());
    }
    let mut text = String::new();
    for hit in hits {
        if let Ok(page) = store.read(&hit.id) {
            let stamp = page
                .front
                .updated
                .as_deref()
                .map(|updated| updated.get(..10).unwrap_or(updated))
                .unwrap_or("unknown");
            text.push_str(&format!(
                "- {} (id: {}, updated {stamp})\n",
                page.front.title, page.id
            ));
        }
    }
    Ok(text)
}

/// `memory_check`.
fn tool_check(store: &Store, arguments: &Value) -> ToolResult {
    let pages = match string_arg(arguments, "id") {
        Some(id) => vec![store.read(&id).map_err(invalid)?],
        None => store.pages().map_err(internal)?,
    };
    let mut reports: Vec<CheckReport> = Vec::new();
    for page in pages {
        reports.push(check_page(store.root(), &page).map_err(internal)?);
    }
    if reports.iter().all(|report| report.verdicts.is_empty()) {
        return Ok(
            "No page declares an anchor, so there is nothing to verify. Cite files with the \
`anchors` argument of the save tool to make a page checkable."
                .to_owned(),
        );
    }
    let mut text = String::new();
    for report in &reports {
        if report.verdicts.is_empty() {
            continue;
        }
        if report.all_fresh {
            text.push_str(&format!(
                "Every cited file still matches memory page \"{}\". Trust the page; no need to re-read those files.\n",
                report.id
            ));
            continue;
        }
        text.push_str(&format!(
            "Memory page \"{}\" cites files that changed. Re-read these before trusting the page:\n",
            report.id
        ));
        for verdict in &report.verdicts {
            text.push_str(&format!(
                "  {} {} {}\n",
                verdict.state.as_str(),
                verdict.anchor.spec(),
                verdict.detail.as_deref().unwrap_or("")
            ));
        }
    }
    Ok(text)
}

/// `memory_digest`.
fn tool_digest(store: &Store, arguments: &Value) -> ToolResult {
    let options = DigestOptions {
        pages: number_arg(arguments, "pages").unwrap_or(8),
        tokens: number_arg(arguments, "tokens").unwrap_or(500),
    };
    let text = digest(store, options).map_err(internal)?;
    Ok(if text.is_empty() {
        "The pool is empty; there is nothing to digest yet.".to_owned()
    } else {
        text
    })
}

/// `memory_graph`.
fn tool_graph(store: &Store, arguments: &Value) -> ToolResult {
    let format = string_arg(arguments, "format").unwrap_or_else(|| "mermaid".to_owned());
    let format =
        GraphFormat::parse(&format).ok_or((32602, format!("unknown graph format {format:?}")))?;
    let subject = string_arg(arguments, "subject");
    let view = build_graph(store, GraphBounds::default(), subject.as_deref()).map_err(internal)?;
    render_graph(&view, format).map_err(internal)
}

/// Wrap a store error as an internal tool failure.
fn internal(error: wayfinder_core::Error) -> (i64, String) {
    (32603, error.to_string())
}

/// Wrap a caller mistake as an invalid-argument tool failure.
fn invalid(error: wayfinder_core::Error) -> (i64, String) {
    (32602, error.to_string())
}
