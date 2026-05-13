# GSX DAG perf run — 2026-05-12

Real 6-region AWS testnet, ~50 minutes of validator runtime, 60s of 50tps load
from us-east-1 client port. Captured ~10 distinct DAG round-0/1 certs
propagating cross-region.

## What ran
- 6 t3.small instances in us-east-1, us-west-2, eu-west-1, ap-northeast-1,
  ap-southeast-2, sa-east-1. (af-south-1 dropped — region opt-in needed.)
- Each running `gsx-node` daemon over real TCP wire.
- Mysticeti-C round driver + joint-quorum voter + block executor active.
- Genesis manifest: 6 validators with placeholder ML-DSA / BLS keys.

## What we measured
`cross_region_latency.csv` — median + max ms from `proposed` (author) to
`received` (peer) for each authoring-region → observing-region pair.

Highlights, matching textbook AWS RTTs:
| Path | n | p50 ms | Reference RTT/2 |
|------|---|--------|-----------------|
| us-east-1 → us-west-2 | 2 | 64 | ~30 ms |
| eu-west-1 → us-east-1 | 1 | 101 | ~37 ms |
| ap-southeast-2 → us-east-1 | 2 | 199 | ~100 ms |
| ap-northeast-1 → us-west-2 | 1 | 145 | ~60 ms |

(p50 is ~2x the one-way reference; consistent with our event timestamps
being captured at the application layer after kernel + bincode-decode + a
tokio scheduling hop, not at the NIC.)

## What didn't work
- The round driver stopped advancing after a few rounds because it waits
  for `quorum_threshold(n)=5` distinct authors at round R-1 before
  authoring R+1. Cross-region propagation was patchy enough that not all
  5 observed in time, so the driver stalled.
- `joint_commit` never fired anywhere. No `committed` events were emitted.
- Of 3000 intents submitted by gsx-loadgen on us-east-1, only 86 made it
  into the daemon's event log — the daemon stalled mid-load.

## Why
Real-network conditions exposed the round driver's brittleness — the
in-process 4-node loopback test (`daemon::tests::four_node_main_lane_commits`)
always sees all peers within a few ms, so the quorum-wait gate fires
reliably. Across geography it doesn't.

The fix is the Mysticeti-C "fall through with f+1 parents" recovery the
paper describes (§6.2) — not yet implemented. Tracked as a follow-up.

## Files
- `*.ndjson` — raw event logs pulled from each validator via SSM.
- `main_lane.csv` — joined by gsx-metrics, columns
  `cert_hash,region,proposed_ms,received_ms,voted_ms,committed_ms`.
- `cross_region_latency.csv` — pair latencies summary.
