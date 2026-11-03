//! Harness installation: the MCP entry a client reads and the instruction
//! snippet a model reads.
//!
//! Every writer merges instead of replacing: an existing configuration keeps
//! its other servers, and an existing instruction file keeps its other prose.
//! The snippet lives between two markers, so a second install updates it in
//! place rather than appending a copy.

use anyhow::{Context, Result, bail};
use serde_json::{Map, Value, json};
use std::fs;
use std::path::{Path, PathBuf};

/// Marker that opens the instruction block wayfinder owns.
const BEGIN: &str = "<!-- wayfinder:begin -->";
/// Marker that closes it.
const END: &str = "<!-- wayfinder:end -->";
/// Name the MCP entry registers under.
const SERVER: &str = "wayfinder";

/// A harness wayfinder can install into.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Harness {
    /// Claude Code: `.mcp.json` and `CLAUDE.md`.
    Claude,
    /// OpenAI Codex: `~/.codex/config.toml` and `AGENTS.md`.
    Codex,
    /// GitHub Copilot in VS Code: `.vscode/mcp.json` and the instruction file.
    Copilot,
    /// Cursor: `.cursor/mcp.json` and a rule file.
    Cursor,
    /// Gemini CLI: `.gemini/settings.json` and `GEMINI.md`.
    Gemini,
    /// Any other agent: `AGENTS.md` only, since the MCP path is unknown.
    Agents,
}

impl Harness {
    /// Name used in prompts and reports.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Copilot => "copilot",
            Self::Cursor => "cursor",
            Self::Gemini => "gemini",
            Self::Agents => "agents",
        }
    }

    /// Whether this harness reads a user-level configuration at all.
    fn supports_global(self) -> bool {
        true
    }
}

/// One install request.
#[derive(Debug, Clone)]
pub struct InstallOptions {
    /// Harness to install into.
    pub harness: Harness,
    /// Write the user-level configuration instead of this project's.
    pub global: bool,
    /// Replace an entry or snippet that differs.
    pub force: bool,
    /// Command the MCP entry runs; `wayfinder` when it is on `PATH`.
    pub binary: String,
}

/// What one install did.
#[derive(Debug, Clone, Default)]
pub struct InstallReport {
    /// Files written, in order.
    pub written: Vec<PathBuf>,
    /// Files left alone, with the reason.
    pub skipped: Vec<(PathBuf, String)>,
    /// Human-readable next steps.
    pub notes: Vec<String>,
}

/// Install wayfinder into one harness.
/// @param root - project directory the project-level files belong to
/// @param options - harness, scope, and overwrite policy
pub fn install(root: &Path, options: &InstallOptions) -> Result<InstallReport> {
    if options.global && !options.harness.supports_global() {
        bail!(
            "{} has no user-level configuration",
            options.harness.as_str()
        );
    }
    let mut report = InstallReport::default();
    let home = home_directory();
    let (config_path, instructions_path, json_style) = match options.harness {
        Harness::Claude => (
            scope_path(root, &home, options.global, ".mcp.json", ".claude.json"),
            scope_path(
                root,
                &home,
                options.global,
                "CLAUDE.md",
                ".claude/CLAUDE.md",
            ),
            JsonStyle::McpServers,
        ),
        Harness::Codex => (
            home.join(".codex/config.toml"),
            scope_path(root, &home, options.global, "AGENTS.md", ".codex/AGENTS.md"),
            JsonStyle::Toml,
        ),
        Harness::Copilot => (
            root.join(".vscode/mcp.json"),
            root.join(".github/copilot-instructions.md"),
            JsonStyle::VsCodeServers,
        ),
        Harness::Cursor => (
            scope_path(
                root,
                &home,
                options.global,
                ".cursor/mcp.json",
                ".cursor/mcp.json",
            ),
            scope_path(
                root,
                &home,
                options.global,
                ".cursor/rules/wayfinder.mdc",
                ".cursor/rules/wayfinder.mdc",
            ),
            JsonStyle::McpServers,
        ),
        Harness::Gemini => (
            scope_path(
                root,
                &home,
                options.global,
                ".gemini/settings.json",
                ".gemini/settings.json",
            ),
            scope_path(
                root,
                &home,
                options.global,
                "GEMINI.md",
                ".gemini/GEMINI.md",
            ),
            JsonStyle::McpServers,
        ),
        Harness::Agents => (
            PathBuf::new(),
            scope_path(root, &home, options.global, "AGENTS.md", "AGENTS.md"),
            JsonStyle::None,
        ),
    };

    if json_style != JsonStyle::None {
        write_config(
            &config_path,
            json_style,
            &options.binary,
            options.force,
            &mut report,
        )?;
    }
    write_instructions(
        &instructions_path,
        options.harness,
        options.force,
        &mut report,
    )?;
    report.notes.push(format!(
        "Restart {} so it picks up the new MCP server, then ask it to call `memory_search`.",
        options.harness.as_str()
    ));
    if options.harness == Harness::Agents {
        report.notes.push(
            "This harness has no known MCP configuration; the instructions name the CLI commands."
                .to_owned(),
        );
    }
    Ok(report)
}

/// Which configuration dialect a harness reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum JsonStyle {
    /// `{"mcpServers": {"name": {"command": …, "args": […]}}}`
    McpServers,
    /// `{"servers": {"name": {"type": "stdio", …}}}`
    VsCodeServers,
    /// `[mcp_servers.name]` in TOML.
    Toml,
    /// No configuration file at all.
    None,
}

/// Resolve one path for the requested scope.
fn scope_path(
    root: &Path,
    home: &Path,
    global: bool,
    project_relative: &str,
    user_relative: &str,
) -> PathBuf {
    if global {
        home.join(user_relative)
    } else {
        root.join(project_relative)
    }
}

/// Merge the MCP entry into a configuration file, creating it when absent.
fn write_config(
    path: &Path,
    style: JsonStyle,
    binary: &str,
    force: bool,
    report: &mut InstallReport,
) -> Result<()> {
    if style == JsonStyle::Toml {
        return write_toml_config(path, binary, force, report);
    }
    let existing = read_json(path)?;
    let mut document = existing.clone().unwrap_or_else(|| json!({}));
    let key = match style {
        JsonStyle::McpServers => "mcpServers",
        JsonStyle::VsCodeServers => "servers",
        JsonStyle::Toml | JsonStyle::None => unreachable!("handled above"),
    };
    let entry = match style {
        JsonStyle::VsCodeServers => json!({ "type": "stdio", "command": binary, "args": ["mcp"] }),
        _ => json!({ "command": binary, "args": ["mcp"] }),
    };
    let servers = document
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("{} is not a JSON object", path.display()))?
        .entry(key)
        .or_insert_with(|| Value::Object(Map::new()));
    let servers = servers
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("{}: {key} is not an object", path.display()))?;
    match servers.get(SERVER) {
        Some(current) if current == &entry => {
            report
                .skipped
                .push((path.to_path_buf(), "already installed".to_owned()));
            return Ok(());
        }
        Some(current) if !force => {
            report.skipped.push((
                path.to_path_buf(),
                format!("an entry already exists ({current}); pass --force to replace it"),
            ));
            return Ok(());
        }
        _ => {}
    }
    servers.insert(SERVER.to_owned(), entry);
    write_text(
        path,
        &format!("{}\n", serde_json::to_string_pretty(&document)?),
    )?;
    report.written.push(path.to_path_buf());
    Ok(())
}

/// Merge the MCP entry into a TOML configuration (Codex).
fn write_toml_config(
    path: &Path,
    binary: &str,
    force: bool,
    report: &mut InstallReport,
) -> Result<()> {
    let existing = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => {
            return Err(anyhow::Error::new(error)).context(format!("read {}", path.display()));
        }
    };
    let header = format!("[mcp_servers.{SERVER}]");
    let block = format!("{header}\ncommand = {binary:?}\nargs = [\"mcp\"]\n");
    if existing.contains(&header) {
        if !force {
            report.skipped.push((
                path.to_path_buf(),
                "an [mcp_servers.wayfinder] table already exists; pass --force to replace it"
                    .to_owned(),
            ));
            return Ok(());
        }
        // Replace from the table header to the next table header.
        let mut out = String::with_capacity(existing.len());
        let mut skipping = false;
        for line in existing.lines() {
            let is_header = line.trim_start().starts_with('[');
            if is_header {
                skipping = line.trim() == header;
            }
            if skipping {
                continue;
            }
            out.push_str(line);
            out.push('\n');
        }
        let separator = if out.ends_with("\n\n") || out.is_empty() {
            ""
        } else {
            "\n"
        };
        write_text(path, &format!("{out}{separator}{block}"))?;
        report.written.push(path.to_path_buf());
        return Ok(());
    }
    let separator = if existing.trim().is_empty() { "" } else { "\n" };
    write_text(path, &format!("{existing}{separator}{block}"))?;
    report.written.push(path.to_path_buf());
    Ok(())
}

/// Read one JSON file, tolerating absence.
fn read_json(path: &Path) -> Result<Option<Value>> {
    match fs::read_to_string(path) {
        Ok(text) if text.trim().is_empty() => Ok(Some(json!({}))),
        Ok(text) => serde_json::from_str(&text)
            .map(Some)
            .with_context(|| format!("parse {}", path.display())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(anyhow::Error::new(error)).context(format!("read {}", path.display())),
    }
}

/// Write the instruction snippet into one markdown file, between its markers.
fn write_instructions(
    path: &Path,
    harness: Harness,
    force: bool,
    report: &mut InstallReport,
) -> Result<()> {
    let snippet = instructions(harness);
    let existing = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => {
            return Err(anyhow::Error::new(error)).context(format!("read {}", path.display()));
        }
    };
    let updated = match (existing.find(BEGIN), existing.find(END)) {
        (Some(start), Some(end)) if end > start => {
            let current = &existing[start..end + END.len()];
            if current == snippet && !force {
                report.skipped.push((
                    path.to_path_buf(),
                    "instructions already current".to_owned(),
                ));
                return Ok(());
            }
            format!(
                "{}{}{}",
                &existing[..start],
                snippet,
                &existing[end + END.len()..]
            )
        }
        _ => {
            let separator = if existing.is_empty() || existing.ends_with("\n\n") {
                ""
            } else if existing.ends_with('\n') {
                "\n"
            } else {
                "\n\n"
            };
            format!("{existing}{separator}{snippet}\n")
        }
    };
    write_text(path, &updated)?;
    report.written.push(path.to_path_buf());
    Ok(())
}

/// The instruction block a model reads.
fn instructions(harness: Harness) -> String {
    let (search, save, read, check) = match harness {
        // A harness with the MCP server uses the tool names it exposes.
        Harness::Agents => (
            "`wayfinder search \"<question>\"`",
            "`wayfinder save --title \"…\" --body - --anchor <file>`",
            "`wayfinder read <id> --section <heading>`",
            "`wayfinder check <id>`",
        ),
        _ => (
            "`memory_search`",
            "`memory_save`",
            "`memory_read`",
            "`memory_check`",
        ),
    };
    format!(
        "{BEGIN}
## Workspace memory (wayfinder)

This repository keeps what earlier sessions learned in `.wayfinder/memory`. Search it before \
re-deriving: {search}, then {read} — a page section is cheaper than re-reading the files it \
summarizes.

When you learn something a future session would pay to know again, save it: {save}, with one \
`--anchor` (or `anchors` entry) per file the fact rests on. Cite the code, not your memory of it.

{check} reports `fresh`, `changed`, or `missing` per cited file. Re-read anything changed before \
trusting the page; a page whose files moved is a lead, not an answer.
{END}"
    )
}

/// Write one file, creating its directory.
fn write_text(path: &Path, text: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    fs::write(path, text).with_context(|| format!("write {}", path.display()))
}

/// The user's home directory, or the current directory when it cannot be read.
fn home_directory() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| PathBuf::from("."))
}
