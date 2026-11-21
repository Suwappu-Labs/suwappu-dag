#!/usr/bin/env python3
"""Render docs/perf/local-history.csv as a self-contained HTML page.

    python3 scripts/perf/render_history.py OUT.html

Charts node CPU per 1k committed intents across perf-loop iterations
(8 loadgens, 50 ms rounds; iterations from docs/perf/iterations.json),
p50 latency against the round timer, and lists every run.
"""
import csv
import json
import os
import re
import statistics
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))


def load():
    with open(os.path.join(ROOT, "docs/perf/local-history.csv")) as f:
        rows = list(csv.DictReader(f))
    with open(os.path.join(ROOT, "docs/perf/iterations.json")) as f:
        its = json.load(f)["iterations"]
    for r in rows:
        for k in ("loadgens", "offered_tps", "round_cfg_ms", "acked", "committed", "p50_ms", "p95_ms", "p99_ms", "rejects"):
            r[k] = int(r[k])
        for k in ("tps", "round_ms", "cpu_ms_per_1k"):
            r[k] = float(r[k])
    by_n = {it["n"]: dict(it, runs=[]) for it in its}
    for r in rows:
        if r["loadgens"] != 8 or r["round_cfg_ms"] != 50:
            continue
        m = re.match(r"it(\d+)-", r["label"])
        n = int(m.group(1)) if m else next((it["n"] for it in its if r["label"] in it.get("labels", [])), None)
        if n in by_n:
            by_n[n]["runs"].append({k: r[k] for k in ("label", "sha", "date", "cpu_ms_per_1k", "p50_ms", "p99_ms", "tps")})
    iterations = [v for _, v in sorted(by_n.items()) if v["runs"]]
    for it in iterations:
        it["mean"] = round(statistics.mean(x["cpu_ms_per_1k"] for x in it["runs"]), 1)
    latency = []
    for ms in (250, 100, 50):
        base = [r for r in rows if r["round_cfg_ms"] == ms and r["loadgens"] == 8 and (r["label"].startswith("round-") or r["label"] == "baseline-8lg")]
        if base:
            latency.append({"round": ms, "p50": base[0]["p50_ms"], "p99": base[0]["p99_ms"], "label": base[0]["label"]})
    final25 = [r for r in rows if r["label"] == "final-25ms"]
    if final25:
        best = min(final25, key=lambda r: r["p99_ms"])
        latency.append({"round": 25, "p50": best["p50_ms"], "p99": best["p99_ms"], "label": best["label"] + " (final build)"})
    return rows, iterations, latency


def main():
    out = sys.argv[1]
    rows, iterations, latency = load()
    kept = [it for it in iterations if not it.get("reverted")]
    baseline, current = kept[0], kept[-1]
    final = [r for r in rows if r["label"].startswith("final-")]
    peak = max(final or rows, key=lambda r: r["tps"])
    data = {
        "rows": rows,
        "iterations": iterations,
        "latency": latency,
        "summary": {
            "baseline_cpu": baseline["mean"],
            "current_cpu": current["mean"],
            "current_name": current["name"],
            "delta_pct": round((current["mean"] - baseline["mean"]) * 100 / baseline["mean"]),
            "p50_50ms": min(x["p50_ms"] for x in current["runs"]),
            "peak_tps": peak["tps"],
            "peak_label": peak["label"],
            "peak_loadgens": peak["loadgens"],
            "peak_p99": peak["p99_ms"],
            "peak_round": peak["round_cfg_ms"],
        },
    }
    with open(os.path.join(os.path.dirname(os.path.abspath(__file__)), "history_template.html")) as f:
        page = f.read()
    page = page.replace("/*__DATA__*/null", json.dumps(data))
    with open(out, "w") as f:
        f.write(page)
    print(f"wrote {out}: {len(rows)} runs, {len(iterations)} iterations")


if __name__ == "__main__":
    main()
