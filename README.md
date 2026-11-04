# wayfinder

Workspace memory that coding agents write to and reuse, from the terminal.

An agent that solved something once should not solve it again. Wayfinder keeps a pool
of markdown pages under `.wayfinder/memory/` — facts, decisions, gotchas, and
pointers, each citing the files it rests on — and gives any harness five commands to
write, find, read, verify, and draw them.

```console
$ wayfinder init
$ wayfinder seed --structure            # one page per package, from files only
$ wayfinder search "how many requests per second"
$ wayfinder read rate-limits --section Throughput
$ wayfinder save --title "Rate limits" --body - --anchor src/limiter.rs:42
$ wayfinder check
$ wayfinder graph --format html --out graph.html --open
$ wayfinder graph --format html --out graph.html --watch   # rewrite on every change
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
- **A graph you can click.** `graph --format html` writes one self-contained page: the
  pool drawn as a knowledge graph, where a node opens its title, kind, freshness, tags,
  and excerpt; kind and subject filters and a text filter narrow it; clusters name the
  pages a budget hid; a banner counts the pages whose cited file changed. `--watch`
  rewrites the file while a pool is being written to, so a second screen stays current.
  No CDN, no build step, no network — it opens from disk on an offline machine. Static
  exports stay available: `svg`, `json`, `dot`, `mermaid`.
- **A pool on day one.** `seed --structure` writes one pointer page per package from
  its manifest and README — no model, no tokens. `seed --pages` replaces those pointers
  with pages a model wrote from reading the code, behind a plan that states the spend
  first.

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

Milestone 3 adds `seed`: the free structure pass (Cargo, npm, pyproject, and Go
manifests; 18 skipped directory names; anchors on the manifest and README; one
`depends` edge per declared dependency) and the priced page pass (plan, estimate,
`--yes` gate, per-package failure reporting, provider-reported token spend).

Next: the benchmark against no tool, the harness LLM Wiki, and graphify. See
[PLAN.md](PLAN.md).

## Quickstart

```console
$ cargo install --path crates/wayfinder        # or: cargo build --release
$ wayfinder init                              # create .wayfinder/
$ wayfinder seed --structure                  # one page per package, from files only
$ wayfinder install claude                    # MCP entry + instructions for your harness
```

`install` merges into what is already there and tells you what it left alone. Start a
new session in the harness afterwards; the agent gets `memory_search`,
`memory_read`, `memory_save`, `memory_list`, `memory_check`, `memory_digest`, and
`memory_graph`, plus an instruction block that says when to use them. A harness that
already owns `memory_*` names can run `wayfinder mcp --prefix wayfinder`.

Commit `.wayfinder/memory/` with the repository: the pages are the durable part and a
reviewer can read them in a diff. The index (`.wayfinder/index.db`) is derived — add
it to `.gitignore` if you prefer; deleting it costs one scan.

## Measured

Three cases of the harness benchmark (the ones covering package ownership, the
command surface, and a cited file changing between sessions), four arms, one repeat,
Qwen 3.8-27B over the kornerstone route. Follow-up totals per arm:

| Case | none | llm-wiki | wayfinder | graphify |
|---|---|---|---|---|
| ownership | 182 779 | **93 604** | 115 530 | 203 953 |
| commands | 113 008 | 151 280 | **94 535** | 172 781 |
| stale-page | 83 809 | 260 285 | 89 735 | **81 769** |

Follow-up medians against no tool: **wayfinder −30 % tokens and −13 % wall**,
llm-wiki +23 % tokens and +6 % wall, graphify +5 % tokens and +13 % wall. Every
follow-up in every arm passed the ground-truth check.

The discovery investment is real and per-arm: wayfinder spent 328 k / 346 k / 167 k
tokens across the three cases' discovery sessions, llm-wiki 414 k / 214 k / 208 k,
graphify 151 k / 229 k / 179 k.

Three things to keep in view: this is one repeat of three cases (the harness's own
v2 result, from six cases with repeats, was −53 % wall and −31 % tokens for
llm-wiki); the mixed result is the finding, not a bug — llm-wiki wins package
ownership and loses the command surface and the drift case, wayfinder wins two of
three; and the graphify arm runs its code-only graph, queried through the shell.

Full tables: [benchmark/RESULTS.md](benchmark/RESULTS.md).

## Benchmark

`wayfinder-bench` runs four tool surfaces through one agent loop and one model:
no tool, the Factored Harness's own LLM Wiki, graphify, and wayfinder, over the six
cases the harness benchmark defines. Each arm gets its own git worktree, its own
home, and its own tool surface; the only difference between arms is what the model
can call.

```console
$ cargo build --release
$ ./target/release/wayfinder-bench --plan --repo <checkout> --arms none,llm-wiki,wayfinder,graphify
$ ./target/release/wayfinder-bench --repo <checkout> --repeats 3 --out benchmark/results/run-1
```

## Development

```console
$ cargo test          # 64 tests: store, index, search, anchors, graph, viewer, CLI, MCP, install, seed
$ cargo build --release
```
