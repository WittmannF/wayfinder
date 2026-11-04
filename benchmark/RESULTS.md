# Wayfinder benchmark

- Model route: `kornerstone` / `qwen38-27b-fp8`
- Preset: `standard`
- Checkout: `cc35018ff00f34b1e199c11c899bb461b4ad389b`
- Arms: none, llm-wiki, wayfinder, graphify
- Cases: ownership, commands, stale-page
- Repeats: 1

## Follow-up medians

| Arm | Wall ms | Total tokens | Tool calls | Passed / sessions |
|---|---|---|---|---|
| graphify | 69415 | 71737 | 5 | 6 / 6 |
| llm-wiki | 64972 | 83824 | 5 | 6 / 6 |
| none | 61440 | 68384 | 4 | 6 / 6 |
| wayfinder | 53534 | 47677 | 4 | 6 / 6 |

## Against no tool

| Arm | Wall | Tokens | Tool calls |
|---|---|---|---|
| graphify | +13% | +5% | +25% |
| llm-wiki | +6% | +23% | +25% |
| wayfinder | -13% | -30% | +0% |

Negative is cheaper than no tool. `charged`: a follow-up that failed ground truth still counts.

## Per case

- `commands`: Environment facts: the repository command surface
- `ownership`: Package ownership: session-log persistence and its compression option
- `stale-page`: Stale page: the file a discovery session cited changes before the follow-ups

| Case | Arm | Follow-up | Tokens | Wall ms | Memory calls | Reused without read | Verdict |
|---|---|---|---|---|---|---|---|
| ownership | none | compression-file | 114395 | 84183 | 0 | no | pass |
| ownership | none | compression-config | 68384 | 50116 | 0 | no | pass |
| ownership | llm-wiki | compression-file | 47172 | 49189 | 4 | yes | pass |
| ownership | llm-wiki | compression-config | 46432 | 32635 | 3 | yes | pass |
| ownership | wayfinder | compression-file | 68315 | 96076 | 6 | no | pass |
| ownership | wayfinder | compression-config | 47215 | 37943 | 4 | yes | pass |
| commands | none | coverage-and-typecheck | 32158 | 33116 | 0 | no | pass |
| commands | none | unit-tests | 80850 | 61440 | 0 | no | pass |
| commands | llm-wiki | coverage-and-typecheck | 67456 | 52281 | 3 | no | pass |
| commands | llm-wiki | unit-tests | 83824 | 131776 | 4 | no | pass |
| commands | wayfinder | coverage-and-typecheck | 47677 | 53534 | 4 | yes | pass |
| commands | wayfinder | unit-tests | 46858 | 36397 | 4 | yes | pass |
| ownership | graphify | compression-file | 71737 | 68784 | 0 | no | pass |
| ownership | graphify | compression-config | 132216 | 103752 | 0 | no | pass |
| commands | graphify | coverage-and-typecheck | 119329 | 76128 | 0 | no | pass |
| commands | graphify | unit-tests | 53452 | 40176 | 0 | no | pass |
| stale-page | none | note-header | 30463 | 23407 | 0 | no | pass |
| stale-page | none | note-folder | 53346 | 92786 | 0 | no | pass |
| stale-page | llm-wiki | note-header | 100000 | 64972 | 3 | no | pass |
| stale-page | llm-wiki | note-folder | 160285 | 185796 | 7 | no | pass |
| stale-page | wayfinder | note-header | 33852 | 38100 | 1 | no | pass |
| stale-page | wayfinder | note-folder | 55883 | 77440 | 2 | no | pass |
| stale-page | graphify | note-header | 48393 | 51156 | 0 | no | pass |
| stale-page | graphify | note-folder | 33376 | 69415 | 0 | no | pass |

## Discovery

| Case | Arm | Tokens | Wall ms | Tool calls | Memory calls |
|---|---|---|---|---|---|
| ownership | llm-wiki | 413837 | 378449 | 16 | 5 |
| ownership | wayfinder | 327787 | 314214 | 12 | 3 |
| commands | llm-wiki | 213563 | 212087 | 12 | 2 |
| commands | wayfinder | 345999 | 266566 | 14 | 1 |
| ownership | graphify | 150928 | 257320 | 10 | 0 |
| commands | graphify | 228915 | 311469 | 11 | 0 |
| stale-page | llm-wiki | 207723 | 309791 | 12 | 3 |
| stale-page | wayfinder | 166572 | 305954 | 11 | 1 |
| stale-page | graphify | 179243 | 386134 | 13 | 0 |
