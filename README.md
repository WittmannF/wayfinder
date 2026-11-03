# wayfinder

Workspace memory that coding agents write to and reuse, from the terminal.

An agent that solved something once should not solve it again. Wayfinder keeps a pool
of markdown pages under `.wayfinder/memory/` — facts, decisions, gotchas, and
pointers, each citing the files it rests on — and gives any harness five commands to
write, find, read, verify, and draw them.

```console
$ wayfinder init
$ wayfinder save --title "Rate limits" --body - --anchor src/limiter.rs:42
$ wayfinder search "how many requests per second"
$ wayfinder read rate-limits --section Throughput
$ wayfinder check
```

## Why it pays

In the Factored Harness benchmark, an agent with a reusable wiki answered the same
follow-up questions in **59.1 s instead of 126.2 s (−53 %)** and **63 312 tokens
instead of 92 087 (−31 %)**, and answered six of eight follow-ups without re-reading
the workspace at all. Wayfinder is that feature set with the harness left out.

## What it does

- **Pages with declared relations.** `kind`, `tags`, `subject`, `depends`,
  `supersedes`, and `anchors` are frontmatter an agent fills in; the body is prose a
  later session reads.
- **Section reads.** `read --section` returns the matching heading and its body, so a
  follow-up costs one section instead of a whole page.
- **Search over a real index.** SQLite FTS5, then one hop over declared relations, so
  a page that never names the query still surfaces.
- **Anchors that expire.** Every cited file is fingerprinted at save; `check` reports
  `fresh`, `changed`, or `missing` so a session knows what to re-read.
- **Revision-safe concurrent writes.** A page's revision is a content hash; a save
  carrying a stale `--expected-rev` is refused instead of clobbering another agent.
- **A prompt digest.** `digest` prints the block that tells a session the pool exists.
- **A graph.** `graph --format svg|html|json|dot|mermaid` draws pages, subjects, cited
  paths, and the relations between them.

## Harnesses

Wayfinder is harness-agnostic: the CLI works anywhere an agent can run a command, and
`wayfinder mcp` (next milestone) speaks MCP over stdio for Claude Code, Codex, Copilot,
Cursor, and Gemini CLI. `wayfinder install <harness>` writes that harness's MCP entry
and instruction snippet.

## Status

Milestone 1 is the store and the terminal surface: `init`, `save`, `read`, `search`,
`list`, `check`, `digest`, `graph`, `reindex`, `status`, with tests for each, the
revision fence, anchors, the graph, and concurrent writers. `mcp`, `install`, `seed`,
and the benchmark harness are the next milestones; see [PLAN.md](PLAN.md).

## Development

```console
$ cargo test          # 25 tests: store, index, search, anchors, graph, CLI
$ cargo build --release
```
