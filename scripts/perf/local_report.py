#!/usr/bin/env python3
"""Summarise one local-sat.sh run and append it to the history CSV.

Joins every loadgen CSV (client_submitted_ms,tx_hash,target_idx) to the
first `committed` main-lane event carrying that intent hash on any
validator, and derives:

  offered_tps     LOADGENS x RATE_PER
  acked           intents the validators acknowledged
  committed       acked intents that reached a committed certificate
  tps             committed / load window
  p50/p95/p99     submit -> first commit latency (ms)
  round_ms        mean proposal interval on v0 during the load window
  cpu_ms_per_1k   node CPU milliseconds (all 4 nodes) per 1000 committed
  rejects         loadgen stderr lines reporting a refused submission
"""
import argparse
import csv
import datetime as dt
import glob
import json
import os
import sys

CLK_TCK = os.sysconf("SC_CLK_TCK")


def pct(xs, p):
    if not xs:
        return 0
    return xs[min(len(xs) - 1, int(len(xs) * p / 100))]


def main():
    ap = argparse.ArgumentParser()
    for a in ("out-dir", "history", "label", "sha", "cpu-start", "cpu-end"):
        ap.add_argument("--" + a, required=True)
    for a in ("loadgens", "rate-per", "duration", "round-ms", "t-start", "t-end"):
        ap.add_argument("--" + a, required=True, type=int)
    a = ap.parse_args()

    submitted = {}
    for path in glob.glob(os.path.join(a.out_dir, "loadgen", "*.csv")):
        with open(path) as f:
            for row in csv.reader(f):
                if len(row) < 2 or not row[0].isdigit():
                    continue
                t, h = int(row[0]), row[1]
                if h not in submitted or t < submitted[h]:
                    submitted[h] = t
    rejects = 0
    for path in glob.glob(os.path.join(a.out_dir, "loadgen", "*.err")):
        with open(path, errors="replace") as f:
            rejects += sum(1 for line in f if "rate limited" in line or "Err" in line or "failed" in line)

    first_commit = {}
    proposals_v0 = []
    for v in range(4):
        path = os.path.join(a.out_dir, f"v{v}", "events.ndjson")
        if not os.path.exists(path):
            continue
        with open(path, errors="replace") as f:
            for line in f:
                try:
                    e = json.loads(line)
                except ValueError:
                    continue
                if e.get("lane") != "main":
                    continue
                ev = e.get("event")
                if ev == "committed":
                    for h in e.get("intent_hashes") or ():
                        t = e["t_ms"]
                        if h not in first_commit or t < first_commit[h]:
                            first_commit[h] = t
                elif ev == "proposed" and v == 0 and a.t_start <= e["t_ms"] <= a.t_end:
                    proposals_v0.append((e["t_ms"], e.get("round", 0)))

    lat = sorted(first_commit[h] - t for h, t in submitted.items() if h in first_commit)
    committed = len(lat)
    window_s = max(1e-3, (a.t_end - a.t_start) / 1000)
    tps = committed / window_s
    round_ms = 0.0
    if len(proposals_v0) >= 2:
        proposals_v0.sort()
        (t0, r0), (t1, r1) = proposals_v0[0], proposals_v0[-1]
        if r1 > r0:
            round_ms = (t1 - t0) / (r1 - r0)
    c0 = [int(x) for x in a.cpu_start.split()]
    c1 = [int(x) for x in a.cpu_end.split()]
    cpu_ms = sum(b - x for x, b in zip(c0, c1)) * 1000 / CLK_TCK
    cpu_per_1k = cpu_ms * 1000 / committed if committed else 0.0

    row = {
        "date": dt.datetime.now(dt.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
        "label": a.label,
        "sha": a.sha,
        "loadgens": a.loadgens,
        "offered_tps": a.loadgens * a.rate_per,
        "duration_s": a.duration,
        "round_cfg_ms": a.round_ms,
        "acked": len(submitted),
        "committed": committed,
        "tps": round(tps, 1),
        "p50_ms": pct(lat, 50),
        "p95_ms": pct(lat, 95),
        "p99_ms": pct(lat, 99),
        "round_ms": round(round_ms, 1),
        "cpu_ms_per_1k": round(cpu_per_1k, 1),
        "rejects": rejects,
    }
    os.makedirs(os.path.dirname(a.history) or ".", exist_ok=True)
    new = not os.path.exists(a.history)
    with open(a.history, "a", newline="") as f:
        w = csv.DictWriter(f, fieldnames=list(row))
        if new:
            w.writeheader()
        w.writerow(row)
    for k, v in row.items():
        print(f"  {k:>14}: {v}")
    if committed == 0:
        print("no committed intents joined; check node logs and loadgen .err", file=sys.stderr)
        sys.exit(1)


if __name__ == "__main__":
    main()
