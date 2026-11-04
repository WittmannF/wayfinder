# Wayfinder — implementation plan

Wayfinder gives any coding agent a workspace memory it writes to and reuses across
sessions: markdown pages with declared anchors, revision-safe concurrent writes,
search that expands over declared relations, a drift check, a prompt digest, two
seeding passes, and a knowledge graph — all reachable from the terminal and from
MCP.

The measured reason this exists: in a published LLM Wiki benchmark, giving an agent a
wiki it can reuse cut follow-up sessions from **126.2 s to 59.1 s (−53 %)** and
**92 087 to 63 312 tokens (−31 %)**, and took answers that never reused the pool to
six of eight. Wayfinder is that feature set without the harness.

## Shape

```
wayfinder init claude            # detect + install: store, MCP entry, instructions, structure seed
wayfinder search "rate limit"    # FTS over pages, expanded over declared relations
wayfinder read <id> --section Throughput
wayfinder save --title … --body - --anchor src/limiter.rs:42
wayfinder check [<id>]           # fresh / changed / missing per anchor
wayfinder list --limit 20
wayfinder digest --tokens 500    # the block a harness injects into the prompt
wayfinder graph --format svg|json|dot|mermaid|html
wayfinder seed --structure       # free pass: one page per package, from manifests
wayfinder seed --pages --model qwen3.8-27b
wayfinder install codex --global
wayfinder mcp serve              # stdio MCP server: the same operations as tools
wayfinder status | doctor
```

## Decisions

**Markdown is the source of truth.** Pages live in `.wayfinder/memory/<id>.md`, so a
reviewer diffs them, git carries them, and any editor opens them. The SQLite index
(`.wayfinder/index.db`, FTS5) is derived and rebuildable; deleting it costs a scan.

**Revisions are content hashes, not counters.** `rev = hex16(sha256(canonical page
bytes))`, computed on demand and never stored, so any out-of-band edit changes the
rev and a stale writer's `--expected-rev` is refused rather than silently winning.

**Concurrency is a lock plus compare-and-set.** Page writes are temp-file + rename
(atomic), the index is updated under an exclusive file lock, and the logical fence
is the rev. N agents may write different pages at once; two agents writing one page
serialize on the rev.

**Anchors are hashes of files, checked on demand.** `--anchor path[:line]` records
`sha256(file)`; `wayfinder check` reports `fresh` / `changed` / `missing`. A page
whose anchor changed is a page to re-read before trusting.

**MCP is the integration, the CLI is the product.** `wayfinder mcp serve` speaks MCP
over stdio, so any MCP-capable harness (Claude Code, Codex, Copilot, Cursor, Gemini
CLI) gets typed tools; `wayfinder install <harness>` writes that harness's MCP entry
and instruction snippet. The same operations remain terminal commands.

**One store per project.** The root is `--root`, `WAYFINDER_ROOT`, or the nearest
ancestor holding `.wayfinder/` or `.git/`, so a repository's worktrees and branches
share one pool instead of forking it.

## Phases

1. **Core store** — done: page model and frontmatter, rev, anchors, atomic writes,
   file lock, index build, search with relation expansion, section reads, drift
   check, digest, and a concurrency test with N writers.
2. **Graph** — done: nodes (page/subject/path), edges (about/cites/supersedes/
   depends), deterministic radial layout, JSON/DOT/SVG/HTML/Mermaid emitters, bounds
   with cluster aggregation.
3. **Harness integration** — done: `mcp` (initialize, tools/list, tools/call, seven
   tools, `--prefix` for a harness that owns `memory_*`) and `install` for
   claude/codex/copilot/cursor/gemini/agents, merging into existing configuration.
4. **Seeding** — done: free structure pass over manifests and READMEs (Cargo, npm,
   pyproject, Go; bounded walk; anchors on the files it read; one `depends` edge per
   dependency), and the priced page pass through an OpenAI-compatible endpoint,
   plan-first with an explicit estimate and a `--yes` gate.
5. **Benchmark** — 4 arms (no tool, in-process LLM Wiki, graphify, wayfinder) ×
   the 6 harness cases × 3 repeats against `kornerstone-dev/qwen38-27b-fp8`, in a
   fresh checkout, with the same ground-truth guard and the charged-per-answer view.
   Next.
6. **Docs** — README quickstart, command reference, architecture, and the measured
   results.

## Feature parity with the harness release

| Harness feature | Wayfinder |
|---|---|
| pages with kind/subject/anchors/supersedes/depends | same frontmatter, own subset parser |
| section-scoped reads | `read --section` |
| FTS search + RRF relation expansion | `search` |
| anchor drift check (`memory_check`) | `check` |
| CAS revisions (`rev`/`expectedRev`) | `--expected-rev`, content-hash revs |
| project-root pool scope | root resolution over `.wayfinder`/`.git` |
| prompt digest gated by a setting | `digest`, injected by the harness instructions |
| structure seeding (free) | `seed --structure` |
| model seeding with a plan and a job | `seed --pages --plan` / `--pages` |
| knowledge graph in Settings | `graph --format svg|html|json|dot|mermaid` |
| concurrent agent writes | file lock + CAS + atomic rename |
| memory tools for the model | `mcp serve` tool surface |
