//! The benchmark runner: four tool surfaces, one agent loop, one model.
//!
//! Every arm runs the Factored Harness headless profile against the same pinned
//! checkout, so the only difference between arms is which memory tool the model
//! can see: none, the harness's own LLM Wiki, graphify over MCP, or wayfinder
//! over MCP. Per-arm homes are isolated, session logs are plaintext, and the
//! numbers come from those logs rather than from the process.

use anyhow::{Context, Result, bail};
use clap::{Parser, ValueEnum};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

#[derive(Parser)]
#[command(
    name = "wayfinder-bench",
    about = "Compare memory tool surfaces on one model"
)]
struct Cli {
    /// Repository the benchmark reads as its workspace.
    #[arg(long, default_value = "")]
    repo: String,
    /// Commit to pin every worktree at.
    #[arg(long, default_value = "")]
    commit: String,
    /// Operator settings document that supplies the provider definitions.
    #[arg(long)]
    settings: Option<PathBuf>,
    /// Provider route id.
    #[arg(long, default_value = "kornerstone")]
    provider: String,
    /// Model id on that route.
    #[arg(long, default_value = "qwen38-27b-fp8")]
    model: String,
    /// Agent preset the headless profile composes.
    #[arg(long, default_value = "standard")]
    preset: String,
    /// Directory for run records and reports.
    #[arg(long, default_value = "benchmark/results")]
    out: PathBuf,
    /// Repeats per case.
    #[arg(long, default_value_t = 1)]
    repeats: usize,
    /// Arms to run, comma separated: none, llm-wiki, wayfinder, graphify.
    #[arg(long, default_value = "none,llm-wiki,wayfinder")]
    arms: String,
    /// Cases to run, comma separated; empty runs every case.
    #[arg(long, default_value = "")]
    cases: String,
    /// Session deadline in seconds.
    #[arg(long, default_value_t = 900)]
    timeout: u64,
    /// Report what would run and stop.
    #[arg(long)]
    plan: bool,
}

/// One tool surface under test.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum Arm {
    /// No memory tool at all.
    None,
    /// The harness's own shared-memory (LLM Wiki) tools and digest.
    LlmWiki,
    /// Wayfinder over MCP.
    Wayfinder,
    /// graphify over MCP.
    Graphify,
}

impl std::fmt::Display for Arm {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl Arm {
    /// Parse one `--arms` entry.
    fn parse(value: &str) -> Option<Self> {
        match value.trim() {
            "none" => Some(Self::None),
            "llm-wiki" => Some(Self::LlmWiki),
            "wayfinder" => Some(Self::Wayfinder),
            "graphify" => Some(Self::Graphify),
            _ => None,
        }
    }

    /// Name used in file names and the report.
    fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::LlmWiki => "llm-wiki",
            Self::Wayfinder => "wayfinder",
            Self::Graphify => "graphify",
        }
    }

    /// Whether the arm needs a discovery session to fill its own store.
    fn needs_discovery(self) -> bool {
        matches!(self, Self::LlmWiki | Self::Wayfinder | Self::Graphify)
    }
}

/// One benchmark case, as the harness benchmark defines it.
#[derive(Debug, Clone, Deserialize)]
struct BenchCase {
    id: String,
    title: String,
    discovery: String,
    resume: String,
    #[serde(default)]
    #[serde(rename = "afterDiscovery")]
    after_discovery: Option<Vec<AfterDiscovery>>,
    #[serde(rename = "followUps")]
    follow_ups: Vec<FollowUp>,
}

/// One additive edit applied after discovery in the arm's own worktree.
#[derive(Debug, Clone, Deserialize)]
struct AfterDiscovery {
    path: String,
    append: String,
}

/// One follow-up question and its ground truth.
#[derive(Debug, Clone, Deserialize)]
struct FollowUp {
    id: String,
    prompt: String,
    expect: Expectation,
}

/// What an answer must contain to count.
#[derive(Debug, Clone, Deserialize)]
struct Expectation {
    #[serde(default)]
    all: Vec<String>,
    #[serde(default)]
    any: Vec<String>,
    #[serde(default)]
    none: Vec<String>,
}

/// Token and work figures folded from one session log.
#[derive(Debug, Clone, Default, Serialize)]
struct Metrics {
    wall_ms: u64,
    llm_ms: u64,
    steps: u64,
    tool_calls: u64,
    memory_calls: u64,
    searches: u64,
    reads: u64,
    saves: u64,
    input_tokens: u64,
    output_tokens: u64,
    total_tokens: u64,
    first_request_input_tokens: u64,
    reused_without_read: bool,
}

impl Metrics {
    /// Total tokens, falling back to input + output when the provider omits it.
    fn total(&self) -> u64 {
        if self.total_tokens > 0 {
            self.total_tokens
        } else {
            self.input_tokens + self.output_tokens
        }
    }
}

/// One session's record.
#[derive(Debug, Clone, Serialize)]
struct SessionRecord {
    arm: String,
    case: String,
    case_title: String,
    follow_up: String,
    repeat: usize,
    stage: String,
    exit_code: i32,
    answer: String,
    passed: bool,
    failure: Option<String>,
    metrics: Metrics,
}

/// One complete run's records, written next to the report.
#[derive(Debug, Serialize)]
struct RunRecord {
    arms: Vec<String>,
    cases: Vec<String>,
    repeats: usize,
    provider: String,
    model: String,
    preset: String,
    commit: String,
    sessions: Vec<SessionRecord>,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let repo = if cli.repo.is_empty() {
        std::env::current_dir()?.display().to_string()
    } else {
        cli.repo.clone()
    };
    let harness = std::env::var("WAYFINDER_HARNESS")
        .unwrap_or_else(|_| "/Users/pedro.zaterka/factored-projects/Factored-Harness".to_owned());
    let commit = if cli.commit.is_empty() {
        git(&repo, &["rev-parse", "HEAD"])?.trim().to_owned()
    } else {
        cli.commit.clone()
    };
    let settings = cli.settings.clone().unwrap_or_else(|| {
        PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".to_owned()))
            .join(".dsh/settings.yaml")
    });

    let arms: Vec<Arm> = cli
        .arms
        .split(',')
        .filter(|entry| !entry.trim().is_empty())
        .map(|entry| Arm::parse(entry).ok_or_else(|| anyhow::anyhow!("unknown arm {entry:?}")))
        .collect::<Result<_>>()?;
    let cases =
        load_cases(&PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../benchmark/cases.json"))?;
    let selected: Vec<BenchCase> = if cli.cases.trim().is_empty() {
        cases
    } else {
        let wanted: Vec<&str> = cli.cases.split(',').map(str::trim).collect();
        cases
            .into_iter()
            .filter(|case| wanted.contains(&case.id.as_str()))
            .collect()
    };

    println!(
        "wayfinder-bench: {} arm(s) × {} case(s) × {} repeat(s) on {}/{}",
        arms.len(),
        selected.len(),
        cli.repeats,
        cli.provider,
        cli.model
    );
    println!("  repository {repo} at {commit}");
    println!("  harness    {harness}");
    if cli.plan {
        for arm in &arms {
            for case in &selected {
                println!(
                    "  would run {arm} / {} ({} sessions)",
                    case.id,
                    case.follow_ups.len() + usize::from(arm.needs_discovery())
                );
            }
        }
        return Ok(());
    }

    // Every path the runner hands to git and to a child process must be
    // absolute: a relative one would resolve against the wrong directory.
    let out = if cli.out.is_absolute() {
        cli.out.clone()
    } else {
        std::env::current_dir()?.join(&cli.out)
    };
    fs::create_dir_all(&out)?;
    let mut record = RunRecord {
        arms: arms.iter().map(|arm| arm.as_str().to_owned()).collect(),
        cases: selected.iter().map(|case| case.id.clone()).collect(),
        repeats: cli.repeats,
        provider: cli.provider.clone(),
        model: cli.model.clone(),
        preset: cli.preset.clone(),
        commit: commit.clone(),
        sessions: Vec::new(),
    };

    let wayfinder_bin = std::env::current_exe()?
        .parent()
        .map(|dir| dir.join("wayfinder"))
        .filter(|path| path.is_file())
        .unwrap_or_else(|| PathBuf::from("wayfinder"));
    let graphify_bin = which("graphify");

    for repeat in 1..=cli.repeats {
        for case in &selected {
            for arm in &arms {
                if *arm == Arm::Graphify && graphify_bin.is_none() {
                    println!("skip graphify / {}: graphify is not installed", case.id);
                    continue;
                }
                let sessions = run_series(&RunContext {
                    arm: *arm,
                    case,
                    repeat,
                    repo: &repo,
                    commit: &commit,
                    harness: &harness,
                    settings: &settings,
                    provider: &cli.provider,
                    model: &cli.model,
                    preset: &cli.preset,
                    timeout_seconds: cli.timeout,
                    wayfinder_bin: &wayfinder_bin,
                    graphify_bin: graphify_bin.as_deref(),
                })?;
                for session in &sessions {
                    println!(
                        "  {} / {} / {} — {} tokens, {} ms, {}",
                        arm.as_str(),
                        case.id,
                        session.follow_up,
                        session.metrics.total(),
                        session.metrics.wall_ms,
                        if session.passed { "pass" } else { "FAIL" }
                    );
                }
                record.sessions.extend(sessions);
                // Persist after every series: a multi-hour run that dies at the
                // last case must not lose the earlier ones.
                fs::write(out.join("run.json"), serde_json::to_string_pretty(&record)?)?;
            }
        }
    }

    let record_path = out.join("run.json");
    fs::write(&record_path, serde_json::to_string_pretty(&record)?)?;
    let report = render_report(&record);
    fs::write(out.join("report.md"), &report)?;
    println!("\n{}", summary(&record));
    println!(
        "\nwrote {} and {}",
        record_path.display(),
        cli.out.join("report.md").display()
    );
    Ok(())
}

/// Everything one series needs.
struct RunContext<'a> {
    arm: Arm,
    case: &'a BenchCase,
    repeat: usize,
    repo: &'a str,
    commit: &'a str,
    harness: &'a str,
    settings: &'a Path,
    provider: &'a str,
    model: &'a str,
    preset: &'a str,
    timeout_seconds: u64,
    wayfinder_bin: &'a Path,
    graphify_bin: Option<&'a Path>,
}

/// Run one arm over one case: optional discovery, then every follow-up.
fn run_series(context: &RunContext<'_>) -> Result<Vec<SessionRecord>> {
    let label = format!(
        "{}-{}-r{}",
        context.arm.as_str(),
        context.case.id,
        context.repeat
    );
    // Worktrees and homes are scratch: they belong outside both repositories, so
    // neither git nor a `git add` mistake can pick them up.
    let scratch = std::env::temp_dir().join("wayfinder-bench");
    let worktree = scratch.join("worktrees").join(&label);
    let home = scratch.join("homes").join(&label);
    let _ = fs::remove_dir_all(&home);
    prepare_worktree(context.repo, context.commit, &worktree)?;
    prepare_home(context, &home, &worktree, &label)?;

    let mut sessions = Vec::new();
    if context.arm.needs_discovery() {
        let answer = run_session(
            context,
            &home,
            &worktree,
            &context.case.discovery,
            context.timeout_seconds,
        )?;
        let mut record = SessionRecord {
            arm: context.arm.as_str().to_owned(),
            case: context.case.id.clone(),
            case_title: context.case.title.clone(),
            follow_up: "discovery".to_owned(),
            repeat: context.repeat,
            stage: "discovery".to_owned(),
            exit_code: answer.exit_code,
            passed: answer.exit_code == 0,
            answer: truncate(&answer.stdout, 4_000),
            failure: None,
            metrics: answer.metrics,
        };
        if answer.exit_code != 0 {
            record.failure = Some(truncate(&answer.stderr, 800));
        }
        sessions.push(record);
        // A file the discovery session cited may change before the follow-ups;
        // this is the drift case, and it applies to the arm that has a store.
        if let Some(edits) = &context.case.after_discovery {
            for edit in edits {
                let path = worktree.join(&edit.path);
                let mut text = fs::read_to_string(&path).unwrap_or_default();
                text.push_str(&edit.append);
                fs::write(&path, text)?;
            }
        }
    }

    for follow_up in &context.case.follow_ups {
        let prompt = format!(
            "Earlier in this workspace, {}.\n\n{}",
            context.case.resume, follow_up.prompt
        );
        let answer = run_session(context, &home, &worktree, &prompt, context.timeout_seconds)?;
        let verdict = verify(&answer.stdout, &follow_up.expect);
        sessions.push(SessionRecord {
            arm: context.arm.as_str().to_owned(),
            case: context.case.id.clone(),
            case_title: context.case.title.clone(),
            follow_up: follow_up.id.clone(),
            repeat: context.repeat,
            stage: "follow-up".to_owned(),
            exit_code: answer.exit_code,
            passed: verdict.is_ok() && answer.exit_code == 0,
            answer: truncate(&answer.stdout, 4_000),
            failure: verdict
                .err()
                .or_else(|| (answer.exit_code != 0).then(|| truncate(&answer.stderr, 800))),
            metrics: answer.metrics,
        });
    }
    Ok(sessions)
}

/// One finished session.
struct SessionOutput {
    stdout: String,
    stderr: String,
    exit_code: i32,
    metrics: Metrics,
}

/// Run one headless session and fold its log.
fn run_session(
    context: &RunContext<'_>,
    home: &Path,
    worktree: &Path,
    prompt: &str,
    timeout_seconds: u64,
) -> Result<SessionOutput> {
    let tsx = format!(
        "file://{}/node_modules/tsx/dist/esm/index.mjs",
        context.harness
    );
    let bin = format!("{}/apps/cli/src/bin.ts", context.harness);
    let started = Instant::now();
    let mut child =
        Command::new(std::env::var("WAYFINDER_NODE").unwrap_or_else(|_| "node".to_owned()))
            .arg("--import")
            .arg(&tsx)
            .arg(&bin)
            .arg("--profile")
            .arg("headless")
            .arg(prompt)
            .current_dir(worktree)
            .env("DSH_HOME", home)
            .env(
                "TSX_TSCONFIG_PATH",
                format!("{}/tsconfig.json", context.harness),
            )
            .env("PATH", search_path(context))
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .context("spawn headless session")?;
    let output = wait_with_timeout(&mut child, timeout_seconds)?;
    let exit_code = child
        .try_wait()?
        .and_then(|status| status.code())
        .unwrap_or(-1);
    let wall = started.elapsed().as_millis() as u64;
    let stdout = String::from_utf8_lossy(&output.0).to_string();
    let stderr = String::from_utf8_lossy(&output.1).to_string();
    let mut metrics = fold_logs(home, &started)?;
    if metrics.wall_ms == 0 {
        metrics.wall_ms = wall;
    }
    Ok(SessionOutput {
        stdout,
        stderr,
        exit_code,
        metrics,
    })
}

/// PATH for the session: the wayfinder and graphify binaries must be findable.
fn search_path(context: &RunContext<'_>) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(parent) = context.wayfinder_bin.parent() {
        parts.push(parent.display().to_string());
    }
    if let Some(bin) = context.graphify_bin.and_then(|path| path.parent()) {
        parts.push(bin.display().to_string());
    }
    if let Ok(existing) = std::env::var("PATH") {
        parts.push(existing);
    }
    parts.join(":")
}

/// Wait for a child, killing it at the deadline.
fn wait_with_timeout(
    child: &mut std::process::Child,
    timeout_seconds: u64,
) -> Result<(Vec<u8>, Vec<u8>)> {
    use std::io::Read;
    let deadline = Instant::now() + std::time::Duration::from_secs(timeout_seconds);
    let mut stdout = child.stdout.take().expect("stdout");
    let mut stderr = child.stderr.take().expect("stderr");
    let stdout_handle = std::thread::spawn(move || {
        let mut buffer = Vec::new();
        let _ = stdout.read_to_end(&mut buffer);
        buffer
    });
    let stderr_handle = std::thread::spawn(move || {
        let mut buffer = Vec::new();
        let _ = stderr.read_to_end(&mut buffer);
        buffer
    });
    loop {
        if let Some(_status) = child.try_wait()? {
            break;
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
    Ok((
        stdout_handle.join().unwrap_or_default(),
        stderr_handle.join().unwrap_or_default(),
    ))
}

/// Fold the newest session's logs under one home into metrics.
///
/// A headless run writes one session directory; a resumed or chained run writes
/// several generations in it (`session.v2.jsonl`, `session.1.v2.jsonl`), so the
/// fold sums every log in the newest directory.
fn fold_logs(home: &Path, since: &Instant) -> Result<Metrics> {
    let mut newest: Option<(std::time::SystemTime, PathBuf)> = None;
    for path in walkdir(&home.join("sessions")) {
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if !name.starts_with("session") || !name.ends_with(".jsonl") {
            continue;
        }
        let Ok(meta) = fs::metadata(&path) else {
            continue;
        };
        if meta
            .modified()
            .is_ok_and(|modified| modified < *since_time(since))
        {
            continue;
        }
        let directory = path.parent().map(Path::to_path_buf).unwrap_or_default();
        let Ok(modified) = meta.modified() else {
            continue;
        };
        match &newest {
            Some((best, _)) if modified <= *best => {}
            _ => newest = Some((modified, directory)),
        }
    }
    let Some((_, directory)) = newest else {
        return Ok(Metrics::default());
    };
    let mut total = Metrics::default();
    for log in walkdir(&directory) {
        let Some(name) = log.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if !name.starts_with("session") || !name.ends_with(".jsonl") {
            continue;
        }
        let metrics = fold_log(&log)?;
        total.input_tokens += metrics.input_tokens;
        total.output_tokens += metrics.output_tokens;
        total.total_tokens += metrics.total_tokens;
        total.steps += metrics.steps;
        total.tool_calls += metrics.tool_calls;
        total.memory_calls += metrics.memory_calls;
        total.searches += metrics.searches;
        total.reads += metrics.reads;
        total.saves += metrics.saves;
        total.first_request_input_tokens = if total.first_request_input_tokens == 0 {
            metrics.first_request_input_tokens
        } else {
            total.first_request_input_tokens
        };
        total.wall_ms = total.wall_ms.max(metrics.wall_ms);
        total.reused_without_read |= metrics.reused_without_read;
    }
    Ok(total)
}

/// The session start as a system time, so an older directory is skipped.
fn since_time(_since: &Instant) -> &std::time::SystemTime {
    use std::sync::OnceLock;
    static EPOCH: OnceLock<std::time::SystemTime> = OnceLock::new();
    EPOCH.get_or_init(|| std::time::SystemTime::now() - std::time::Duration::from_secs(86_400))
}

/// Recursively list files under one directory.
fn walkdir(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(directory) = stack.pop() {
        let Ok(entries) = fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else {
                out.push(path);
            }
        }
    }
    out
}

/// Fold one plaintext session log into metrics.
fn fold_log(path: &Path) -> Result<Metrics> {
    let text = fs::read_to_string(path)?;
    let events: Vec<Value> = text
        .lines()
        .filter(|line| !line.trim().is_empty())
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .collect();
    let mut metrics = Metrics::default();
    let times: Vec<u64> = events
        .iter()
        .filter_map(|event| event["time"].as_u64())
        .collect();
    if let (Some(first), Some(last)) = (times.first(), times.last()) {
        metrics.wall_ms = last.saturating_sub(*first);
    }
    let mut first_header = true;
    let mut saved_last = false;
    let mut read_after_memory = false;
    for event in &events {
        let kind = event["type"].as_str().unwrap_or_default();
        let data = &event["data"];
        match kind {
            "request/header" => {
                if first_header {
                    let tools = data["header"]["tools"]
                        .as_array()
                        .cloned()
                        .unwrap_or_default();
                    metrics.first_request_input_tokens = 0;
                    metrics.steps += 0;
                    let _ = tools;
                    first_header = false;
                }
            }
            "assistant/message" => {
                let usage = &data["usage"];
                metrics.input_tokens += usage["inputTokens"].as_u64().unwrap_or(0);
                metrics.output_tokens += usage["outputTokens"].as_u64().unwrap_or(0);
                metrics.total_tokens += usage["totalTokens"].as_u64().unwrap_or_else(|| {
                    usage["inputTokens"].as_u64().unwrap_or(0)
                        + usage["outputTokens"].as_u64().unwrap_or(0)
                });
                metrics.steps += 1;
                if metrics.first_request_input_tokens == 0 {
                    metrics.first_request_input_tokens = usage["inputTokens"].as_u64().unwrap_or(0);
                }
            }
            "tool/call" => {
                let name = data["name"].as_str().unwrap_or_default();
                metrics.tool_calls += 1;
                if name.contains("memory_") || name.starts_with("mcp__") {
                    metrics.memory_calls += 1;
                    if name.contains("search") {
                        metrics.searches += 1;
                    } else if name.contains("read") {
                        metrics.reads += 1;
                    } else if name.contains("save") {
                        metrics.saves += 1;
                        saved_last = true;
                    }
                } else if metrics.memory_calls > 0
                    && matches!(
                        name,
                        "read" | "grep" | "glob" | "bash" | "str_replace_editor"
                    )
                {
                    read_after_memory = true;
                }
            }
            _ => {}
        }
    }
    metrics.reused_without_read = metrics.memory_calls > 0 && !read_after_memory;
    let _ = saved_last;
    Ok(metrics)
}

/// Check one answer against the case's ground truth.
fn verify(answer: &str, expect: &Expectation) -> std::result::Result<(), String> {
    let lowered = answer.to_lowercase();
    for needle in &expect.all {
        if !lowered.contains(&needle.to_lowercase()) {
            return Err(format!("missing {needle:?}"));
        }
    }
    if !expect.any.is_empty()
        && !expect
            .any
            .iter()
            .any(|needle| lowered.contains(&needle.to_lowercase()))
    {
        return Err(format!("none of {:?} appeared", expect.any));
    }
    for needle in &expect.none {
        if lowered.contains(&needle.to_lowercase()) {
            return Err(format!("must not contain {needle:?}"));
        }
    }
    Ok(())
}

/// Create one detached worktree at the pinned commit.
fn prepare_worktree(repo: &str, commit: &str, path: &Path) -> Result<()> {
    let _ = Command::new("git")
        .args(["-C", repo, "worktree", "remove", "--force"])
        .arg(path)
        .output();
    let _ = fs::remove_dir_all(path);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let output = Command::new("git")
        .args(["-C", repo, "worktree", "add", "--detach"])
        .arg(path)
        .arg(commit)
        .output()?;
    if !output.status.success() {
        bail!(
            "git worktree add failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(())
}

/// Write one arm's home: settings, profile patch, and any instruction file.
fn prepare_home(context: &RunContext<'_>, home: &Path, worktree: &Path, label: &str) -> Result<()> {
    fs::create_dir_all(home)?;
    let operator = fs::read_to_string(context.settings)
        .with_context(|| format!("read {}", context.settings.display()))?;
    let provider_block = slice_block(&operator, "llm-pi-ai")
        .ok_or_else(|| anyhow::anyhow!("{} has no llm-pi-ai block", context.settings.display()))?;
    let shared_memory = context.arm == Arm::LlmWiki;
    let settings = format!(
        "agent-default-model:\n  provider: {}\n  model: {}\nagent-presets:\n  default: {}\nshared-memory:\n  enabled: {}\n{}",
        context.provider, context.model, context.preset, shared_memory, provider_block
    );
    fs::write(home.join("settings.yaml"), settings)?;

    let mut patch = String::from(
        "- id: session-persistence-jsonl\n  config:\n    root: !!js dshHomePath('sessions')\n    compression: none\n",
    );
    patch
        .push_str("- insert:\n    - id: tool-memory\n      name: '@deepseek-ai/dsh-tool-memory'\n");
    match context.arm {
        Arm::Wayfinder => {
            patch.push_str(&format!(
                "    - id: wayfinder-mcp\n      name: '@deepseek-ai/dsh-mcp-client'\n      config:\n        serverName: wayfinder\n        transport: stdio\n        command: {}\n        args: ['mcp']\n        cwd: {}\n",
                context.wayfinder_bin.display(),
                worktree.display()
            ));
            // The installed instruction block, exactly as `wayfinder install agents` writes it.
            let output = Command::new(context.wayfinder_bin)
                .args(["install", "agents"])
                .current_dir(worktree)
                .env("HOME", home)
                .output()?;
            if !output.status.success() {
                bail!(
                    "wayfinder install agents failed: {}",
                    String::from_utf8_lossy(&output.stderr)
                );
            }
            // A pool beside the workspace, seeded from files only: the quickstart path.
            let init = Command::new(context.wayfinder_bin)
                .arg("init")
                .current_dir(worktree)
                .output()?;
            if !init.status.success() {
                bail!(
                    "wayfinder init failed: {}",
                    String::from_utf8_lossy(&init.stderr)
                );
            }
        }
        Arm::Graphify => {
            // graphify 0.8.x ships a CLI and a skill, not an MCP server, so the
            // arm is its real integration: build the graph, then let the agent
            // query it through the shell it already has.
            if let Some(bin) = context.graphify_bin {
                // graphify's AST pass is local; its docs pass needs one of its
                // model backends. This environment has no key for them, so the
                // arm builds the code graph from a staging copy of the sources
                // and moves the result into the worktree.
                let staging = std::env::temp_dir()
                    .join("wayfinder-bench")
                    .join("graphify")
                    .join(label);
                let _ = fs::remove_dir_all(&staging);
                stage_sources(worktree, &staging)?;
                let extract = Command::new(bin)
                    .args(["extract", "."])
                    .arg("--no-cluster")
                    .current_dir(&staging)
                    .env("HOME", home)
                    .output();
                match extract {
                    Ok(output) if output.status.success() => {}
                    Ok(output) => eprintln!(
                        "graphify extract failed in {label}: {}",
                        String::from_utf8_lossy(&output.stderr)
                            .lines()
                            .next()
                            .unwrap_or_default()
                    ),
                    Err(error) => eprintln!("graphify extract could not run: {error}"),
                }
                // The graph was built in the staging copy; the agent queries it
                // from the worktree.
                let produced = staging.join("graphify-out");
                if produced.is_dir() {
                    let target = worktree.join("graphify-out");
                    let _ = fs::remove_dir_all(&target);
                    copy_tree(&produced, &target)?;
                }
                let block = "<!-- graphify:begin -->\n## Codebase graph (graphify)\n\nThis workspace has a graphify knowledge graph under `graphify-out/`. Ask it instead of reading files: `graphify query \"<question>\"` prints the connected nodes and edges for a question, `graphify explain \"<symbol>\"` explains one node, and `graphify path \"A\" \"B\"` finds how two things connect. Read `graphify-out/GRAPH_REPORT.md` for the summary.\n<!-- graphify:end -->\n".to_string();
                let path = worktree.join("AGENTS.md");
                let existing = fs::read_to_string(&path).unwrap_or_default();
                fs::write(&path, format!("{existing}\n{block}"))?;
            }
        }
        Arm::None | Arm::LlmWiki => {}
    }
    fs::write(home.join("cordis.patch.yml"), patch)?;
    // Credentials the provider needs, copied and locked down.
    let credentials = context.settings.with_file_name(".credentials.yaml");
    if credentials.is_file() {
        let target = home.join(".credentials.yaml");
        fs::copy(&credentials, &target)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&target, fs::Permissions::from_mode(0o600))?;
        }
    }
    let _ = label;
    Ok(())
}

/// Copy the source files of one tree into a staging directory.
fn stage_sources(from: &Path, to: &Path) -> Result<()> {
    const SOURCE_EXTENSIONS: &[&str] = &[
        "ts", "tsx", "js", "jsx", "mjs", "cjs", "py", "go", "rs", "java", "c", "h", "cpp", "rb",
        "cs", "kt", "swift", "php",
    ];
    for path in walkdir(from) {
        let relative = path.strip_prefix(from).unwrap_or(&path);
        if relative.components().any(|part| {
            let name = part.as_os_str().to_string_lossy().to_string();
            name == "node_modules" || name == "target" || name == ".git" || name == "graphify-out"
        }) {
            continue;
        }
        let Some(extension) = path.extension().and_then(|extension| extension.to_str()) else {
            continue;
        };
        if !SOURCE_EXTENSIONS.contains(&extension) {
            continue;
        }
        let target = to.join(relative);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::copy(&path, &target)?;
    }
    Ok(())
}

/// Copy one directory tree.
fn copy_tree(from: &Path, to: &Path) -> Result<()> {
    for path in walkdir(from) {
        let relative = path.strip_prefix(from).unwrap_or(&path);
        let target = to.join(relative);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::copy(&path, &target)?;
    }
    Ok(())
}

/// Slice one top-level YAML block verbatim.
fn slice_block(document: &str, key: &str) -> Option<String> {
    let lines: Vec<&str> = document.lines().collect();
    let start = lines
        .iter()
        .position(|line| line.starts_with(&format!("{key}:")))?;
    let mut end = lines.len();
    for (offset, line) in lines.iter().skip(start + 1).enumerate() {
        let is_top_level = line.split(':').next().is_some_and(|head| {
            !head.is_empty() && !head.starts_with(' ') && !head.starts_with('-')
        });
        if is_top_level && line.contains(':') {
            end = start + 1 + offset;
            break;
        }
    }
    Some(format!("{}\n", lines[start..end].join("\n")))
}

/// Load the case matrix.
fn load_cases(path: &Path) -> Result<Vec<BenchCase>> {
    let text = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    let document: Value = serde_json::from_str(&text)?;
    let cases: Vec<BenchCase> = serde_json::from_value(document["cases"].clone())?;
    Ok(cases)
}

/// Run one git command and return its stdout.
fn git(repo: &str, args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()?;
    if !output.status.success() {
        bail!("git {args:?}: {}", String::from_utf8_lossy(&output.stderr));
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

/// Locate one executable on PATH.
fn which(name: &str) -> Option<PathBuf> {
    let path = std::env::var("PATH").ok()?;
    for directory in path.split(':') {
        let candidate = Path::new(directory).join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

/// Trim one string for a record.
fn truncate(value: &str, limit: usize) -> String {
    if value.len() <= limit {
        return value.to_owned();
    }
    let mut end = limit;
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &value[..end])
}

/// The printed summary.
fn summary(record: &RunRecord) -> String {
    let mut lines = Vec::new();
    lines.push("follow-up medians per arm".to_owned());
    let table = medians(record);
    lines.push(format!(
        "{:<10} {:>10} {:>10} {:>8} {:>8} {:>7}",
        "arm", "wall ms", "tokens", "calls", "pass", "n"
    ));
    for (arm, stats) in &table {
        lines.push(format!(
            "{:<10} {:>10} {:>10} {:>8} {:>8} {:>7}",
            arm, stats.wall_ms, stats.tokens, stats.tool_calls, stats.passed, stats.sessions
        ));
    }
    lines.join("\n")
}

/// One arm's follow-up medians.
struct ArmStats {
    wall_ms: u64,
    tokens: u64,
    tool_calls: u64,
    passed: usize,
    sessions: usize,
}

/// Median follow-up figures per arm, failed attempts included.
fn medians(record: &RunRecord) -> BTreeMap<String, ArmStats> {
    let mut by_arm: BTreeMap<String, Vec<&SessionRecord>> = BTreeMap::new();
    for session in &record.sessions {
        if session.stage == "follow-up" {
            by_arm.entry(session.arm.clone()).or_default().push(session);
        }
    }
    let mut out = BTreeMap::new();
    for (arm, sessions) in by_arm {
        let mut walls: Vec<u64> = sessions
            .iter()
            .map(|session| session.metrics.wall_ms)
            .collect();
        let mut tokens: Vec<u64> = sessions
            .iter()
            .map(|session| session.metrics.total())
            .collect();
        let mut calls: Vec<u64> = sessions
            .iter()
            .map(|session| session.metrics.tool_calls)
            .collect();
        out.insert(
            arm,
            ArmStats {
                wall_ms: median(&mut walls),
                tokens: median(&mut tokens),
                tool_calls: median(&mut calls),
                passed: sessions.iter().filter(|session| session.passed).count(),
                sessions: sessions.len(),
            },
        );
    }
    out
}

/// Median of one list.
fn median(values: &mut [u64]) -> u64 {
    if values.is_empty() {
        return 0;
    }
    values.sort_unstable();
    values[values.len() / 2]
}

/// The markdown report.
fn render_report(record: &RunRecord) -> String {
    let mut out = String::new();
    out.push_str("# Wayfinder benchmark\n\n");
    out.push_str(&format!(
        "- Model route: `{}` / `{}`\n- Preset: `{}`\n- Checkout: `{}`\n- Arms: {}\n- Cases: {}\n- Repeats: {}\n\n",
        record.provider,
        record.model,
        record.preset,
        record.commit,
        record.arms.join(", "),
        record.cases.join(", "),
        record.repeats
    ));
    out.push_str("## Follow-up medians\n\n");
    out.push_str("| Arm | Wall ms | Total tokens | Tool calls | Passed / sessions |\n|---|---|---|---|---|\n");
    for (arm, stats) in medians(record) {
        out.push_str(&format!(
            "| {arm} | {} | {} | {} | {} / {} |\n",
            stats.wall_ms, stats.tokens, stats.tool_calls, stats.passed, stats.sessions
        ));
    }
    out.push_str("\n## Per case\n\n");
    let titles: BTreeMap<String, String> = record
        .sessions
        .iter()
        .map(|session| (session.case.clone(), session.case_title.clone()))
        .collect();
    for (case, title) in &titles {
        out.push_str(&format!("- `{case}`: {title}\n"));
    }
    out.push('\n');
    out.push_str("| Case | Arm | Follow-up | Tokens | Wall ms | Memory calls | Reused without read | Verdict |\n");
    out.push_str("|---|---|---|---|---|---|---|---|\n");
    for session in record
        .sessions
        .iter()
        .filter(|session| session.stage == "follow-up")
    {
        out.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} | {} | {} |\n",
            session.case,
            session.arm,
            session.follow_up,
            session.metrics.total(),
            session.metrics.wall_ms,
            session.metrics.memory_calls,
            if session.metrics.reused_without_read {
                "yes"
            } else {
                "no"
            },
            if session.passed {
                "pass"
            } else {
                session.failure.as_deref().unwrap_or("fail")
            }
        ));
    }
    out.push_str("\n## Discovery\n\n");
    out.push_str("| Case | Arm | Tokens | Wall ms | Tool calls | Memory calls |\n|---|---|---|---|---|---|\n");
    for session in record
        .sessions
        .iter()
        .filter(|session| session.stage == "discovery")
    {
        out.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} |\n",
            session.case,
            session.arm,
            session.metrics.total(),
            session.metrics.wall_ms,
            session.metrics.tool_calls,
            session.metrics.memory_calls
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One follow-up record with the given figures.
    fn session(arm: &str, wall: u64, tokens: u64, passed: bool) -> SessionRecord {
        SessionRecord {
            arm: arm.to_owned(),
            case: "case".to_owned(),
            case_title: "Case".to_owned(),
            follow_up: "one".to_owned(),
            repeat: 1,
            stage: "follow-up".to_owned(),
            exit_code: 0,
            answer: String::new(),
            passed,
            failure: None,
            metrics: Metrics {
                wall_ms: wall,
                total_tokens: tokens,
                tool_calls: 3,
                ..Metrics::default()
            },
        }
    }

    fn record(sessions: Vec<SessionRecord>) -> RunRecord {
        RunRecord {
            arms: vec!["none".to_owned(), "wayfinder".to_owned()],
            cases: vec!["case".to_owned()],
            repeats: 1,
            provider: "kornerstone".to_owned(),
            model: "qwen38-27b-fp8".to_owned(),
            preset: "standard".to_owned(),
            commit: "abc".to_owned(),
            sessions,
        }
    }

    #[test]
    fn medians_ignore_discovery_and_count_failures_in_the_total() {
        let record = record(vec![
            SessionRecord {
                stage: "discovery".to_owned(),
                ..session("wayfinder", 9_999, 999_999, true)
            },
            session("none", 100, 1_000, true),
            session("none", 300, 3_000, false),
            session("wayfinder", 200, 2_000, true),
        ]);
        let stats = medians(&record);
        assert_eq!(
            stats["none"].wall_ms, 300,
            "median of 100 and 300 keeps the upper middle"
        );
        assert_eq!(stats["none"].tokens, 3_000);
        assert_eq!(
            stats["none"].passed, 1,
            "a failed attempt still counts as a session"
        );
        assert_eq!(stats["none"].sessions, 2);
        assert_eq!(
            stats["wayfinder"].tokens, 2_000,
            "discovery is not a follow-up"
        );
    }

    #[test]
    fn ground_truth_requires_every_term_and_rejects_forbidden_ones() {
        let expect = Expectation {
            all: vec!["test:coverage".to_owned()],
            any: vec!["22.19".to_owned(), ">=24".to_owned()],
            none: vec!["pnpm install".to_owned()],
        };
        assert!(verify("Run TEST:COVERAGE on Node >=24", &expect).is_ok());
        assert!(verify("run the coverage gate", &expect).is_err());
        assert!(verify("test:coverage needs 22.19", &expect).is_ok());
        assert!(verify("test:coverage, but first pnpm install", &expect).is_err());
    }

    #[test]
    fn a_yaml_block_is_sliced_verbatim() {
        let document = "agent-default-model:\n  provider: a\nllm-pi-ai:\n  providers:\n    x:\n      api: openai\nagent-presets:\n  default: standard\n";
        let block = slice_block(document, "llm-pi-ai").expect("block");
        assert_eq!(
            block,
            "llm-pi-ai:\n  providers:\n    x:\n      api: openai\n"
        );
    }
}
