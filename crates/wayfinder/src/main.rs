//! `wayfinder` — workspace memory for coding agents, from the terminal.

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand, ValueEnum};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use wayfinder_core::{
    Anchor, DigestOptions, GraphBounds, GraphFormat, Kind, SaveRequest, SearchFilters, Store,
    build_graph, check_page, digest, render_graph,
};

#[derive(Parser)]
#[command(
    name = "wayfinder",
    version,
    about = "Workspace memory that agents write to and reuse",
    long_about = "Wayfinder keeps a pool of markdown pages under .wayfinder/memory: facts, decisions, \
gotchas, and pointers an agent wrote in one session and a later session reads instead of \
re-deriving. Pages cite files with fingerprints, saves are fenced by content revision, and the \
pool draws as a knowledge graph.\n\nStart with `wayfinder init`, then `wayfinder search`.",
    after_help = "Examples:\n  wayfinder init\n  wayfinder save --title \"Rate limits\" --body - --anchor src/limiter.rs:42\n  wayfinder search \"rate limit\"\n  wayfinder read rate-limits --section Throughput\n  wayfinder check"
)]
struct Cli {
    /// Project root; defaults to the nearest ancestor holding .wayfinder or .git.
    #[arg(long, global = true, value_name = "DIR")]
    root: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Create the pool in this project and build its index.
    Init {
        /// Also write one page per package, derived from manifests and READMEs.
        #[arg(long)]
        seed: bool,
    },
    /// Write or update one page.
    Save(SaveArgs),
    /// Print one page, or one section of it.
    Read {
        /// Page id.
        id: String,
        /// Heading to return; matching is case-insensitive and partial.
        #[arg(long)]
        section: Option<String>,
        /// Print the file as it is on disk.
        #[arg(long)]
        raw: bool,
    },
    /// Find pages by text, widened over declared relations.
    Search {
        /// What to look for; empty lists the most recent pages.
        query: Vec<String>,
        /// Maximum pages to print.
        #[arg(long, short, default_value_t = 8)]
        limit: usize,
        /// Only pages of this kind.
        #[arg(long)]
        kind: Option<KindArg>,
        /// Only pages declaring this subject.
        #[arg(long)]
        subject: Option<String>,
        /// Machine-readable output.
        #[arg(long)]
        json: bool,
    },
    /// List pages, newest first.
    List {
        /// Maximum pages to print.
        #[arg(long, short, default_value_t = 20)]
        limit: usize,
        /// Machine-readable output.
        #[arg(long)]
        json: bool,
    },
    /// Verify the files each page cites.
    Check {
        /// Page to check; absent checks the whole pool.
        id: Option<String>,
        /// Machine-readable output.
        #[arg(long)]
        json: bool,
    },
    /// Print the block a harness injects into the prompt.
    Digest {
        /// Maximum pages to list.
        #[arg(long, default_value_t = 8)]
        pages: usize,
        /// Token budget for the block.
        #[arg(long, default_value_t = 500)]
        tokens: usize,
    },
    /// Draw the pool as a knowledge graph.
    Graph {
        /// Output format.
        #[arg(long, default_value = "svg")]
        format: String,
        /// Write to this file instead of stdout.
        #[arg(long, short)]
        out: Option<PathBuf>,
        /// Draw only one subject's neighbourhood.
        #[arg(long)]
        subject: Option<String>,
    },
    /// Rebuild the derived index from the page files.
    Reindex,
    /// Show what the pool holds.
    Status {
        /// Machine-readable output.
        #[arg(long)]
        json: bool,
    },
    /// Install Wayfinder into a harness: MCP entry and instruction snippet.
    Install {
        /// Harness to install into.
        harness: HarnessArg,
        /// Write to the user-level config instead of this project.
        #[arg(long)]
        global: bool,
        /// Overwrite an existing entry.
        #[arg(long)]
        force: bool,
    },
    /// Serve the pool over MCP on stdin/stdout.
    Mcp {
        /// Tool-name root: `memory` (the harness surface) or `wayfinder`.
        #[arg(long, default_value = "memory")]
        prefix: String,
    },
    /// Write pages with a model, behind an explicit spend.
    Seed {
        /// Write one page per package from its manifest and README: no model.
        #[arg(long)]
        structure: bool,
        /// Upgrade those pages with a model-written one; states the cost first.
        #[arg(long)]
        pages: bool,
        /// Only report what a run would do.
        #[arg(long)]
        plan: bool,
        /// OpenAI-compatible endpoint to call.
        #[arg(long, default_value = "https://api.deepseek.com/v1/chat/completions")]
        endpoint: String,
        /// Model name to request.
        #[arg(long)]
        model: Option<String>,
        /// Maximum packages to write.
        #[arg(long)]
        limit: Option<usize>,
    },
}

#[derive(clap::Args)]
struct SaveArgs {
    /// Page title.
    #[arg(long)]
    title: String,
    /// Page id; derived from the title when absent.
    #[arg(long)]
    id: Option<String>,
    /// Body text; `-` reads stdin.
    #[arg(long)]
    body: Option<String>,
    /// Read the body from a file.
    #[arg(long, value_name = "FILE")]
    body_file: Option<PathBuf>,
    /// Tags to merge; repeatable and comma-separated.
    #[arg(long, value_delimiter = ',')]
    tags: Vec<String>,
    /// What the page is for.
    #[arg(long)]
    kind: Option<KindArg>,
    /// Subject the page is about; repeatable.
    #[arg(long = "subject")]
    subjects: Vec<String>,
    /// Package this page depends on; repeatable.
    #[arg(long)]
    depends: Vec<String>,
    /// Page this one replaces.
    #[arg(long)]
    supersedes: Option<String>,
    /// Where the page came from.
    #[arg(long)]
    source: Option<String>,
    /// File the page cites, as path, path:line, or path@sha; repeatable.
    #[arg(long, value_name = "PATH")]
    anchor: Vec<String>,
    /// Revision the caller read; a different current revision is refused.
    #[arg(long)]
    expected_rev: Option<String>,
    /// Overwrite a file whose frontmatter cannot be parsed.
    #[arg(long)]
    force: bool,
}

#[derive(Copy, Clone, ValueEnum)]
enum KindArg {
    Fact,
    Decision,
    Gotcha,
    Pointer,
}

impl From<KindArg> for Kind {
    fn from(value: KindArg) -> Self {
        match value {
            KindArg::Fact => Kind::Fact,
            KindArg::Decision => Kind::Decision,
            KindArg::Gotcha => Kind::Gotcha,
            KindArg::Pointer => Kind::Pointer,
        }
    }
}

#[derive(Copy, Clone, ValueEnum)]
enum HarnessArg {
    Claude,
    Codex,
    Copilot,
    Cursor,
    Gemini,
    Agents,
}

impl From<HarnessArg> for install::Harness {
    fn from(value: HarnessArg) -> Self {
        match value {
            HarnessArg::Claude => install::Harness::Claude,
            HarnessArg::Codex => install::Harness::Codex,
            HarnessArg::Copilot => install::Harness::Copilot,
            HarnessArg::Cursor => install::Harness::Cursor,
            HarnessArg::Gemini => install::Harness::Gemini,
            HarnessArg::Agents => install::Harness::Agents,
        }
    }
}

mod install;
mod mcp;

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli) {
        Ok(code) => code,
        Err(error) => {
            eprintln!("wayfinder: {error:#}");
            ExitCode::from(1)
        }
    }
}

fn run(cli: Cli) -> Result<ExitCode> {
    let root = cli
        .root
        .clone()
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    match cli.command {
        Command::Init { seed } => {
            let store = Store::init(&root)?;
            let stats = store.reindex()?;
            println!("wayfinder: pool at {}", store.paths().pool.display());
            println!("  {} page(s) indexed", stats.indexed);
            if seed {
                println!(
                    "  structure seeding lands in the next milestone; run `wayfinder seed --structure`"
                );
            }
            Ok(ExitCode::SUCCESS)
        }
        Command::Save(args) => save(&open(&root)?, args),
        Command::Read { id, section, raw } => read(&open(&root)?, &id, section.as_deref(), raw),
        Command::Search {
            query,
            limit,
            kind,
            subject,
            json,
        } => search(&open(&root)?, &query.join(" "), limit, kind, subject, json),
        Command::List { limit, json } => list(&open(&root)?, limit, json),
        Command::Check { id, json } => check(&open(&root)?, id.as_deref(), json),
        Command::Digest { pages, tokens } => {
            let store = open(&root)?;
            let text = digest(&store, DigestOptions { pages, tokens })?;
            if text.is_empty() {
                eprintln!("wayfinder: the pool is empty; nothing to digest");
            } else {
                print!("{text}");
            }
            Ok(ExitCode::SUCCESS)
        }
        Command::Graph {
            format,
            out,
            subject,
        } => graph(&open(&root)?, &format, out, subject),
        Command::Reindex => {
            let store = open(&root)?;
            let stats = store.reindex()?;
            println!(
                "wayfinder: {} indexed, {} removed, {} unchanged",
                stats.indexed, stats.removed, stats.unchanged
            );
            Ok(ExitCode::SUCCESS)
        }
        Command::Status { json } => status(&open(&root)?, json),
        Command::Install {
            harness,
            global,
            force,
        } => install(
            &root,
            install::InstallOptions {
                harness: harness.into(),
                global,
                force,
                binary: "wayfinder".to_owned(),
            },
        ),
        Command::Mcp { prefix } => {
            let prefix = mcp::Prefix::parse(&prefix).ok_or_else(|| {
                anyhow::anyhow!("unknown prefix {prefix:?}; use memory or wayfinder")
            })?;
            mcp::serve(open(&root)?, prefix)?;
            Ok(ExitCode::SUCCESS)
        }
        Command::Seed { .. } => bail!("`wayfinder seed` lands in the next milestone"),
    }
}

/// Open the pool, pointing a first-time user at `init`.
fn open(root: &Path) -> Result<Store> {
    Store::open(root).map_err(|error| anyhow::anyhow!("{error}"))
}

fn save(store: &Store, args: SaveArgs) -> Result<ExitCode> {
    let body = match (&args.body, &args.body_file) {
        (Some(text), None) if text == "-" => read_stdin()?,
        (Some(text), None) => text.clone(),
        (None, Some(path)) => std::fs::read_to_string(path)
            .with_context(|| format!("read body from {}", path.display()))?,
        (None, None) => String::new(),
        (Some(_), Some(_)) => bail!("pass either --body or --body-file, not both"),
    };
    let request = SaveRequest {
        id: args.id,
        title: args.title,
        body,
        tags: args.tags,
        kind: args.kind.map(Kind::from),
        subjects: args.subjects,
        depends: args.depends,
        supersedes: args.supersedes,
        source: args.source,
        anchors: args.anchor.iter().map(|spec| Anchor::parse(spec)).collect(),
        expected_rev: args.expected_rev,
        force: args.force,
    };
    let result = store
        .save(&request)
        .map_err(|error| anyhow::anyhow!("{error}"))?;
    println!(
        "{} \"{}\" (id: {}, rev: {})",
        if result.created { "Saved" } else { "Updated" },
        request.title,
        result.id,
        result.rev
    );
    if !result.unresolved_anchors.is_empty() {
        eprintln!(
            "wayfinder: no digest recorded for {}: {}",
            if result.unresolved_anchors.len() == 1 {
                "this anchor"
            } else {
                "these anchors"
            },
            result.unresolved_anchors.join(", ")
        );
    }
    Ok(ExitCode::SUCCESS)
}

fn read(store: &Store, id: &str, section: Option<&str>, raw: bool) -> Result<ExitCode> {
    if raw {
        let path = store.paths().page(id);
        let text =
            std::fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
        print!("{text}");
        return Ok(ExitCode::SUCCESS);
    }
    let outcome = store
        .read_section(id, section)
        .map_err(|error| anyhow::anyhow!("{error}"))?;
    let page = &outcome.page;
    let meta = {
        let mut parts = vec![format!("id: {}", page.id)];
        if !page.front.tags.is_empty() {
            parts.push(format!("tags: {}", page.front.tags.join(", ")));
        }
        if let Some(kind) = page.front.kind {
            parts.push(format!("kind: {}", kind.as_str()));
        }
        if let Some(updated) = &page.front.updated {
            parts.push(format!("updated: {updated}"));
        }
        parts.push(format!("rev: {}", page.rev()));
        parts.join(", ")
    };
    println!("Read memory page \"{}\" ({meta})", page.front.title);
    if section.is_some() && !outcome.matched {
        eprintln!(
            "wayfinder: no section matched {:?}; printing the whole page",
            section.unwrap_or_default()
        );
    }
    println!();
    println!("{}", outcome.section.text);
    Ok(ExitCode::SUCCESS)
}

fn search(
    store: &Store,
    query: &str,
    limit: usize,
    kind: Option<KindArg>,
    subject: Option<String>,
    json: bool,
) -> Result<ExitCode> {
    let filters = SearchFilters {
        kind: kind.map(Kind::from),
        subject,
        limit: Some(limit),
    };
    let outcome = store
        .search(query, &filters)
        .map_err(|error| anyhow::anyhow!("{error}"))?;
    if json {
        println!("{}", serde_json::to_string_pretty(&json_search(&outcome))?);
        return Ok(ExitCode::SUCCESS);
    }
    if outcome.pages.is_empty() {
        println!(
            "No memory pages matched. Save what you learn with `wayfinder save --title … --body …` so a later session can find it."
        );
        return Ok(ExitCode::SUCCESS);
    }
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
        println!("{}. {} ({})", index + 1, hit.title, meta.join(", "));
        if !hit.snippet.is_empty() {
            for line in hit.snippet.lines().take(6) {
                println!("   {line}");
            }
        }
    }
    if outcome.expanded > 0 {
        println!(
            "\n{} of these came from a declared relation, not the query itself.",
            outcome.expanded
        );
    }
    Ok(ExitCode::SUCCESS)
}

fn json_search(outcome: &wayfinder_core::SearchOutcome) -> serde_json::Value {
    serde_json::json!({
        "query": outcome.query,
        "expanded": outcome.expanded,
        "pages": outcome.pages.iter().map(|hit| serde_json::json!({
            "id": hit.id,
            "title": hit.title,
            "kind": hit.kind.map(|kind| kind.as_str()),
            "tags": hit.tags,
            "updated": hit.updated,
            "rev": hit.rev,
            "score": hit.score,
            "via": hit.via,
            "anchors": hit.anchors,
            "snippet": hit.snippet,
        })).collect::<Vec<_>>(),
    })
}

fn list(store: &Store, limit: usize, json: bool) -> Result<ExitCode> {
    let index = store.index().map_err(|error| anyhow::anyhow!("{error}"))?;
    index
        .sync(store.paths())
        .map_err(|error| anyhow::anyhow!("{error}"))?;
    let hits = index
        .list(&SearchFilters {
            kind: None,
            subject: None,
            limit: Some(limit),
        })
        .map_err(|error| anyhow::anyhow!("{error}"))?;
    if json {
        let rows: Vec<serde_json::Value> = hits
            .iter()
            .filter_map(|hit| store.read(&hit.id).ok())
            .map(|page| {
                serde_json::json!({
                    "id": page.id,
                    "title": page.front.title,
                    "kind": page.front.kind.map(|kind| kind.as_str()),
                    "tags": page.front.tags,
                    "updated": page.front.updated,
                    "rev": page.rev(),
                    "anchors": page.front.anchors.len(),
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&rows)?);
        return Ok(ExitCode::SUCCESS);
    }
    if hits.is_empty() {
        println!("The pool is empty. `wayfinder save` writes the first page.");
        return Ok(ExitCode::SUCCESS);
    }
    for hit in hits {
        if let Ok(page) = store.read(&hit.id) {
            let stamp = page
                .front
                .updated
                .as_deref()
                .map(|updated| updated.get(..10).unwrap_or(updated))
                .unwrap_or("unknown");
            let kind = page
                .front
                .kind
                .map(|kind| format!(", {}", kind.as_str()))
                .unwrap_or_default();
            println!(
                "- {} (id: {}, updated {stamp}{kind})",
                page.front.title, page.id
            );
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn check(store: &Store, id: Option<&str>, json: bool) -> Result<ExitCode> {
    let pages = match id {
        Some(id) => vec![store.read(id).map_err(|error| anyhow::anyhow!("{error}"))?],
        None => store.pages().map_err(|error| anyhow::anyhow!("{error}"))?,
    };
    let mut reports = Vec::new();
    for page in pages {
        reports.push(check_page(store.root(), &page).map_err(|error| anyhow::anyhow!("{error}"))?);
    }
    let drifted: usize = reports.iter().map(|report| report.drifted()).sum();
    if json {
        let rows: Vec<serde_json::Value> = reports
            .iter()
            .map(|report| {
                serde_json::json!({
                    "id": report.id,
                    "title": report.title,
                    "rev": report.rev,
                    "fresh": report.all_fresh,
                    "drifted": report.drifted(),
                    "anchors": report.verdicts.iter().map(|verdict| serde_json::json!({
                        "path": verdict.anchor.spec(),
                        "state": verdict.state.as_str(),
                        "detail": verdict.detail,
                    })).collect::<Vec<_>>(),
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&rows)?);
        return Ok(if drifted > 0 {
            ExitCode::from(1)
        } else {
            ExitCode::SUCCESS
        });
    }
    if reports.iter().all(|report| report.verdicts.is_empty()) {
        println!("No page declares an anchor, so there is nothing to verify.");
        println!("Cite files with `wayfinder save --anchor path[:line]` to make a page checkable.");
        return Ok(ExitCode::SUCCESS);
    }
    for report in &reports {
        if report.verdicts.is_empty() {
            continue;
        }
        if report.all_fresh {
            println!(
                "Every cited file still matches memory page \"{}\". Trust the page; no need to re-read those files.",
                report.id
            );
            continue;
        }
        println!(
            "Memory page \"{}\" cites files that changed. Re-read these before trusting the page:",
            report.id
        );
        for verdict in &report.verdicts {
            match verdict.state {
                wayfinder_core::AnchorState::Fresh => println!("  ok {}", verdict.anchor.spec()),
                wayfinder_core::AnchorState::Changed => println!(
                    "  changed {} ({})",
                    verdict.anchor.spec(),
                    verdict.detail.as_deref().unwrap_or("differs")
                ),
                wayfinder_core::AnchorState::Missing => println!(
                    "  missing {} ({})",
                    verdict.anchor.spec(),
                    verdict.detail.as_deref().unwrap_or("gone")
                ),
                wayfinder_core::AnchorState::Unverified => {
                    println!(
                        "  unverified {} (no fingerprint recorded)",
                        verdict.anchor.spec()
                    )
                }
            }
        }
    }
    Ok(if drifted > 0 {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    })
}

fn graph(
    store: &Store,
    format: &str,
    out: Option<PathBuf>,
    subject: Option<String>,
) -> Result<ExitCode> {
    let format = GraphFormat::parse(format).ok_or_else(|| {
        anyhow::anyhow!("unknown format {format:?}; use json, dot, svg, html, or mermaid")
    })?;
    let view = build_graph(store, GraphBounds::default(), subject.as_deref())
        .map_err(|error| anyhow::anyhow!("{error}"))?;
    let rendered = render_graph(&view, format).map_err(|error| anyhow::anyhow!("{error}"))?;
    match out {
        Some(path) => {
            std::fs::write(&path, rendered).with_context(|| format!("write {}", path.display()))?;
            println!(
                "wayfinder: wrote {} ({} nodes, {} edges)",
                path.display(),
                view.nodes.len(),
                view.edges.len()
            );
        }
        None => print!("{rendered}"),
    }
    Ok(ExitCode::SUCCESS)
}

fn status(store: &Store, json: bool) -> Result<ExitCode> {
    let index = store.index().map_err(|error| anyhow::anyhow!("{error}"))?;
    let stats = index
        .sync(store.paths())
        .map_err(|error| anyhow::anyhow!("{error}"))?;
    let pages = index
        .page_count()
        .map_err(|error| anyhow::anyhow!("{error}"))?;
    let edges = index
        .edges()
        .map_err(|error| anyhow::anyhow!("{error}"))?
        .len();
    let mut drift = 0usize;
    for page in store.pages().map_err(|error| anyhow::anyhow!("{error}"))? {
        if let Ok(report) = check_page(store.root(), &page) {
            drift += report.drifted();
        }
    }
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "root": store.root().display().to_string(),
                "pool": store.paths().pool.display().to_string(),
                "pages": pages,
                "edges": edges,
                "driftedAnchors": drift,
                "lastSync": { "indexed": stats.indexed, "removed": stats.removed, "unchanged": stats.unchanged },
            }))?
        );
        return Ok(ExitCode::SUCCESS);
    }
    println!("pool     {}", store.paths().pool.display());
    println!("root     {}", store.root().display());
    println!("pages    {pages}");
    println!("edges    {edges}");
    println!("drifted  {drift} anchor(s) no longer match their file");
    if stats.indexed + stats.removed > 0 {
        println!(
            "index    refreshed {} page(s), dropped {}",
            stats.indexed, stats.removed
        );
    }
    Ok(ExitCode::SUCCESS)
}

fn install(root: &Path, options: install::InstallOptions) -> Result<ExitCode> {
    let report = install::install(root, &options)?;
    for path in &report.written {
        println!("wrote {}", path.display());
    }
    for (path, reason) in &report.skipped {
        println!("left {} alone: {reason}", path.display());
    }
    for note in &report.notes {
        println!("{note}");
    }
    Ok(ExitCode::SUCCESS)
}

/// Read all of stdin as a string.
fn read_stdin() -> Result<String> {
    use std::io::Read;
    let mut buffer = String::new();
    std::io::stdin()
        .read_to_string(&mut buffer)
        .context("read body from stdin")?;
    Ok(buffer)
}
