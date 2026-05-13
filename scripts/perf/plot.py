#!/usr/bin/env python3
"""
Plot latency CDFs for the main DAG lane across all 7 regions.

Reads `target/perf/run/main_lane.csv` (produced by gsx-metrics) and emits:

- `target/perf/run/cdf_main_lane.png` — per-region commit-latency CDF
  (received_ms - proposed_ms for the authoring region, or
  committed_ms - proposed_ms for the joint-quorum-fires perspective).
- `target/perf/run/summary.txt` — p50 / p95 / p99 / max per region.

Usage:
    scripts/perf/plot.py [--csv path] [--out path]
"""

from __future__ import annotations

import argparse
import csv
import statistics
import sys
from pathlib import Path

try:
    import matplotlib.pyplot as plt
except ImportError:
    print("error: matplotlib not installed. pip install matplotlib", file=sys.stderr)
    sys.exit(1)


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument(
        "--csv",
        type=Path,
        default=Path("target/perf/run/main_lane.csv"),
        help="CSV from gsx-metrics with columns: cert_hash,region,proposed_ms,received_ms,voted_ms,committed_ms",
    )
    ap.add_argument(
        "--out",
        type=Path,
        default=Path("target/perf/run/cdf_main_lane.png"),
    )
    ap.add_argument(
        "--summary",
        type=Path,
        default=Path("target/perf/run/summary.txt"),
    )
    args = ap.parse_args()

    if not args.csv.exists():
        print(f"error: {args.csv} not found", file=sys.stderr)
        return 1

    # For each cert_hash, gather (proposed_ms across rows) and
    # (committed_ms per region). Plot committed_ms - proposed_ms (the cert
    # author's view of "time for joint-commit to fire").
    proposed: dict[str, int] = {}
    committed_by_region: dict[str, list[int]] = {}
    with args.csv.open() as f:
        reader = csv.DictReader(f)
        for row in reader:
            h = row["cert_hash"]
            region = row["region"]
            if row.get("proposed_ms"):
                proposed[h] = int(row["proposed_ms"])
            if row.get("committed_ms"):
                committed_by_region.setdefault(region, []).append((h, int(row["committed_ms"])))

    # Compute latencies = committed_ms - proposed_ms, dropping rows where the
    # proposed timestamp wasn't seen (cert authored on a different region).
    latencies: dict[str, list[int]] = {}
    for region, rows in committed_by_region.items():
        for h, c_ms in rows:
            if h in proposed:
                latencies.setdefault(region, []).append(c_ms - proposed[h])

    if not latencies:
        print("error: no joinable rows in CSV", file=sys.stderr)
        return 1

    # Plot.
    plt.figure(figsize=(10, 6))
    for region, lats in sorted(latencies.items()):
        lats = sorted(l for l in lats if l >= 0)
        if not lats:
            continue
        ys = [i / len(lats) for i in range(1, len(lats) + 1)]
        plt.plot(lats, ys, label=f"{region} (n={len(lats)})")
    plt.xlabel("commit latency (ms) — committed_ms − proposed_ms")
    plt.ylabel("CDF")
    plt.title("GSX DAG main-lane commit latency, 7-region testnet")
    plt.legend()
    plt.grid(True, alpha=0.3)
    plt.tight_layout()
    args.out.parent.mkdir(parents=True, exist_ok=True)
    plt.savefig(args.out, dpi=150)
    print(f"wrote {args.out}", file=sys.stderr)

    # Summary.
    with args.summary.open("w") as f:
        f.write("region            n      p50     p95     p99     max\n")
        f.write("-" * 60 + "\n")
        for region in sorted(latencies):
            lats = sorted(l for l in latencies[region] if l >= 0)
            if not lats:
                continue
            p50 = statistics.median(lats)
            p95 = lats[int(0.95 * (len(lats) - 1))]
            p99 = lats[int(0.99 * (len(lats) - 1))]
            mx = lats[-1]
            f.write(
                f"{region:16s} {len(lats):5d}  {p50:6.1f}  {p95:6.1f}  {p99:6.1f}  {mx:6.1f}\n"
            )
    print(f"wrote {args.summary}", file=sys.stderr)
    with args.summary.open() as f:
        sys.stdout.write(f.read())

    return 0


if __name__ == "__main__":
    sys.exit(main())
