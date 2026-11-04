# Benchmark

Does reusing a wiki actually pay? This page answers with numbers, says how they were
obtained, and is explicit about what they do not prove.

Headline, measured against an agent with **no memory tool at all**, on the same model, the
same cases, and the same agent loop:

| Surface | Follow-up wall time | Follow-up tokens |
|---|---|---|
| **wayfinder** (this project, over MCP) | **−13 %** | **−30 %** |
| LLM Wiki implemented in-process by the harness under test | +6 % | +23 % |
| graphify (code graph, queried through the shell) | +13 % | +5 % |

Every follow-up in every arm passed the same ground-truth check, so no arm bought its
number by answering less often.

## What was compared

The **LLM Wiki** idea: an agent writes what it learns into a wiki it reads next session
instead of re-deriving it ([Karpathy's gist](https://gist.github.com/karpathy/442a6bf555914893e9891c11519de94f)).
Four surfaces test it:

| Arm | What the model can call |
|---|---|
| `none` | Nothing. The control. |
| `llm-wiki` | The idea implemented in-process by the harness under test: memory tools plus a prompt digest. |
| `wayfinder` | The idea as a standalone tool: MCP tools, an instruction block, a pool in the worktree. |
| `graphify` | A different idea: a code knowledge graph built ahead of time, queried through the shell. |

Three of the six cases, one repeat:

- **ownership** — which package owns durable session-log persistence, and what its
  `compression` option accepts.
- **commands** — the exact build/type-check/test/coverage commands and the Node range.
- **stale-page** — the file a discovery session cited changes before the follow-ups, so a
  page that was true is now wrong.

Per-case follow-up tokens (two follow-ups each):

| Case | none | llm-wiki | wayfinder | graphify |
|---|---|---|---|---|
| ownership | 182 779 | **93 604** | 115 530 | 203 953 |
| commands | 113 008 | 151 280 | **94 535** | 172 781 |
| stale-page | 83 809 | 260 285 | 89 735 | **81 769** |

The per-case split is the interesting result, and it is not a clean win:

- The **in-process wiki** wins package ownership decisively (half the tokens) and loses the
  command surface and the drift case. Its digest is injected for free, but when the answer
  is a literal string from `package.json` the model reads the file anyway, and after a cited
  file changes it reconciles drift that costs more than re-deriving.
- **wayfinder** wins two of three. It pays a fixed cost for a pool that has to be searched
  explicitly rather than being pushed into the prompt, which is why its advantage is in
  tokens rather than in wall time.
- **graphify** answers a different question (structure, not findings) and lands at parity on
  cost — its value is in what a graph can answer, not in token savings.

Discovery is an investment, reported separately rather than folded into the medians:
wayfinder spent 328 k / 346 k / 167 k tokens across the three cases' discovery sessions,
llm-wiki 414 k / 214 k / 208 k, graphify 151 k / 229 k / 179 k. At the measured saving, a
pool pays for itself after roughly a dozen follow-up sessions.

## How to reproduce

```console
$ cargo build --release
$ ./target/release/wayfinder-bench --plan --repo <checkout> --agent-dir <agent-cli-checkout>
$ ./target/release/wayfinder-bench --repo <checkout> --agent-dir <agent-cli-checkout> \
      --repeats 3 --out results/run-1
$ ./target/release/wayfinder-bench --report --out results/run-1
```

Each series gets its own detached git worktree and its own harness home, so an arm can only
see the files and the tool surface it is supposed to have. Cost is folded from the session log
the agent loop wrote — provider-reported input and output tokens — not from the process.

## What these numbers do not prove

- **Three cases of six, one repeat.** The remaining cases (`conventions`, `format-version`,
  `fan-out`) and repeats 2–3 are one `--resume` away; the runner appends to the same record.
- **One repository and one model.** Qwen 3.8-27B over a self-hosted route. The facts in these
  cases are cheap to look up in a checkout, which understates a wiki on a codebase that is
  expensive to search.
- **Ground truth is substring matching.** A confident wrong answer that names the right
  identifiers passes; a correct paraphrase in other words fails.
- **The graphify arm builds a code-only graph.** Its docs pass needs one of four hardcoded
  model backends, none of which the benchmark environment can reach, so its graph is AST-only
  and it is queried through the shell rather than as MCP tools.
- **Not graphify's own headline number.** Its published reduction is measured against reading
  raw files, not against an agent doing the same task; we do not reproduce it, and a
  cross-tool comparison of the two would be apples to oranges.
- **Wall time is a shared-host measurement.** The arms run sequentially, but the endpoint's
  load moves times between runs.

The runner is `crates/wayfinder-bench`; the methodology, the counters, and the adapter
boundary are documented in [`benchmark/README.md`](benchmark/README.md), and the raw tables
from this run are in [`benchmark/RESULTS.md`](benchmark/RESULTS.md).
