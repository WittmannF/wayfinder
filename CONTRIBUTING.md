# Contributing

Wayfinder is small on purpose: three crates, no runtime dependencies beyond SQLite, and a
page format a human can read. Contributions that keep it that way are welcome.

## Before you open a pull request

```console
$ cargo fmt --all --check
$ cargo clippy --all-targets -- -D warnings
$ cargo test --all
```

CI runs the same three commands on Linux and macOS. A pull request that changes behaviour
should also change or add a test that fails without it — the counter-examples worth pinning
are in `crates/wayfinder-core/tests/` (page round-trip, revision fence, anchor drift, search
expansion, concurrent writers, the viewer's escaping and controls).

## What belongs where

| Crate | Owns |
|---|---|
| `crates/wayfinder-core` | pages, frontmatter, revisions, anchors, the SQLite index, search, the graph, the viewer, seeding |
| `crates/wayfinder` | the CLI, the MCP server, harness installation |
| `crates/wayfinder-bench` | the four-arm comparison runner |

Rules the code keeps, and pull requests should keep:

- **The page is the source of truth.** Anything derivable — index rows, edges, graph nodes —
  must rebuild from the files, and deleting `.wayfinder/index.db` must never lose data.
- **Revisions are content hashes.** Never store a revision in the page; compute it. A save
  that cannot prove it saw the current bytes must refuse.
- **Every counterexample gets a test.** Especially anything about concurrency, escaping, or
  paths: those are where this class of tool breaks.
- **No new dependency without a reason in the pull request.** The tool is one binary; a
  dependency is a promise to keep it building.

## Commit and review

Conventional-commit subjects (`feat:`, `fix:`, `docs:`, `chore:`) with a body that says what
changed and why. Say which command or test proves the change. For anything that alters the
page format, the CLI surface, or the MCP tools, update `README.md` and `PLAN.md` in the same
commit — the documentation is part of the change, not a follow-up.

## Benchmark claims

If you change the memory surface, you can measure it instead of asserting it:

```console
$ cargo build --release
$ ./target/release/wayfinder-bench --plan --repo <checkout>
$ ./target/release/wayfinder-bench --repo <checkout> --repeats 3 --out results/run-1
```

Numbers in a pull request should say which commit of the benchmarked checkout they came
from, how many repeats, and what the arm configuration was. [`benchmark/README.md`](benchmark/README.md)
lists what the comparison cannot claim; a claim outside those limits will be asked to justify
itself.
