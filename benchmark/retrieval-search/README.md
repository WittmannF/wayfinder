# Related-page retrieval benchmark

This small benchmark checks whether search can retrieve pages connected to a direct text match through shared graph entities. It also records warm CLI latency and checks that an unrelated control page stays out of the results.

## Run

From the repository root:

```sh
cargo build
python3 benchmark/retrieval-search/measure.py
```

The script uses only Python's standard library. It creates a temporary 55-page pool, runs ten warmups, then measures 100 searches by default. Override the binary or run counts with `--binary`, `--warmups`, and `--runs`.

The timing includes starting the CLI process and running the indexed search. It excludes pool creation and indexing. Results are printed as JSON.

## Recorded comparison

The before and after runs used the same synthetic corpus, query, debug build, Rust 1.89 toolchain, and three paired measurement rounds. Each round used ten warmups and 100 measured searches. The values below are medians across the three round medians (and p95s).

| Measure | Before graph expansion | After graph expansion |
| --- | ---: | ---: |
| Related-page recall@8 | 0/3 | 3/3 |
| Related page ranks | — | 2, 3, 4 |
| Direct match rank | 1 | 1 |
| Unrelated control returned | No | No |
| Median latency | 13.042 ms | 14.274 ms |
| p95 latency | 14.992 ms | 17.085 ms |

On this fixture, the change added 1.232 ms to median latency (+9.4%) and 2.093 ms to p95 (+14.0%), while retrieving all three related pages in the top eight. These figures describe this local synthetic benchmark, not general production latency.
