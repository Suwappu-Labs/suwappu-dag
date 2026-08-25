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
- Boxes: `[x]` done, `[ ]` not started or blocked, `[~]` partly landed —
  read the note before assuming either that there is nothing to do or that
  you are starting from scratch.

## A. Chain: code gaps that block a *credible* testnet

- [x] A1. Late join — `allow_post_genesis_join` + `GetTip`/`GetCertsByRound`/`GetBlock` backfill.
- [x] A2. Peer discovery — seeds accept up to 64 dynamic inbound peers full-duplex.
- [x] A3. Dual-signature governance at ingress (client wire v3).
- [x] A4. Block-level governance intent auth — on-chain `GovAuth` envelope, bound into `payload_digest`, re-verified at the epoch boundary (IQ-007).
- [x] A5. IQ-007 automated guard — `phase_g_growing_prefix_under_transient_unavailability` (no commit retraction + cross-node settlement agreement). NOTE: `blocks_by_round` is a lossy last-writer-wins index, **not** the finalize order — do not assert it append-only (see the test's doc comment).
- [ ] A6. **No persistence — the DAG store never prunes.** A seed's RAM grows without bound and a restart loses all history. Today's mitigation is periodic regenesis, which is not credible for a testnet others join. Implement snapshot/checkpoint persistence, or at minimum bounded pruning + a documented regenesis cadence. This is the single biggest "it will fall over" risk.
- [ ] A7. **Snapshot/checkpoint sync.** A joiner can only catch up as far back as its peers have held in memory (A6). Until snapshot sync exists, a late joiner cannot reconstruct full history.
- [ ] A8. **Validator Ring has no independent join path.** `AdmitAuthority` mirrors the same identity into BOTH registries, so the paper's open PoS Validator Ring does not exist yet. Remaining work here is **code only**: implement the ring, or make the single-ring PoA framing explicit in the paper-facing docs. (Doc half checked 2026-08-15: `VALIDATOR-OPERATORS.md` only ever claims the *Authority* Ring, which is what the code actually does, so it was not lying about the rings.)
- [x] A8b. Operator-guide honesty pass. `VALIDATOR-OPERATORS.md` is the front door for exactly the outsiders D5 is about, and it promised a program that does not exist. Now carries a status box (no network, no release, no apply form/Discord/leaderboard, dead URLs, retired AWS topology) and corrects: the points→token conversion (stated as settled fact → marked a proposal, plus an explicit no-offer disclaimer); a "state grows ~10 GB/month" disk spec that hid the real unbounded-**RAM** risk from A6; an AWS IAM/S3 event-log step that cannot exist; `*linux-musl*` download commands (also fixed in `RELEASING.md` + `OPERATIONS.md`, the latter referencing an aarch64-musl artifact the workflow never produced); build-from-source keygen instructions outsiders cannot follow (C5); and a "100% slash" line that overstated A9.
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

Read the dates before acting on this section. E2–E4 are worded "on the new
chain", and there is no new chain yet (all of D is open). What actually
happened in 2026-08 is that the bridge was re-legged onto **existing public
testnets** — Ethereum Sepolia and Tempo — because the SUWAPPU leg died with
the AWS teardown. That work is real and is recorded in
`docs/DEPLOYED_CONTRACTS.md` in the lattice repo. The boxes stay unchecked
because their stated condition (D4) is still unmet, **not** because the work
is undone. Do not redeploy what is already deployed — a duplicate registry
would orphan the addresses in that file, and changing a deployed address there
requires an upgrade plan under `plans/`.

- [~] E1. **Corridor service layer now exists; the daemon still does not.**
  Was: "no membership registry, no PoP exchange, no partial-signature
  transport." All three landed 2026-08-25 in `src/ltp/corridor/`:
  `membership.py` (`CorridorRegistry` — PoP verified at enrollment, rejects
  duplicate authority ids *and* duplicate BLS keys, deterministic ordering,
  comparable `roster_digest()`), `enrollment.py` (`EnrollmentAnnouncement` —
  the bare PoP signs the public key alone, so an observed PoP could be
  rebroadcast under any seat/corridor/epoch to squat a seat and lock the real
  operator out; the binding signature covers all three), and `session.py`
  (`SigningSession` partial collection to 7-of-9, `CorridorSigner` local
  double-sign guard, `EquivocationMonitor`/`EquivocationEvidence`).
  `bridge/wire.py` closes the relayer half: `RelayPacket` had no serialization
  at all, so `Relayer.relay()` and `L2Materializer.materialize()` could only
  run in one process.
  Seat entitlement is now decided too (`policy.py`, 2026-08-25): the binding
  stops a *replay*, but not a stranger announcing for your seat with their own
  freshly generated key — that announcement is genuinely signed and verifies.
  `enroll_announcement` is fail-closed and requires a policy; `SeatAllowlist`
  binds a seat to a specific published key. Stake and governance policies were
  ruled out as unimplementable rather than unwanted: there is no escrowed bond
  to check a claim against (this repo's own A9) and no corridor governance
  surface, so `EnrollmentPolicy` is a Protocol and they land later as
  implementations.
  And it is runnable: `scripts/corridor_ceremony.py` covers the whole ceremony
  — keygen, allowlist, announce, roster, payload, sign, aggregate, verify — as
  file-based commands, no network, no new dependencies. That is what an
  external corridor operator would actually follow, so it is the E-track
  counterpart to D5.
  **Still open**, and why this is `[~]` not `[x]`: there is still no daemon.
  No sockets, no process, no supervision, no peer discovery — the ceremony is
  operator-driven, one command at a time. That is a deliberate stopping point
  (a daemon needs a transport decision the corridor has not made) rather than
  an oversight, but it means a corridor cannot yet run unattended.
- [ ] E2. Deploy `LTPAnchorRegistry` (+ bridge pair) on the new chain once D4
  passes. **Already done on the interim legs** (2026-08-24): Ethereum Sepolia
  `11155111` and Tempo testnet `42431`, both registry v6 behind a UUPS proxy
  with LTPMultiSig (2-of-2) + TimelockController (60s). Base Sepolia `84532`
  remains live from before. Governance path is multisig submit → confirm →
  execute (schedules the timelock) → wait → **a second multisig round** to
  execute, because the multisig is the timelock's only executor — budget for
  that, it is not a single round.
- [ ] E3. Regenerate + register the gateway keypair (2-of-2 multisig ceremony),
  fund operators. **Done for the interim legs**: a signable bridge-operator
  key was generated 2026-08-24 (vk hash `0x47f8caa7…`, testnet-only) because
  the original operator key's secret is not in the repo and therefore cannot
  sign. Deployer and second multisig owner are funded on both legs.
- [ ] E4. Run `scripts/bridge_live.py` end-to-end and capture the transcript.
  **Done on Ethereum Sepolia + Base Sepolia** (cross-chain anchor pair) and on
  Tempo (additionally carrying an `entityIdHash` in a transfer memo). Redo on
  the new chain once D4 passes. Note `BRIDGE_TX_TIMEOUT` (default 600s) — the
  old hardcoded 180s reported `TimeExhausted` on anchors that had in fact
  succeeded.
- [ ] E5. Until E2–E4 land **on the real chain**, the bridge trust model is
  2-of-2 discretionary with ZERO bonds. Fine for a demo, not for value-bearing
  settlement — `BRIDGE_TRUST_MODEL.md` says the same. The interim-leg
  deployments above do not change this: they are testnet, discretionary, and
  unbonded. Do not let marketing outrun this line.

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
