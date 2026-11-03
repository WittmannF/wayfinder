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

The CLI works anywhere an agent can run a command. For a harness that speaks MCP,
`wayfinder mcp` serves the same operations over stdio — `memory_search`,
`memory_read`, `memory_save`, `memory_list`, `memory_check`, `memory_digest`, and
`memory_graph` — and `wayfinder mcp --prefix wayfinder` renames them for a harness
that already owns `memory_*`.

`wayfinder install <harness>` writes that harness's MCP entry and its instruction
snippet, merging into what is already there:

| Harness | MCP entry | Instructions |
|---|---|---|
| `claude` | `.mcp.json` (or `~/.claude.json`) | `CLAUDE.md` |
| `codex` | `~/.codex/config.toml` | `AGENTS.md` |
| `copilot` | `.vscode/mcp.json` | `.github/copilot-instructions.md` |
| `cursor` | `.cursor/mcp.json` | `.cursor/rules/wayfinder.mdc` |
| `gemini` | `.gemini/settings.json` | `GEMINI.md` |
| `agents` | — | `AGENTS.md`, with the CLI commands |

## Status

Milestone 1 was the store and the terminal surface. Milestone 2 adds the MCP server and
`install`, so a harness gets typed tools and an instruction block from one command.

Next: `seed` (the free structure pass and the priced model pass), then the benchmark
against no tool, the harness LLM Wiki, and graphify. See [PLAN.md](PLAN.md).

## Development

```console
$ cargo test          # 40 tests: store, index, search, anchors, graph, CLI, MCP, install
$ cargo build --release
```
