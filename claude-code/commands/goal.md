---
description: "Standing goal: get the SUWAPPU chain + LTP bridge live on a public testnet outsiders can join, and SuwappuBot settling over it. Shows the launch backlog and how to verify each item. Usage: /goal [item]"
---

# /goal — Public testnet, external validators, bot settling over it

Standing goal, in the user's words: **"SuwappuBot will eventually use the
Suwappu lattice bridge and chain, but we need it on testnet and others
able to join."**

Three repos move together:
`suwappu-dag` (the chain) → `suwappu-lattice-protocol` (the LTP bridge) →
`suwappubot` (the consumer, gated off until the bridge is real).

Done means all four of these are true at once:
1. A public testnet is live and reachable (RPC + faucet + explorer).
2. A third party who is **not** the foundation can run a validator, sync,
   and get seated — using only published artifacts (no private source).
3. The LTP bridge is re-legged onto that chain and an end-to-end transfer
   has been observed.
4. SuwappuBot's `lattice_bridge_enabled` is on, with all seven gates in
   `suwappubot/docs/pq-settlement-profile.md` passed.

## How to use

- `/goal` — re-verify the next unchecked item against current code (this
  list is point-in-time; things may have landed since), do it, check it
  off, and commit the edit to this file.
- `/goal <item>` — jump to that item.
- Found a new launch blocker? ADD it here.
- Keep `docs/testnet/LAUNCH-STATUS.md` and this file consistent — that doc
  is the long-form tracker, this is the actionable queue.

## A. Chain: code gaps that block a *credible* testnet

- [x] A1. Late join — `allow_post_genesis_join` + `GetTip`/`GetCertsByRound`/`GetBlock` backfill.
- [x] A2. Peer discovery — seeds accept up to 64 dynamic inbound peers full-duplex.
- [x] A3. Dual-signature governance at ingress (client wire v3).
- [x] A4. Block-level governance intent auth — on-chain `GovAuth` envelope, bound into `payload_digest`, re-verified at the epoch boundary (IQ-007).
- [x] A5. IQ-007 automated guard — `phase_g_growing_prefix_under_transient_unavailability` (no commit retraction + cross-node settlement agreement). NOTE: `blocks_by_round` is a lossy last-writer-wins index, **not** the finalize order — do not assert it append-only (see the test's doc comment).
- [ ] A6. **No persistence — the DAG store never prunes.** A seed's RAM grows without bound and a restart loses all history. Today's mitigation is periodic regenesis, which is not credible for a testnet others join. Implement snapshot/checkpoint persistence, or at minimum bounded pruning + a documented regenesis cadence. This is the single biggest "it will fall over" risk.
- [ ] A7. **Snapshot/checkpoint sync.** A joiner can only catch up as far back as its peers have held in memory (A6). Until snapshot sync exists, a late joiner cannot reconstruct full history.
- [ ] A8. **Validator Ring has no independent join path.** `AdmitAuthority` mirrors the same identity into BOTH registries, so the paper's open PoS Validator Ring does not exist yet. Either implement it, or present the testnet honestly as single-ring PoA and reconcile `docs/testnet/VALIDATOR-OPERATORS.md`. Do not ship docs claiming dual-ring if the code is single-ring.
- [ ] A9. **Stake is a claimed integer, not an escrowed bond** (IQ-007 residual). Two colluding seated authorities can admit with arbitrary declared stake. Bonding is a separate change; until then the slashing story is nominal.
- [ ] A10. **Throughput go/no-go.** `.sprint-state.md` records ~0.125 TPS p50 against a 1–5k target (S31 partially landed). An incentivized points-per-cert program at that rate is self-defeating — finish S31 or launch un-incentivized and say so.

## B. Chain: verification debt

- [ ] B1. **IQ-007 still needs human sign-off.** The doc says it MUST NOT be treated as production-ready without consensus-team review AND a loaded multi-node devnet run with adversarial fault injection (block-withholding / stripped-block relay / straggler-cert ordering). A5's test and CI are necessary, not sufficient. This is a human gate — flag it, do not self-approve.
- [ ] B2. Run the fault-injection devnet from B1 and record the result in `docs/iq/IQ-007-*.md`.

## C. Ship the artifacts outsiders need

- [x] C1. GHCR image publishing (`docker.yml`) — **was broken from the day it was added** (cargo-chef vs `edition2024` on a pinned `rust:1.78`, then cargo-chef choking on the workspace-excluded `zkvm/*` path deps). Fixed by bumping to `rust:1.90` and dropping cargo-chef; first successful publish 2026-08-15.
- [x] C2. `aarch64-unknown-linux-gnu` release binary (cross-compiled on the free x86 runner) — the ARM path for Oracle A1.
- [x] C3. `suwappu-keygen` + `suwappu-validator-program` packaged in releases.
- [x] C4a. `release.yml` verified buildable. It had NEVER been run (zero tags), and its x86_64 leg was **broken**: the `musl` target fails on PQClean's glibc-only `__GNUC_PREREQ` macro, and behind that on `openssl-sys` needing a musl-linked OpenSSL. Cutting a tag would have produced **zero artifacts** (the `release` job `needs: build`). Both Linux legs now target glibc and are verified locally to produce all 7 binaries with the correct arch; packaging now fails loudly instead of shipping a half-empty tarball.
- [ ] C4. **Cut the first release tag (`suwappu-dag-v0.1.0`).** Nothing above is actually downloadable until a tag exists — `git tag` is still empty. Now unblocked by C4a. Outward-facing and effectively permanent, so it wants a human decision on version/timing. Note the macOS legs remain unverified (no macOS runner available to test locally) — they are not on the testnet critical path, and `fail-fast: false` means a macOS failure still blocks the `release` job, so watch the first run.
- [ ] C5. Decide `suwappu-db` visibility. While it stays private, external operators can ONLY use the published image/binary — they cannot build from source. Either accept that permanently and document it, or open the repo.
- [ ] C6. **Enable GitHub Pages** (Settings → Pages → Source: GitHub Actions). **Human/admin only — cannot be done from the repo.** `docs.yml`'s `deploy to Pages` job fails every run on `main` with `Failed to create deployment (status: 404) … Ensure GitHub Pages has been enabled`. Two consequences: the API-reference site never publishes, and the free-tier plan in `NON-AWS-DEPLOY.md` (which hosts `genesis.toml`, `peers.txt`, explorer and status page on Pages) has no host until this is flipped. The rustdoc half of that workflow is green as of 2026-08-15.

## D. Stand the network up (needs a human — no AWS)

AWS (profile `gsn`) is gone. The free-tier path is written up in
[`docs/testnet/NON-AWS-DEPLOY.md`](../../docs/testnet/NON-AWS-DEPLOY.md);
`terraform/` is retained only as a record of the old design.

- [ ] D1. Provision the host(s) — Oracle Cloud "Always Free" Ampere A1 (ARM64). Read the runbook's "Honest caveats" first: one box is NOT fault-independent, and A6 above means RAM grows.
- [ ] D2. DNS zone + TLS (Cloudflare free tier, or Caddy on the box). The raw p2p port cannot go through an HTTP proxy.
- [ ] D3. Genesis ceremony with REAL keys (`suwappu-keygen` on PATH — placeholder keys cannot sign). Publish `genesis.toml` + `peers.txt`.
- [ ] D4. Verify the mesh: seeds, faucet, explorer, status page.
- [ ] D5. **Prove the join path with a real outsider.** Have someone outside the foundation run a node from published artifacts only and get seated. Until this happens, "others able to join" is a claim, not a fact — this is the acceptance test for the whole goal.

## E. Bridge re-legging (suwappu-lattice-protocol)

- [ ] E1. **Corridor daemon does not exist.** `src/ltp/corridor/` is a byte-parity library — no membership registry, no PoP exchange, no partial-signature transport for the 7-of-9 super-node quorum. `Relayer.relay()` returns an in-process object; there is no relayer transport. "Joining a corridor" is currently a human arrangement. This is the bridge's A8.
- [ ] E2. Deploy `LTPAnchorRegistry` (+ bridge pair) on the new chain once D4 passes.
- [ ] E3. Regenerate + register the gateway keypair (2-of-2 multisig ceremony), fund operators.
- [ ] E4. Run `scripts/bridge_live.py` end-to-end and capture the transcript.
- [ ] E5. Until E2–E4 land, the bridge trust model is 2-of-2 discretionary with ZERO bonds. Fine for a demo, not for value-bearing settlement — `BRIDGE_TRUST_MODEL.md` says the same. Do not let marketing outrun this line.

## F. SuwappuBot activation (suwappubot)

- [ ] F1. Keep `lattice_bridge_enabled=false` until every gate in `docs/pq-settlement-profile.md` passes. A quote-only, default-off provider may merge dark; activation may not.
- [ ] F2. The final gate is an OBSERVED end-to-end testnet transfer — not CI green, not a successful deploy. Per that repo's standing rules: if a live test is blocked, say "code-complete, not functionally verified — needs X."

## Rules

- **Re-verify before acting.** This is a point-in-time list; check the code.
- **Do not let docs outrun the code.** Several items here (A8, A9, E5) exist because a doc claimed a property the implementation does not have. If you cannot ship the property, fix the doc instead — and say so.
- **Consensus-touching changes** (`suwappu-consensus`, joint-quorum, commit ordering, `daemon.rs` commit path) require `consensus-reviewer` + human sign-off. CI green is not sufficient for this class — see B1.
- **`make contracts-secaudit` green** before suggesting any contract change in the lattice repo.
- Iteration loop is the CI cycle, not local cargo (see CLAUDE.md §Local development). Never run workspace-wide cargo locally.
- No `git rebase`. No `Co-Authored-By`.
