# wayfinder-memory

Installs the **wayfinder** binary from its release: workspace memory that coding agents write
to — a self-generating knowledge graph and an LLM Wiki in one tool.

This package is an installer, not an SDK: it carries the released executable for your
platform and puts the `wayfinder` command on the path. The distribution is named
`wayfinder-memory` because `wayfinder` on PyPI is an unrelated, dormant package.

```console
$ uv tool install wayfinder-memory   # isolated app environment
$ pip install wayfinder-memory      # into the current Python environment
$ wayfinder --help
```

Linux and macOS, arm64 or x86_64. On any other platform, build from source:

```console
$ cargo install --git https://github.com/zaterka/wayfinder wayfinder
```

The command behaves exactly as the release binary does: `wayfinder init`, `seed`, `search`,
`graph`, `install <harness>`, `mcp`, and `--upgrade` to fetch a newer release.

Docs, source, and the benchmark: <https://github.com/zaterka/wayfinder>.
