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
page format, the CLI surface, or the MCP tools, update `README.md` and `docs/architecture.md`
in the same commit — the documentation is part of the change, not a follow-up.

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

## Releasing

Cut releases from `main` with `scripts/release.sh`, never by tagging a commit by hand.
Preview the run first with `--dry-run`, which prints the bump, the checks, and the push
commands without changing anything:

```console
$ scripts/release.sh X.Y.Z --dry-run
$ scripts/release.sh X.Y.Z
$ git push origin main
$ git push origin vX.Y.Z
```

The helper bumps `Cargo.toml`, `Cargo.lock`, and the Python `__version__` together, commits
`chore(release): X.Y.Z`, and creates the annotated tag `vX.Y.Z`. It refuses an existing tag
and a tree that is not clean `main`. Tags are numeric (`vX.Y.Z`); the helper and the gate do
not accept pre-release suffixes.

The release workflow refuses to publish unless the tag, `Cargo.toml`, `Cargo.lock`, the
Python `__version__`, and the built binary's `--version` all agree (the cross-compiled Intel
Mac asset is not executed; the source checks cover it). A tag without a bump ships a binary
whose `version` names the previous release (v0.1.5 and v0.1.6 both print `wayfinder 0.1.4`).

Always push the tag: a release created from the GitHub UI or API does not emit the `push`
event the workflow needs. Once the PyPI job uploads, that version is immutable, so a bad
publish is recovered by deleting the tag and release and cutting the next patch, never by
reusing the version.

## Licensing of contributions

Wayfinder is licensed under the [Apache License 2.0](LICENSE). By opening a pull request you
agree that your contribution is licensed under the same terms, and you confirm you have the
right to submit it. No separate contributor agreement is required. If your change adds code
from another project, say so in the pull request and make sure its license is compatible and
its notice is carried in [NOTICE](NOTICE).
