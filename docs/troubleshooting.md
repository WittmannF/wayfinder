# Installation & Troubleshooting

Wayfinder ships as a single native binary with no runtime, no daemon, and no external database.
This page covers platform requirements, installer flags, building from source, and common
troubleshooting steps.

## Platform support & Linux `glibc`

Prebuilt binaries are published for **macOS 11+** and **Linux**, on both `arm64` (`aarch64`) and
`x86_64`.

| Platform | Architectures | Requirement |
|---|---|---|
| macOS | `arm64`, `x86_64` | macOS 11+ |
| Linux (v0.1.6+) | `arm64`, `x86_64` | **glibc 2.28+** (Ubuntu 20.04+, Debian 10+, RHEL 8+) |
| Linux (v0.1.5 and earlier) | `arm64`, `x86_64` | **glibc 2.39+** (built on newer runners) |
| Windows, Alpine (`musl`), older Linux | Any supported by Rust | Build from source with `cargo` |

### `version 'GLIBC_2.39' not found` on Linux

If you see a dynamic linker error mentioning `GLIBC_2.39`, you are either running v0.1.5 or
earlier, or your distribution predates `glibc 2.28`. Upgrade to v0.1.6+ (`wayfinder --upgrade` or
re-run the installer), or build from source on that machine. Note that v0.1.5 and v0.1.6
both print `wayfinder 0.1.4`, because their version carriers were not bumped, so
`--version` cannot distinguish those builds; re-run the installer and check
`wayfinder --version` to confirm you are on v0.1.7 or later.

## Installation options

### Python installer (`uv` / `pip`)

```bash
uv tool install wayfinder-memory     # isolated, placed on your PATH
pip install wayfinder-memory         # into the current Python environment
```

Both commands install the same prebuilt native binary—the Python package is a distribution
wrapper, not a Python SDK.

### Shell installer

```bash
curl -fsSL https://raw.githubusercontent.com/zaterka/wayfinder/main/scripts/install.sh | bash
```

The script downloads the release archive for your OS and architecture, verifies its SHA-256
checksum against the published `.sha256` file, and installs into `~/.local/bin`.

Environment variables recognized by `install.sh`:

| Variable | Default | Purpose |
|---|---|---|
| `WAYFINDER_VERSION` | latest release | Pin a specific release tag (e.g. `WAYFINDER_VERSION=v0.1.7`) |
| `WAYFINDER_INSTALL_DIR` | `~/.local/bin` | Target directory for the installed binary |
| `WAYFINDER_BASE_URL` | GitHub Releases URL | Override the release host for mirrors or air-gapped tests |

If `~/.local/bin` is not on your `PATH`, add it to your shell profile:

```bash
export PATH="$HOME/.local/bin:$PATH"
```

### Building from source

Requires **Rust 1.89+** (`cargo`). SQLite is bundled at compile time, so no system database
libraries are needed:

```bash
cargo install --git https://github.com/zaterka/wayfinder wayfinder
```

## Upgrading & update checks

Upgrade an existing installation in place (with SHA-256 checksum verification):

```bash
wayfinder --upgrade
```

Wayfinder checks for a newer release at most once per day, caches the result under
`~/.cache/wayfinder/`, and prints a single notice line when an update is available. To disable
the check (for example in CI or air-gapped environments):

```bash
export WAYFINDER_NO_UPDATE_CHECK=1
```

## Troubleshooting

### Corrupt or out-of-sync SQLite index

The Markdown files in `.wayfinder/memory/<id>.md` are the sole source of truth;
`.wayfinder/index.db` is a derived SQLite database (FTS5 + graph edges). If the index is ever
corrupted or out of sync with the working tree:

```bash
wayfinder reindex
```

Or delete `.wayfinder/index.db` directly—Wayfinder rebuilds it automatically on the next read
or search.

### Revision mismatch (`--expected-rev`) when saving

When two agents race to update the same page, the second writer receives a revision mismatch
naming the current `rev` hash on disk. This is an intentional fence against lost updates: re-read
the page (`wayfinder read <id>`), merge the new findings, and save with the updated revision.

### Resolving `.wayfinder/` across git worktrees and subdirectories

Wayfinder resolves the pool root in this order:
1. `--root <path>` CLI flag
2. `WAYFINDER_ROOT` environment variable
3. Nearest ancestor directory containing `.wayfinder/` or `.git/`

Because `.git` resolution works across linked git worktrees, branches and worktrees share one
memory pool instead of forking it. If an agent runs from a directory outside the repository,
pass `--root` or set `WAYFINDER_ROOT`.

### Updating or removing harness configuration

`wayfinder install <harness>` merges its configuration idempotently:
- Existing MCP servers in `.mcp.json`, `.cursor/mcp.json`, `.vscode/mcp.json`,
  `.gemini/settings.json`, or `~/.codex/config.toml` are preserved.
- Instruction blocks in `CLAUDE.md`, `AGENTS.md`, `GEMINI.md`, `.cursor/rules/wayfinder.mdc`, or
  `.github/copilot-instructions.md` sit between markers so re-running `wayfinder install` updates
  the block in place without touching your own instructions.

