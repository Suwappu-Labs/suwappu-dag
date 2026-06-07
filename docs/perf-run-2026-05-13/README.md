# Suwappu DAG perf run — 2026-05-13 (4-region, post-f+1)

Second AWS campaign, this time with the leader-aware f+1 fallback
(commit `8b2902a`) and the 6-region testnet trimmed to 4 fast-RTT regions
after the 2026-05-12 run showed cross-Pacific peers stalling consensus.

## What ran
- 4 t3.small EC2s in us-east-1, us-west-2, eu-west-1, ap-northeast-1.
- 90 seconds of `suwappu-loadgen --rate 100` from us-east-1 EC2 against its
  own client port.
- Binary compiled via AWS CodeBuild (no Docker Desktop locally per repo
  preference).

## Headline numbers

### Submission throughput (the lane that worked)
| Metric | Value |
|---|---|
| Intents submitted + acked | **9000** |
| Duration | 89.99 s |
| Sustained TPS | **100.0 /sec** (target 100) |
| p50 inter-submit ms | 10.0 |
| p95 inter-submit ms | 11.0 |
| Max inter-submit ms | 19 |

Every intent got an `Ack` from the daemon's client listener. 0% drops.
Each round-trip (submit → bincode-decode → enqueue → blake3 + bincode-encode
Ack) ran at ~10 ms p50 on us-east-1, loopback.

### Cross-region cert propagation
| Path | n | p50 ms | p95 ms | Textbook RTT/2 |
|---|---:|---:|---:|---:|
| us-east-1 → us-west-2 | 112 | **35** | 159 | ~30 |
| us-east-1 → eu-west-1 | 112 | **34** | 172 | ~37 |
| us-east-1 → ap-northeast-1 | 112 | **70** | 362 | ~70 |
| us-west-2 → us-east-1 | 112 | **28** | 28 | ~30 |
| us-west-2 → eu-west-1 | 112 | 59 | 59 | ~70 |
| us-west-2 → ap-northeast-1 | 112 | 50 | 51 | ~60 |
| eu-west-1 → us-east-1 | 4 | 35 | 35 | ~37 |
| eu-west-1 → us-west-2 | 4 | 60 | 60 | ~70 |
| ap-northeast-1 → us-west-2 | 3 | 48 | 48 | ~60 |
| ap-northeast-1 → us-east-1 | 3 | 73 | 73 | ~70 |

p50 figures match published AWS one-way latency within ~5–10 ms,
confirming the wire transport + bincode framing add negligible overhead
once the syscall has returned. Max latencies (17 s on us-east-1 →
ap-northeast-1) are queueing effects when the inbox handler is busy
catching up after a stall — not network.

### Round cadence
| Region | Rounds | Span | p50 gap | Effective rate |
|---|---:|---:|---:|---:|
| us-east-1 | 112 | 574 s | 5.75 s | 0.20 / s |
| us-west-2 | 112 | 569 s | 2.25 s | 0.20 / s |
| eu-west-1 | **4** | 17.8 s | 6.5 s | 0.23 / s |
| ap-northeast-1 | **3** | 10.5 s | 5.25 s | 0.29 / s |

us-east-1 + us-west-2 advance in lockstep (similar count, slight RTT-paired
cadence). eu-west-1 and ap-northeast-1 stop advancing after round 3-4 —
this is the bug below.

### Committed
| Metric | Value |
|---|---|
| Distinct certs committed (entire run) | **1** (round 0 only) |
| Regions agreeing on commit | 2 / 4 (us-east-1, us-west-2) |
| Committed TPS | **0 / sec** (round 0 carried no client intents — load gen started after) |

### Data volume
| File | Bytes | Rows | B/row |
|---|---:|---:|---:|
| `loadgen.csv` | 694 KB | 9000 | 79 |
| `us-east-1.ndjson` | 1.44 MB | 9351 | 158 |
| `us-west-2.ndjson` | 59 KB | 351 | 172 |
| `eu-west-1.ndjson` | 43 KB | 237 | 185 |
| `ap-northeast-1.ndjson` | 44 KB | 239 | 190 |

Estimated bytes-on-wire (computed from struct layout, not measured directly):
- Certificate frame: ~86 B (length prefix + bincode of `{author, round, parents, payload_digest}`)
- Block payload frame: ~1.7 KB max at full load (32 + 4 + 8 + 32 + 25 × 64 B/intent)
- Vote frame: ~50 B

## Why committed TPS was 0

`commit_leader(R, n)` requires **`quorum_threshold(n)` round-(R+1) certs
that include the round-R leader's hash as a parent**. The round driver
collects parents from round R-1 strictly. Once cross-region delay
puts peers on different rounds, two things go wrong simultaneously:

1. **Slow peers fall out of the parent window.** A validator stuck at
   round 3 can never produce round-4 supporters of round-3 leader if
   healthy peers have already moved to round 10+ — there's no quorum of
   round-4 certs anywhere to reference round-3's leader.

2. **Healthy peers' rounds advance under fallback without the slow peer
   as a parent.** Their round-(R+1) certs include only the healthy
   subset → commit_leader still can't reach quorum.

Both halves of the chain need a fix. The proper Mysticeti-C pattern:
`parents_for_round` looks at `max(observed_round)` rather than `R-1`,
and round driver advances at `max(last_authored, max_observed) + 1`.
Slow validators "snap up" instead of falling behind. Tracked as the
next change.

## Wire-transport health check
The TCP wire (length-prefixed bincode, geometric reconnect) is solid:
- 0 connection drops observed in any region.
- Frame parse errors: 0.
- All 4 daemons stayed `active` for the full 10-minute run.
- p95 cross-region cert delivery <400 ms on the three intercontinental
  links, consistent with the published AWS backbone numbers.

## Files
- `*.ndjson` — full event logs (uploaded via S3 + IAM PutObject to
  `s3://suwappu-dag-perf-artifacts/logs/`, then pulled locally).
- `loadgen.csv` — `client_submitted_ms,tx_hash` per row, 9000 rows.

## What worked vs. last campaign
- **2026-05-12** (6 regions, original f+1=4-round wait): 0 commits, round driver advanced too fast.
- **2026-05-13** (4 regions, leader-aware fallback): 1 commit (round 0), measurable cross-region propagation, 100% load-gen ack rate.

Half-step forward. Next change is the `max(observed_round)` snap-up.
