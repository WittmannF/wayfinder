# Benchmark: four tool surfaces, one agent loop, one model

`wayfinder-bench` asks the same questions with and without a memory tool and reports
what each surface cost in wall time and tokens. The case matrix is the one the
Factored Harness benchmark published its v2 numbers with
([`benchmarks/llm-wiki`](https://github.com/factoredai/Factored-Harness/tree/master/benchmarks/llm-wiki)),
exported here as `cases.json` so the two runs stay comparable.

## Arms

| Arm | What the model can call |
|---|---|
| `none` | Nothing: the control. |
| `llm-wiki` | The harness's own shared memory: `memory_*` tools plus the prompt digest. |
| `wayfinder` | This project over MCP: `memory_*` tools, its instruction block, and a pool initialised in the worktree. |
| `graphify` | graphify's knowledge graph: built by `graphify extract`, queried through the shell. |

Every arm runs the Factored Harness headless profile against the same pinned commit,
in its own detached worktree, with its own `DSH_HOME`. The profile patch selects
plaintext session logs, so every figure is folded from what the harness wrote rather
than from the process.

## Series

One *series* is a discovery session (tool arms only) followed by the case's follow-up
questions in a fresh session. A follow-up never names memory: both a returning user
and a cold one receive the same words.

## What is measured

- Provider-reported input, output, and total tokens, summed over the session.
- Wall time from the first to the last logged event.
- Tool calls, memory calls split by kind, and whether a session answered without
  reopening the workspace after its first memory call.
- Ground truth: the harness benchmark's substring check, so a confident wrong answer
  fails.

## Running it

```console
$ cargo build --release
$ ./target/release/wayfinder-bench --plan --repo <checkout> --arms none,llm-wiki,wayfinder
$ ./target/release/wayfinder-bench --repo <checkout> --repeats 3 --out <results dir>
```

Worktrees and harness homes live under the platform temp directory; `--out` holds
`run.json` and `report.md`.

## Reading the numbers

`memory_calls` counts calls whose tool name carries `memory_` or the `mcp__` prefix.
The graphify arm reaches its graph through the shell (`graphify query …`), so its
memory-call column stays zero even when the model used the graph: for that arm, read
the token and wall columns, not the call counts.

`reused_without_read` means the session made a memory call and then answered without
calling a workspace-reading tool afterwards. It measures the order of tool work, not
whether the page was sufficient: a page that answered and a page that was never opened
look the same from the log.

A discovery session is charged to its arm in the discovery table and excluded from
the follow-up medians. The `charged` view — one that keeps failed attempts in the
median — is what stops an arm that answers less often from looking cheaper.

## Known limits

- One repository and one model: the facts in the case matrix are the ones a checkout
  makes cheap to look up, which understates a wiki on a larger codebase.
- A discovery session is charged to its arm as an investment; the report shows it
  separately from follow-up medians.
- graphify 0.8.x ships a CLI and a skill, not an MCP server, and its `extract` runs a
  model pass over every file. Its arm therefore needs a bounded build (a subtree or a
  token cap) before it is comparable; the current run reports the other three arms.
