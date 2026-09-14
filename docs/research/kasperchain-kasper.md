# Research note — `kasperchain/kasper`

**Date:** 2026-09-07
**Question:** Is there anything in <https://github.com/kasperchain/kasper> that
`suwappu-dag` can use?
**Verdict:** No code. Three small operator-UX ideas are worth carrying into
Phase 4/5 work; they are listed at the end with the concrete place each one
lands.

## What the repository actually is

The README describes "Kasperchain", a GPU-only proof-of-work Layer 1 with a
custom hash called "Blake3v2" (BLAKE3 compression plus parallel mixing plus a
memory-mixing stage), a 21M `KASP` supply, 60 s blocks, and Bitcoin-style
halvings. None of that is implemented in the repository.

The code is a mechanical rename of Kaspa's deprecated **KDX** desktop app
(`aspectron/kdx`, "Kaspa Desktop eXperience"). Evidence, all reproducible
from a shallow clone of both repositories:

| Check | Result |
|---|---|
| `package.json` description | Still reads `"Kaspa Desktop eXperience"`, version `2.12.10` (KDX's last version) |
| Line-ending normalised diff after `kasperchain→aspectron`, `kasper→kdx` | 56 changed lines across 22 source files, every one a `KDX`/`kdx` capitalisation artefact of the search-and-replace |
| `lib/daemon.js`, `lib/manager.js`, `lib/interfaces/kaspad.js` | Identical to upstream KDX |
| `.emanate` build script | Still clones `github.com/kaspanet/kaspad` (Go) and `kaspa-miner` (Rust, kHeavyHash) as the actual node and miner |
| Search for `blake3v2` outside `README.md` | Zero hits |
| npm packages `@kasperchain/flow-app`, `flow-ux`, `flow-async`, `process-list` | Do not exist on the registry, so `npm install` fails |
| Commit history | 1 commit (2026-08-25), README only. 1 star, 0 forks |
| License | None at the root. Upstream KDX also ships no `LICENSE` and no `license` field, so there is no grant to copy from either |
| Upstream status | KDX was marked deprecated on 2025-04-29 and replaced by `aspectron/kaspa-ng` |

So the repository is a process-manager shell for a Go `kaspad` binary, wrapped
in a README for a chain that does not exist. The underlying chain it would
drive (Kaspa: PoW, GHOSTDAG ordering, kHeavyHash, secp256k1) is orthogonal to
`suwappu-dag` (BFT certificate DAG, joint-quorum finality, ML-DSA-65). Nothing
in it touches our load-bearing invariants; adopting its hash or consensus
would violate invariants 1 and 2 outright.

## What KDX does that we do not (yet)

KDX is a supervisor for node + miner + wallet processes. Stripped of the
NW.js UI, the reusable behaviours are:

1. **Sync-progress estimate for operators.** `kaspad.js::getStats` polls the
   node RPC every 1.25 s, records the first observed past-median-time in a
   `sync.json` marker, and reports `% synced` and `median latency` against
   wall clock. Operators get a single number instead of block counts.
2. **NAT traversal for home-run nodes.** UPnP mapping via `nat-upnp` with a
   2 h lease, renewal at TTL/4, two retries on a random alternate port, and
   automatic `externalip` rewriting. Opt-in.
3. **Storage growth-rate sampling.** `du` on the data directory every 30 s
   under the UI, deriving bytes/hour and a projected bytes/year over a
   5-sample window. Surfaced as a dashboard badge.
4. Process supervision: relaunch with delay, restart-count tolerance window,
   15 MB log rotation, per-process CPU/RSS sampling. We already get all of
   this from systemd + CloudWatch on the seed cluster.

## What we can use, and where

| Idea | Lands in | Effort | Priority |
|---|---|---|---|
| `suwappu_getSyncStatus` JSON-RPC method: local committed round, highest peer `Tip` seen by the catch-up backfill (`crates/suwappu-node/src/daemon.rs`, forward backfill), rounds-behind, and a `synced: bool` | `crates/suwappu-rpc` + `suwappu-node` RPC adapter | Small | **Do for G8 status page / G7 explorer.** We have no sync-state method today (`router.rs` lists 9 methods, none reports sync). External validators in `docs/testnet/VALIDATOR-OPERATORS.md` are told they will be "syncing passively" with no way to observe progress. |
| State-directory growth-rate gauge on `/metrics` (`suwappu_state_bytes`, derive rate in CloudWatch) | G6 Prometheus track | Trivial | Do alongside G6. |
| Optional UPnP mapping for `listen` on nodes without a public IP (Rust: `igd-next`) | `suwappu-node` config flag, default off | Small | Defer. Phase 5 seeds are AWS with EIPs; revisit only if the external-operator points program admits home-hosted nodes. Never on by default. |
| Desktop operator app | none | Large | Not now. If ever wanted, the reference is `aspectron/kaspa-ng` (Rust, egui, maintained), not KDX. |

## What not to take from it

- No hash-function or PoW ideas. "Blake3v2" is unspecified and unimplemented,
  and invariant 2 (PQ-conservative surface, NIST primitives only) rules out a
  bespoke hash regardless.
- No code. No license, deprecated upstream, dead dependencies.
- The README-before-code pattern. Our own `ROADMAP.md` already flags a
  dangling paper citation; external validators and auditors read READMEs as
  claims, so keep ours matched to what ships.

## Reproduction

```bash
git clone --depth 1 https://github.com/kasperchain/kasper
git clone --depth 1 https://github.com/aspectron/kdx
cd kasper
for f in $(find lib modules -type f); do
  diff --strip-trailing-cr \
    <(sed 's/kasperchain/aspectron/g; s/kasper/kdx/g; s/Kasper/KDX/g' "$f") \
    "../kdx/$f" | grep -c '^[<>]'
done
grep -rli blake3v2 . --exclude=README.md   # no output
curl -s https://registry.npmjs.org/@kasperchain/flow-app   # {"error":"Not found"}
```
