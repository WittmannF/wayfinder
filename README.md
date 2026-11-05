<p align="center">
  <h1 align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="images/logo-dark.svg">
    <img src="images/logo-light.svg" alt="wayfinder" width="360">
  </picture>
</h1>
  <sub>A hybrid of a self-generating knowledge graph and the LLM Wiki — written by your agents over MCP, read back by the next session.</sub>
</p>

<p align="center">
  <a href="#quickstart">Quickstart</a> &nbsp;·&nbsp;
  <a href="#install">Install</a> &nbsp;·&nbsp;
  <a href="#measured">Benchmark</a> &nbsp;·&nbsp;
  <a href="#how-it-works">How it works</a> &nbsp;·&nbsp;
  <a href="#concurrent-agents">Concurrent agents</a> &nbsp;·&nbsp;
  <a href="CONTRIBUTING.md">Contributing</a>
</p>

<p align="center">
  <a href="https://github.com/zaterka/wayfinder/actions/workflows/ci.yml"><img alt="CI" src="https://github.com/zaterka/wayfinder/actions/workflows/ci.yml/badge.svg"></a>
  <a href="https://pypi.org/project/wayfinder-memory/"><img alt="PyPI" src="https://img.shields.io/pypi/v/wayfinder-memory.svg?color=blue"></a>
  <a href="LICENSE"><img alt="License: Apache 2.0" src="https://img.shields.io/badge/license-Apache%202.0-blue.svg"></a>
  <a href="Cargo.toml"><img alt="Rust 1.89+" src="https://img.shields.io/badge/rust-1.89%2B-orange.svg"></a>
</p>

![The Wayfinder viewer: a 47-page pool drawn as a knowledge graph, with one page selected](docs/viewer.png)

Agents burn tokens re-deriving the architecture and conventions they already explored. Wayfinder
fixes that at the source: every finding a session saves becomes a structured Markdown page in
your repository — fingerprinted against the files it cites, linked into a knowledge graph, and
served to your agent over MCP.

In follow-up sessions across our [agent benchmark](#measured), Wayfinder cut **token usage by 30 %**
and **wall time by 13 %** compared to an agent with no memory tool — while a naive in-process
wiki and a shell-queried code graph both *increased* time and token cost.

- **Memory in git, not in a service.** Pages are plain Markdown under `.wayfinder/memory/`,
  committed with the code. You review what an agent learned in a pull request, `git diff` shows
  it, and the pool outlives the session, the model, and the harness that wrote it.
- **No daemon, no graph database, no vector store.** One binary; the index is a SQLite file the
  store rebuilds from the pages whenever it is deleted.
- **One command to wire a harness.**
  `wayfinder install claude` — or `cursor`, `codex`, `copilot`, `gemini` — writes the MCP entry
  and the instruction block that tells the agent when to search, read, and save.

### Why a hybrid: LLM Wiki × Knowledge Graph

Pure wikis drift silently when code changes and bloat prompts when injected wholesale; pure AST
code graphs capture file structure, not the findings an agent had to work to discover. Wayfinder
combines both:

- **The LLM Wiki** — an agent writes what it learned into a wiki it reads next session
  ([Karpathy's gist](https://gist.github.com/karpathy/442a6bf555914893e9891c11519de94f)), with
  targeted section reads (`--section`), search over declared relations, and SHA-fingerprinted
  anchors that warn when a cited file changed.
- **A self-generating knowledge graph** — pages, subjects, packages, and cited paths are nodes;
  `about` / `cites` / `depends` / `supersedes` are edges. Nothing to model by hand:
  `seed --structure` maps an existing repository in seconds, and every page an agent saves
  afterwards makes the graph richer.

`wayfinder graph --format html --out graph.html --watch` draws it — and keeps drawing it while
agents work.

## Quickstart

Map an existing repository, wire your agent harness, and open the interactive knowledge graph in
four commands:

```bash
wayfinder init                    # create .wayfinder/ in this repository
wayfinder seed --structure        # pages for each package and each code directory, from files
wayfinder install claude          # MCP server + instructions for your harness
wayfinder graph --format html --out graph.html --open
```

Then start a new session. Your agent can call `memory_search`, `memory_read`, `memory_save`,
`memory_list`, `memory_check`, `memory_digest`, and `memory_graph`, and it has been told when
to use them.

### In your agent session

No prompt engineering, no special syntax — the installed instructions do the routing:

```console
> how are rate limits enforced here?
  · memory_search("rate limits")                    → 1 hit: rate-limits (fact, fresh)
  · memory_read("rate-limits", section="Throughput")
  The limiter allows 100 requests per second per key, verified against src/limiter.rs.

> (after debugging) save that the limiter reads its window from Redis, not memory
  · memory_save(title="Rate limit window", kind="fact", subject=…, anchor="src/limiter.rs")
  Saved rate-limit-window. Next session starts here instead of re-reading the tree.
```

The same operations from a terminal, when you want them without a session:

```console
$ wayfinder search "how many requests per second"
1. Rate limits (id: rate-limits, kind: fact, updated 2026-11-03, rev: 890e3bf665eb6756)
   ## Throughput
   The limiter allows 100 requests per second per key.

$ wayfinder read rate-limits --section Throughput
Read memory page "Rate limits" (id: rate-limits, tags: memory, kind: fact, rev: …)

## Throughput
The limiter allows 100 requests per second per key.

$ wayfinder check
Memory page "rate-limits" cites files that changed. Re-read these before trusting the page:
  changed src/limiter.rs (now 3a1a16bbdd29312f)
```

## Install

**One binary. No runtime, no daemon, no database.** Prebuilt for macOS 11+ and Linux (`arm64` and `x86_64`).

```bash
# With uv (recommended) or pip
uv tool install wayfinder-memory
pip install wayfinder-memory

# Or via the standalone shell installer (verifies SHA-256, installs to ~/.local/bin)
curl -fsSL https://raw.githubusercontent.com/zaterka/wayfinder/main/scripts/install.sh | bash
```

Keep an existing installation current with:

```bash
wayfinder --upgrade     # newest release for this platform, checksum verified
```

> Building from source (`cargo`), pinning versions, Linux `glibc` requirements, Windows/Alpine
> support, and troubleshooting are covered in
> [**Installation & Troubleshooting**](docs/troubleshooting.md).

## Measured

**Why it saves:** the agent answers from a page that already states the finding, and only trusts
it while the files it cites still match — instead of re-exploring the tree and re-reading code it
already understood.

Three cases of an agent benchmark, four surfaces, one agent loop, one model (Qwen 3.8-27B,
self-hosted). Follow-up cost against an agent with **no memory tool**:

| Surface | Wall time | Tokens |
|---|---|---|
| **wayfinder** | **−13 %** | **−30 %** |
| LLM Wiki implemented in-process by the harness | +6 % | +23 % |
| graphify (code graph, queried through the shell) | +13 % | +5 % |

Every follow-up in every arm passed the same ground-truth check. Method, per-case split, and
reproduction steps: [**docs/benchmark.md**](docs/benchmark.md).

## What it gives you

| Capability | How Wayfinder does it |
|---|---|
| **A graph that writes itself** | Every saved page declares `subject`, `depends`, `anchors`, and `supersedes`; those become nodes and edges. `seed --structure` maps an existing repository without a model: a page per package, plus a page per code directory once a package holds more than a handful of source files, with the entry files each page starts from cited as anchors. |
| **A wiki built for reuse** | `read --section` returns one heading instead of a page; search ranks with FTS5, then walks one hop over declared relations, so a page that never names the query can still answer it. |
| **A trust check** | Every cited file is fingerprinted at save. `check` reports `fresh` / `changed` / `missing` per anchor, so a session knows whether to trust a page or re-read the file. |
| **Concurrent agents, no corruption** | Atomic writes, a locked index, and content-hash revisions: two agents on one page serialize instead of clobbering, and the loser is told to re-read. See [Concurrent agents](#concurrent-agents). |
| **A dashboard you can click** | `graph --format html` writes one offline file — node details, kind and subject filters, a text filter, a drift banner, cluster counts — and `--watch` keeps it current. |
| **Harness-agnostic** | A terminal-first CLI, an MCP server over stdio, and one-command installs for Claude Code, Codex, Copilot, Cursor, and Gemini CLI. |
| **A pool on day one** | `seed --pages` upgrades the free pointer pages with model-written ones, behind a stated cost and a `--yes` gate. |

## How it works

```
.wayfinder/
├── memory/<id>.md     one page per finding: frontmatter + markdown
├── index.db           derived SQLite (FTS5 + edges); delete it and it rebuilds
└── write.lock         cross-process write lock
```

The page is the durable artifact; the graph and the index are derived from it.

```markdown
---
title: Rate limits
tags: [memory]
kind: fact
subject: [package:gateway, path:src/limiter.rs]
depends: [serde]
anchors:
  - src/limiter.rs
    sha: 3f2a1c4d5e6b7a89
---

## Throughput

The limiter allows 100 requests per second per key.
```

Commit `.wayfinder/memory/` with the repository: the pages are reviewable in a diff, and the
pool outlives the session, the model, and the harness that wrote it. The root resolves as
`--root`, then `WAYFINDER_ROOT`, then the nearest ancestor holding `.wayfinder/` or `.git/`, so
a worktree and its branches share one pool instead of forking it.

## Concurrent agents

Several agents in one repository is the normal case now: parallel subagents, a swarm, a
reviewer writing while an implementer reads. Wayfinder is built for it rather than around it.

- **Atomic page writes.** Every save goes to a temporary file and is renamed into place, so a
  reader never sees a half-written page.
- **A locked index.** The derived index updates under an exclusive cross-process lock, so two
  writers never interleave their rows.
- **Revisions as fences.** `rev` is a hash of the page's bytes. A save carrying
  `--expected-rev` is refused when the page moved, and the refusal names the revision that is
  there now — the writer re-reads and retries instead of silently overwriting a colleague.
- **Different pages never contend.** The lock covers the index update, not the investigation,
  so agents writing different pages proceed in parallel.

Both properties are tested, not asserted: eight writers saving at once all land and are
indexed, and two writers racing one page leave exactly one winner
(`crates/wayfinder-core/tests/core.rs`). Point `wayfinder graph --watch` at the pool to watch a
swarm fill it in.

## Harnesses

| Harness | MCP entry | Instructions |
|---|---|---|
| `claude` | `.mcp.json` (or `~/.claude.json`) | `CLAUDE.md` |
| `codex` | `~/.codex/config.toml` | `AGENTS.md` |
| `copilot` | `.vscode/mcp.json` | `.github/copilot-instructions.md` |
| `cursor` | `.cursor/mcp.json` | `.cursor/rules/wayfinder.mdc` |
| `gemini` | `.gemini/settings.json` | `GEMINI.md` |
| `agents` | — | `AGENTS.md`, with the CLI commands |

`install` merges instead of replacing: an existing `.mcp.json` keeps its other servers, an
existing `CLAUDE.md` keeps its prose, and the wayfinder block sits between markers so a second
install updates it in place. Nothing listens on a port — the MCP server is a stdio child
process the harness starts and stops.

## Commands

| Command | What it does |
|---|---|
| `wayfinder init` | Create the pool and build its index. |
| `wayfinder save` | Write or merge a page; `--anchor path[:line]` fingerprints a cited file. |
| `wayfinder search "…"` | FTS5 over titles and bodies, then one hop over declared relations. |
| `wayfinder read <id> [--section …]` | One page, or just the heading that matches. |
| `wayfinder list` · `check` | The pool, newest first · verify every cited file. |
| `wayfinder digest` | The block a harness injects so a session knows the pool exists. |
| `wayfinder graph --format …` | `html` (interactive), `svg`, `json`, `dot`, `mermaid`; `--watch`, `--open`. |
| `wayfinder seed --structure` · `--pages` | Free pages from files — per package and per code directory · model-written pages, plan first. |
| `wayfinder install <harness>` · `mcp` | Wire a harness · serve the pool over MCP. |
| `wayfinder status` · `reindex` | What the pool holds · rebuild the derived index. |

## Documentation

- [Architecture](docs/architecture.md) — crate layout, storage model, revisions, search expansion, and the graph.
- [Benchmark](docs/benchmark.md) — what was measured, how, and what it does not prove.
- [Installation & Troubleshooting](docs/troubleshooting.md) — platform support, Linux `glibc` requirements, installer environment variables, building from source, and common fixes.
- [Contributing](CONTRIBUTING.md) — the checks CI runs and the rules the code keeps.

## License

Apache License 2.0 — see [LICENSE](LICENSE) and [NOTICE](NOTICE). In short: use it, fork it,
ship it in a product, commercial or not; keep the notices and the license text, and if you
sue anyone over a patent in it, your patent grant ends.
