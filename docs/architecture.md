# Architecture

Wayfinder gives coding agents a persistent, git-native workspace memory: Markdown pages with
declared relations and SHA-fingerprinted file anchors, a derived SQLite index and knowledge
graph, revision-safe concurrent writes, and an MCP server and CLI shipped in a single binary.

## Workspace layout

| Crate | Purpose |
|---|---|
| `crates/wayfinder-core` | Storage engine, Markdown/frontmatter parser, content-hash revisions, file anchor verification, SQLite FTS5 + edge index, page-level relation expansion, graph layout and emitters (HTML, SVG, JSON, DOT, Mermaid), and repository seeding. |
| `crates/wayfinder` | Terminal CLI (`wayfinder`), stdio MCP server (`wayfinder mcp`), and idempotent harness installer (`wayfinder install <harness>`). |
| `crates/wayfinder-bench` | Standalone four-arm benchmark runner (`publish = false`) used to evaluate follow-up wall time and token usage across memory surfaces. |

## Storage model

```
.wayfinder/
├── memory/<id>.md     durable artifact: YAML frontmatter + Markdown body
├── index.db           derived SQLite database (FTS5 + graph edges)
└── write.lock         cross-process exclusive write lock
```

### Markdown is the source of truth

Pages live in `.wayfinder/memory/<id>.md` so humans can review them in pull requests, `git`
tracks their history across branches, and any editor can open them.

The SQLite database (`.wayfinder/index.db`) is strictly derived from the Markdown files on disk.
Deleting `.wayfinder/index.db` never loses data—Wayfinder rebuilds the index automatically when
it is missing or when `wayfinder reindex` is run.

### Pool root resolution

A single memory pool serves the entire repository, including linked git worktrees. Wayfinder
resolves the pool root in order:
1. `--root <path>` flag
2. `WAYFINDER_ROOT` environment variable
3. Nearest ancestor directory holding `.wayfinder/` or `.git/`

## Page model & frontmatter

Each page combines structured metadata in YAML frontmatter with free-form Markdown headings:

- **`id`** — derived from the filename (`.wayfinder/memory/<id>.md`).
- **`title`** and **`kind`** — human title and classification (e.g., `fact`, `architecture`, `convention`).
- **`subject`** — entities this page describes (`package:<name>`, `path:<relpath>`, or topic tags).
- **`depends`** — packages or pages this finding depends on.
- **`supersedes`** — older page IDs replaced by this page.
- **`anchors`** — cited repository paths (`path[:line]`) paired with the file's SHA-256 hash prefix at the time the page was saved.

## Revisions & concurrency

Multiple agents operating in the same repository (parallel subagents, background workers, or a
reviewer and an implementer) can read and write concurrently without corrupting pages or the
index:

- **Content-hash revisions (`rev`):** Every page's revision is `hex16(sha256(canonical_page_bytes))`,
  computed on demand and never stored inside the file. Any edit—whether via MCP, the CLI, or a
  text editor—changes the `rev`.
- **Compare-and-set fence (`--expected-rev`):** When updating an existing page, a writer supplies
  the `rev` it read. If the page changed in the meantime, the save is refused with the current
  `rev` so the agent re-reads and merges instead of silently overwriting concurrent work.
- **Atomic writes + locked index:** Saves write to a temporary file and atomically `rename` it
  into `.wayfinder/memory/`, then update `.wayfinder/index.db` under an exclusive lock on
  `.wayfinder/write.lock`. Readers never observe half-written files, and agents writing
  different pages proceed in parallel.

## Search & section reads

- **FTS5 + related-page expansion:** `wayfinder search` tokenizes the query and requires every
  usable term to match in a page title or body. SQLite FTS5's Porter/Unicode tokenizer and
  BM25 rank direct matches. Search then finds pages sharing a declared subject, cited path, or
  dependency node, plus pages linked directly by `supersedes`. Shared nodes are discounted by
  how many pages use them, so a narrow relation ranks above a broad one. Direct matches stay
  ahead of expanded pages via Reciprocal Rank Fusion (RRF), and kind/subject filters also apply
  to expanded pages. This is lexical retrieval with bounded graph assistance, not semantic or
  embedding search.
- **Section-scoped reads (`--section`):** Instead of pulling an entire document into the context
  window, agents can read a single Markdown heading (`wayfinder read <id> --section Throughput`).

## Anchor drift verification

Code changes invalidate prose. When a page cites a file via `--anchor path[:line]`, Wayfinder
records a fingerprint of that file's contents. Running `wayfinder check` (or `memory_check` over
MCP) compares recorded fingerprints against the working tree and reports each anchor as:
- `fresh` — the cited file matches the fingerprint recorded when the page was saved.
- `changed` — the file was modified since the page was written; the agent should re-verify the
  code before trusting the page.
- `missing` — the cited file was moved or deleted.

## Knowledge graph & viewer

Wayfinder constructs a unified property graph directly from the page pool:
- **Nodes:** pages, subjects (`package:*`), and cited file paths (`path:*`).
- **Edges:** `about` (page → subject), `cites` (page → anchor path), `depends` (page/package → dependency), and `supersedes` (page → older page).

`wayfinder graph` lays out the graph deterministically and emits it as:
- **Interactive HTML (`--format html`):** A self-contained offline viewer with node inspection,
  kind/subject/text filters, cluster counts, and an anchor drift banner. Passing `--watch`
  regenerates the view live as agents write to the pool.
- **Static & machine formats:** `svg`, `json`, `dot` (Graphviz), and `mermaid`.

## Seeding

To avoid starting with an empty pool in an existing codebase, `wayfinder seed` provides two
passes:
1. **Structure pass (`wayfinder seed --structure`):** Free and deterministic (no model calls).
   Walks repository manifests (`Cargo.toml`, `package.json`, `pyproject.toml`, `go.mod`) and
   READMEs, creating one page per package and per major code directory with entry files cited as
   anchors and package dependencies wired as `depends` edges.
2. **Model pass (`wayfinder seed --pages`):** Upgrades structural pointer pages with
   model-generated summaries via an OpenAI-compatible endpoint. Always previews a token/cost
   plan first (`--plan`) and requires `--yes` before calling a model.

## CLI & MCP integration

The CLI is the primary interface; the MCP server (`wayfinder mcp`) exposes the exact same core
operations over `stdio`:
- `memory_search`, `memory_read`, `memory_save`, `memory_list`, `memory_check`, `memory_digest`,
  and `memory_graph`.
- `wayfinder install <harness>` wires the MCP server entry and injects a marker-delimited
  instruction block into the harness's configuration (`claude`, `codex`, `copilot`, `cursor`,
  `gemini`, or `agents`), merging cleanly with existing settings.

