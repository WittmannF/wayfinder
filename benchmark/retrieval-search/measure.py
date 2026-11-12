#!/usr/bin/env python3
"""Measure retrieval of pages related through shared graph entities.

Uses only the Python standard library. The harness creates a temporary Wayfinder
pool, seeds a fixed corpus, and measures the CLI's warm search latency.
"""

from __future__ import annotations

import argparse
import json
import math
import statistics
import subprocess
import tempfile
import time
from pathlib import Path


QUERY = "merchant invoice retention policy"
RELATED = {
    "csv-quoting-gotcha",
    "export-date-boundary-gotcha",
    "export-pagination-decision",
}
UNRELATED = "billing-retry-notes"
DIRECT = "export-retention-policy"


def run(args: list[str], *, input_text: str | None = None) -> str:
    result = subprocess.run(
        args,
        input=input_text,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=True,
    )
    return result.stdout


def save(binary: Path, root: Path, title: str, body: str, **options: str) -> None:
    args = [str(binary), "--root", str(root), "save", "--title", title, "--id", options.pop("id")]
    for key, value in options.items():
        args.extend([f"--{key}", value])
    args.extend(["--body", "-"])
    run(args, input_text=body)


def seed(binary: Path, root: Path) -> None:
    run([str(binary), "--root", str(root), "init"])
    save(
        binary, root, "Export retention policy",
        "Merchant invoices are retained for seven years before export cleanup.",
        **{"id": DIRECT, "kind": "decision", "subject": "merchant-invoice-export", "anchor": "src/export.rs"},
    )
    save(
        binary, root, "CSV quoting gotcha",
        "Exported merchant invoices must quote commas and embedded newlines.",
        **{"id": "csv-quoting-gotcha", "kind": "gotcha", "subject": "merchant-invoice-export", "anchor": "src/export.rs"},
    )
    save(
        binary, root, "Export date boundary gotcha",
        "Retention cutoff uses the merchant timezone and includes the final day.",
        **{"id": "export-date-boundary-gotcha", "kind": "gotcha", "subject": "merchant-invoice-export", "anchor": "src/export.rs"},
    )
    save(
        binary, root, "Export pagination decision",
        "The export job pages through invoices in stable created-at and id order.",
        **{"id": "export-pagination-decision", "kind": "decision", "subject": "merchant-invoice-export", "depends": "csv", "anchor": "src/export.rs"},
    )
    save(
        binary, root, "Billing retry notes",
        "Billing retries use exponential backoff after a gateway timeout.",
        **{"id": UNRELATED, "kind": "fact", "subject": "billing-retries", "anchor": "src/billing.rs"},
    )
    for number in range(50):
        save(
            binary, root, f"Unrelated migration note {number:02d}",
            f"Migration batch {number:02d} copies archived account settings.",
            **{"id": f"migration-note-{number:02d}", "kind": "fact", "subject": f"migration-{number:02d}", "anchor": "src/migrations.rs"},
        )


def percentile95(values: list[float]) -> float:
    ordered = sorted(values)
    return ordered[math.ceil(0.95 * len(ordered)) - 1]


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=Path("target/debug/wayfinder"))
    parser.add_argument("--warmups", type=int, default=10)
    parser.add_argument("--runs", type=int, default=100)
    args = parser.parse_args()
    binary = args.binary.resolve()
    if args.runs < 1 or args.warmups < 0:
        parser.error("--runs must be positive and --warmups cannot be negative")
    if not binary.is_file():
        parser.error(f"Wayfinder binary not found: {binary}; build it with cargo build")

    with tempfile.TemporaryDirectory(prefix="wayfinder-retrieval-benchmark-") as temp:
        root = Path(temp)
        (root / "src").mkdir()
        (root / "src/export.rs").write_text("// benchmark fixture\n")
        (root / "src/billing.rs").write_text("// benchmark fixture\n")
        (root / "src/migrations.rs").write_text("// benchmark fixture\n")
        seed(binary, root)

        command = [str(binary), "--root", str(root), "search", "--json", QUERY]
        for _ in range(args.warmups):
            run(command)

        elapsed_ms: list[float] = []
        latest: dict = {}
        for _ in range(args.runs):
            started = time.perf_counter_ns()
            output = run(command)
            elapsed_ms.append((time.perf_counter_ns() - started) / 1_000_000)
            latest = json.loads(output)

    pages = latest.get("results", latest)
    if isinstance(pages, dict):
        pages = pages.get("pages", [])
    ids = [page.get("id") or page.get("page_id") for page in pages]
    ids = [page_id for page_id in ids if page_id]
    top8 = ids[:8]
    found_related = RELATED.intersection(top8)
    result = {
        "query": QUERY,
        "corpus_pages": 55,
        "warmups": args.warmups,
        "runs": args.runs,
        "related_ids": sorted(RELATED),
        "related_recall_at_8": f"{len(found_related)}/{len(RELATED)}",
        "related_ranks": {page_id: ids.index(page_id) + 1 if page_id in ids else None for page_id in sorted(RELATED)},
        "direct_match_rank": ids.index(DIRECT) + 1 if DIRECT in ids else None,
        "expanded_result_count": len(found_related),
        "unrelated_control_returned": UNRELATED in ids,
        "latency_ms": {
            "median": round(statistics.median(elapsed_ms), 3),
            "p95": round(percentile95(elapsed_ms), 3),
            "min": round(min(elapsed_ms), 3),
            "max": round(max(elapsed_ms), 3),
        },
    }
    print(json.dumps(result, indent=2))


if __name__ == "__main__":
    main()
