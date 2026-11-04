# Benchmark: four tool surfaces, one agent loop, one model

`wayfinder-bench` asks the same questions with and without a memory tool and reports what
each surface cost in wall time and tokens.

The comparison exists to test one idea: the **LLM Wiki** — an agent that writes what it
learns into a wiki it reads next session, instead of re-deriving it. Andrej Karpathy sketched
the approach in [a gist](https://gist.github.com/karpathy/442a6bf555914893e9891c11519de94f);
this benchmark measures it against doing without, and against a code-graph tool that answers
a different question.

| Arm | What it is |
|---|---|
| `none` | No memory tool. The control. |
| `llm-wiki` | The LLM Wiki idea implemented **in-process** by the agent harness under test: memory tools plus a prompt digest. |
| `wayfinder` | The same idea as a standalone tool: this project over MCP, with its instruction block and a pool inside the worktree. |
| `graphify` | A different idea — a code knowledge graph built ahead of time and queried through the shell. |

The six cases are onboarding questions about the benchmarked checkout: who owns a subsystem,
what the house conventions require, which command does what, how a format is versioned, what
happens when a cited file changes, and how far a fact fans out. They live in
[`cases.json`](cases.json).

## Arms

Every arm runs the same agent CLI, booted in its headless profile, against the same pinned
commit, in its own detached worktree, with its own home directory. The profile patch selects
plaintext session logs, so every figure is folded from what the agent loop wrote rather than
from the process. The CLI under test comes from `--agent-dir` (or `WAYFINDER_AGENT_DIR`);
the runner names no default.

### The adapter for the CLI under test

The portable part is the arm definition: which tools the model can see and which profile the
loop boots with. The runner also carries an adapter for the specific agent CLI it was written
against — the plugin ids in the profile patch, the settings document path, and the source
entry it launches. Point `--agent-dir` at that checkout and the adapter configures it; a
different CLI needs its own adapter (a patch file and a launch command), not a different
benchmark.

## Series

One *series* is a discovery session (tool arms only) followed by the case's follow-up
questions in a fresh session. A follow-up never names memory: both a returning user
and a cold one receive the same words.

## What is measured

- Provider-reported input, output, and total tokens, summed over the session.
- Wall time from the first to the last logged event.
- Tool calls, memory calls split by kind, and whether a session answered without
  reopening the workspace after its first memory call.
- Ground truth: a substring check over the final answer, so a confident wrong answer that
  happens to name the right identifiers passes, and a correct paraphrase in other words
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
