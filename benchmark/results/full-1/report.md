# Wayfinder benchmark

- Model route: `kornerstone` / `qwen38-27b-fp8`
- Preset: `standard`
- Checkout: `cc35018ff00f34b1e199c11c899bb461b4ad389b`
- Arms: none, llm-wiki, wayfinder, graphify
- Cases: ownership, conventions, commands, format-version, stale-page, fan-out
- Repeats: 3

> **Incomplete run:** 4 of 72 series recorded. Figures below cover what has run so far.

## Follow-up medians

| Arm | Wall ms | Total tokens | Tool calls | Passed / sessions |
|---|---|---|---|---|
| graphify | 105816 | 108948 | 6 | 2 / 2 |
| llm-wiki | 49189 | 47172 | 4 | 2 / 2 |
| none | 84183 | 114395 | 6 | 2 / 2 |
| wayfinder | 96076 | 68315 | 7 | 2 / 2 |

## Per case

- `ownership`: Package ownership: session-log persistence and its compression option

| Case | Arm | Follow-up | Tokens | Wall ms | Memory calls | Reused without read | Verdict |
|---|---|---|---|---|---|---|---|
| ownership | none | compression-file | 114395 | 84183 | 0 | no | pass |
| ownership | none | compression-config | 68384 | 50116 | 0 | no | pass |
| ownership | llm-wiki | compression-file | 47172 | 49189 | 4 | yes | pass |
| ownership | llm-wiki | compression-config | 46432 | 32635 | 3 | yes | pass |
| ownership | wayfinder | compression-file | 68315 | 96076 | 6 | no | pass |
| ownership | wayfinder | compression-config | 47215 | 37943 | 4 | yes | pass |
| ownership | graphify | compression-file | 108948 | 105816 | 0 | no | pass |
| ownership | graphify | compression-config | 52184 | 72504 | 0 | no | pass |

## Discovery

| Case | Arm | Tokens | Wall ms | Tool calls | Memory calls |
|---|---|---|---|---|---|
| ownership | llm-wiki | 413837 | 378449 | 16 | 5 |
| ownership | wayfinder | 327787 | 314214 | 12 | 3 |
| ownership | graphify | 180073 | 348264 | 10 | 0 |
