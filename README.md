# wayfinder

**Workspace memory that coding agents write to and reuse — from the terminal, over MCP, in any harness.**

[![CI](https://github.com/zaterka/wayfinder/actions/workflows/ci.yml/badge.svg)](https://github.com/zaterka/wayfinder/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![Rust 1.85+](https://img.shields.io/badge/rust-1.85%2B-orange.svg)](https://www.rust-lang.org)

An agent that solved something once should not solve it again. Wayfinder keeps a pool of
markdown pages under `.wayfinder/memory/` — facts, decisions, gotchas, and pointers, each
citing the files it rests on — and gives any harness the verbs to write, find, read, verify,
and draw them.

It is the **LLM Wiki** idea — an agent writes what it learns into a wiki it reads next
session, [as Andrej Karpathy describes it](https://gist.github.com/karpathy/442a6bf555914893e9891c11519de94f)
— built as a tool rather than a harness feature, so the pool outlives the session, the model,
and the harness that produced it.

![The Wayfinder viewer: a 47-page pool drawn as a knowledge graph, with one page selected](docs/viewer.png)

## Measured

Three cases of a published agent benchmark, four tool surfaces, one agent loop, one model
(Qwen 3.8-27B over a self-hosted route). Follow-up cost against **no tool at all**:

| Surface | Wall time | Tokens |
|---|---|---|
| **wayfinder** | **−13 %** | **−30 %** |
| LLM Wiki, implemented in-process by the harness under test | +6 % | +23 % |
| graphify (code graph, queried through the shell) | +13 % | +5 % |

The `llm-wiki` arm is that same idea implemented in-process by the harness under test; the
measurement is therefore a fair test of the idea, not of one implementation. Every follow-up
in every arm passed the same ground-truth check. The mixed per-case result is
the interesting part: the in-process wiki wins package-ownership questions, wayfinder wins the
command surface and the drift case. Method, full tables, and what the numbers cannot claim:
[`benchmark/RESULTS.md`](benchmark/RESULTS.md).

## Install

```console
$ cargo install --git https://github.com/zaterka/wayfinder wayfinder
$ cargo install --git https://github.com/zaterka/wayfinder wayfinder-bench   # the comparison runner
```

Rust 1.85+. One binary, no runtime, no daemon, no database server — the index is a SQLite
file the store rebuilds from the pages.

## Quickstart

```console
$ wayfinder init                      # create .wayfinder/ in this project
$ wayfinder seed --structure          # one page per package, derived from files only
$ wayfinder install claude            # MCP entry + instructions for your harness
$ wayfinder graph --format html --out graph.html --open
```

Start a new session in the harness afterwards. The model gets `memory_search`,
`memory_read`, `memory_save`, `memory_list`, `memory_check`, `memory_digest`, and
`memory_graph`, plus an instruction block telling it when to use them.

Commit `.wayfinder/memory/` with the repository: the pages are the durable artifact, and a
reviewer can read them in a diff. The index (`.wayfinder/index.db`) is derived — ignore it if
you prefer; deleting it costs one scan.

## Commands

| Command | What it does |
|---|---|
| `wayfinder init` | Create the pool and build its index. |
| `wayfinder save --title … --body …` | Write or merge a page; `--anchor path[:line]` fingerprints a cited file. |
| `wayfinder search "…"` | FTS5 over titles and bodies, then one hop over declared relations. |
| `wayfinder read <id> [--section …]` | One page, or just the heading that matches. |
| `wayfinder list` | The pool, newest first. |
| `wayfinder check [<id>]` | `fresh` / `changed` / `missing` per cited file; exits non-zero on drift. |
| `wayfinder digest` | The block a harness injects so a session knows the pool exists. |
| `wayfinder graph --format …` | `html` (interactive), `svg`, `json`, `dot`, `mermaid`; `--watch`, `--open`. |
| `wayfinder seed --structure` | One pointer page per package from manifests and READMEs — no model, no tokens. |
| `wayfinder seed --pages` | Replace those pointers with model-written pages, behind a stated cost and `--yes`. |
| `wayfinder install <harness>` | MCP entry + instructions: `claude`, `codex`, `copilot`, `cursor`, `gemini`, `agents`. |
| `wayfinder mcp` | Serve the pool over MCP on stdin/stdout (`--prefix wayfinder` renames the tools). |
| `wayfinder status` · `reindex` | What the pool holds · rebuild the derived index. |

`wayfinder --help` prints the same list with flags.

## How it works

```
.wayfinder/
├── memory/<id>.md     one page per finding: YAML frontmatter + markdown body
├── index.db           derived SQLite (FTS5 + edges); delete it and it rebuilds
└── write.lock         cross-process write lock
```

The page is the durable artifact; everything else is derived from it.

```markdown
---
title: Rate limits
tags: [memory]
kind: fact
subject: [package:gateway, path:src/limiter.rs]
depends: [serde]
created: 2026-11-03T12:00:00Z
updated: 2026-11-03T12:00:00Z
anchors:
  - src/limiter.rs
    sha: 3f2a1c4d5e6b7a89
---

## Throughput

The limiter allows 100 requests per second per key.
```

Four properties make reuse pay instead of cost:

- **Section reads.** `read --section Throughput` returns that heading's body, so a follow-up
  spends one section instead of a page.
- **Content revisions.** `rev = hash16(canonical bytes)`, computed and never stored. A save
  carrying a stale `--expected-rev` is refused, so two agents cannot silently clobber each
  other; page writes are temp-file-and-rename and the index updates under an exclusive lock.
- **Anchors that expire.** Every cited file is fingerprinted at save, and `check` reports what
  changed, so a session knows what to re-read before trusting a page.
- **One pool per project.** The root resolves as `--root`, then `WAYFINDER_ROOT`, then the
  nearest ancestor holding `.wayfinder/` or `.git/` — a worktree and its branches share one
  pool instead of forking it.

Search is FTS5, expanded one hop over declared relations and fused by reciprocal rank, so a
page that never names the query can still answer it. The graph draws pages, subject and
package hubs, and cited paths with `about` / `cites` / `depends` / `supersedes` edges, bounded
so a large pool draws named clusters with counts instead of a hairball.

## The viewer

`wayfinder graph --format html` writes one self-contained file — no CDN, no build step, no
network, so it opens from disk on an offline machine. Clicking or pressing Enter on a node
opens its title, kind, freshness, tags, and excerpt, along with the `read` and `check`
commands that act on it; kind buttons, a subject picker, and a text filter narrow the
drawing; a banner counts the pages whose cited file changed. `--watch` rewrites the file as
the pool changes, which is what several agents writing at once looks like from a second
screen.

## Harnesses

The CLI works anywhere an agent can run a command; `wayfinder mcp` speaks MCP over stdio for
harnesses that prefer typed tools. One process per session, no port, no network surface.

| Harness | MCP entry | Instructions |
|---|---|---|
| `claude` | `.mcp.json` (or `~/.claude.json`) | `CLAUDE.md` |
| `codex` | `~/.codex/config.toml` | `AGENTS.md` |
| `copilot` | `.vscode/mcp.json` | `.github/copilot-instructions.md` |
| `cursor` | `.cursor/mcp.json` | `.cursor/rules/wayfinder.mdc` |
| `gemini` | `.gemini/settings.json` | `GEMINI.md` |
| `agents` | — | `AGENTS.md`, with the CLI commands |

`install` merges instead of replacing: an existing `.mcp.json` keeps its other servers, an
existing `CLAUDE.md` keeps its prose, and the wayfinder block lives between markers so a
second install updates it in place. A differing entry is reported and left alone unless
`--force` is passed.

Wayfinder reads the page format an in-process LLM Wiki harness writes — including its
`sources:` provenance list — so an existing pool from such a harness works as-is.

## Benchmark

`wayfinder-bench` runs four tool surfaces through the same agent loop and model, each in its
own git worktree and harness home, and folds cost from the session log the harness itself
wrote.

```console
$ wayfinder-bench --plan --repo <checkout> --arms none,llm-wiki,wayfinder,graphify
$ wayfinder-bench --repo <checkout> --repeats 3 --out results/run-1
$ wayfinder-bench --report --out results/run-1        # re-render without spending a token
```

Design, counters, and the things it cannot claim: [`benchmark/README.md`](benchmark/README.md).

## Development

```console
$ cargo test                 # 65 tests: store, index, search, anchors, graph, viewer, MCP, install, seed
$ cargo clippy --all-targets # clean
$ cargo fmt --all --check
```

Layout: `crates/wayfinder-core` (pool, index, search, anchors, graph, viewer, seeding),
`crates/wayfinder` (CLI, MCP server, harness install), `crates/wayfinder-bench` (the
comparison runner). Design decisions and the roadmap are in [`PLAN.md`](PLAN.md);
the review bar is in [`CONTRIBUTING.md`](CONTRIBUTING.md).

## License

MIT — see [LICENSE](LICENSE).
