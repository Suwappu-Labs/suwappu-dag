# Mainnet genesis ceremony (M18)

**Status:** Spec — frozen surface for the foundation board +
all 40 Authority Super Node operators (Track C C.5 sale
cohort). Operational execution lands at M18 with the
rehearsal target at M15 (Track B.4 #122). Closes F.1 (#148).

**Audience:** foundation ops, foundation board, all 40
Authority Super Node operators (Tier A sale cohort), public
ceremony attendees + livestream viewers, audit firms
verifying the ceremony's integrity at Track A wave 2
closeout.

**Authoritative inputs:**
- `suwappu-strategy/docs/mainnet-plan.md` Track F §"Genesis ceremony"
- `suwappu-strategy/docs/sources/SUWAPPU-Node-Validator-Sale-Model-v2.0.xlsx`
  Sale Structure (40 Tier A + 200 Tier B)
- `docs/architecture/authority-ring-resilience.md` (E.5,
  PR #174) — Authority Ring quorum-tolerance math
- `crates/suwappu-ltp/src/lib.rs` lines 41–43 —
  LTP_ATTESTATION_QUORUM_THRESHOLD = 7,
  LTP_ATTESTATION_QUORUM_SIZE = 9
- `docs/iq/IQ-001-quorum-formula.md` — 27-of-40 derivation
  (`quorum_threshold(40) = 40 - 13 = 27`)

---

## 1. Two-layer ceremony shape

The ceremony exercises **TWO distinct quorum primitives** in
sequence:

| Layer | Quorum | Participants | What it signs |
|---|---|---|---|
| **Authority Ring quorum** | ≥ 27-of-40 (`quorum_threshold(40) = 27`) | All 40 Tier A Authority Super Nodes from the Track C C.5 sale cohort | The mainnet genesis hash |
| **LTP corridor attestation** | ≥ 7-of-9 (`LTP_ATTESTATION_QUORUM_THRESHOLD`) | 9 super-nodes selected from the 40-member Authority Ring as the LTP corridor super-node set (paper §9 Definition 4 role 2) | The cross-chain genesis LTP attestation (binds the L1 genesis state to the LTP corridor for Ethereum + Solana bridges) |

The two layers are NOT redundant — they exercise different
primitives that the chain depends on:

- The Authority Ring quorum is what every block subsequently
  needs (per `crates/suwappu-consensus/src/commit.rs:61-66`)
- The LTP corridor attestation is what the Ethereum + Solana
  bridges (Track I) verify when they accept cross-chain
  attestations on the destination chains

If either layer fails to reach quorum at the ceremony, the
genesis halts and the foundation invokes the slip-condition
clause (M21 / M24 mainnet target).

---

## 2. Pre-ceremony preparations (T-30 days through T-0)

### T-30 days: cohort lock + manifest preparation

- [ ] All 40 Tier A slot subscriptions are signed + self-stake
  is funded into escrow (per Track C C.5 #128)
- [ ] All 40 buyer key-ceremony attestations on file per
  `docs/validator-custody-requirements.md` (E.2, PR #181) §4.1
- [ ] Foundation board ratifies the genesis manifest
  (`network_id`, `rounds_per_epoch`, `initial_emission_rate`,
  initial validator + authority registries)
- [ ] Manifest published as a foundation-IQ for permanent record

### T-14 days: rehearsal pass

- [ ] All 40 Tier A operators successfully signed at the
  M15 hard-fork dry run (Track B.4 #122 acceptance criterion)
- [ ] Any operator who missed the M15 rehearsal triggers
  foundation-board review before being allowed at the
  live ceremony
- [ ] Public ceremony invitation published; livestream URL
  reserved on multiple platforms (YouTube + X + foundation
  blog)

### T-7 days: ceremony-key generation

Each Tier A operator generates their **ceremony-only signing
keypair** on an air-gapped machine per E.2 §4.1:

- ML-DSA-65 keypair (consensus signing)
- BLS12-381 keypair (LTP attestation signing, super-node
  subset only)
- Self-stake custody multi-sig is **separate** from these
  ceremony keys; the ceremony keys are used ONLY for genesis
  + first-block production then rotated

Operators publish their ceremony pubkeys to the foundation
24h before the ceremony for inclusion in the manifest. The
foundation publishes the FINAL manifest at T-24h with all 40
ceremony pubkeys bound in.

### T-24 hours: super-node corridor selection

The foundation publishes the 9-member LTP corridor super-node
selection from the 40 Authority operators per:

| Criterion | Rule |
|---|---|
| Geographic distribution | ≥ 5 distinct regions among the 9 |
| Sector distribution | No > 30% of corridor in single sector |
| HSM hardware-attestation | All 9 must hold a foundation-verified FIPS 140-3 L3 HSM attestation per E.2 §2.1 |
| Compliance jurisdiction | Mix of US / EU / APAC regulatory anchoring |
| Foundation rotation policy | Initial corridor is "round 1"; rotates every 6 months per LTP corridor governance |

Selection is announced publicly + the affected 9 operators
are notified. Operators NOT in the corridor still sign the
Authority Ring genesis layer; they just don't sign the LTP
corridor attestation.

### T-12 hours: dry-run signature collection

A trial signature run against a **non-canonical test
genesis hash** (`H(b"SUWAPPU-CEREMONY-DRY-RUN-2027")` or similar)
to confirm:

- All 40 operators can produce + transmit an ML-DSA-65
  signature to the foundation within the 15-minute SLA
- The 9 LTP corridor super-nodes can produce + aggregate
  their BLS12-381 signatures within the same window
- Foundation's signature-collection infra (S3 bucket + the
  aggregator script) handles all 40 + 9 simultaneously

If the dry-run finds any issue, the ceremony halts and the
foundation board decides whether to delay or proceed.

### T-1 hour: final state lockout

- Foundation freezes the testnet (no new tx, RPC enters
  read-only mode)
- Testnet final state-tree snapshot archived to
  `s3://suwappu-dag-mainnet-archive/testnet-final-state/` per
  the slip-coverage requirement that pre-mainnet testnet
  history is permanently preserved
- All 40 Tier A operators confirm "ready" via the foundation
  Slack `#ceremony-2027` channel
- Foundation primary on-call confirmed; backup on-call
  confirmed; on-call's on-call confirmed

---

## 3. Ceremony day (T-0)

### Phase 1 — Public opening (T+0 to T+15 min)

- Livestream begins on foundation YouTube + X + blog
- CEO presents the genesis manifest summary (network_id,
  active set, allocation, LTP corridor selection)
- Foundation General Counsel reads the legal-record statement
  (slot subscriptions complete, vesting schedules locked,
  custody attestations on file, audit findings closed)
- Each Tier A operator's CEO or designated representative
  appears on-camera (or via secure pre-recorded video for
  operators in jurisdictions where livestream presence is
  restricted) to confirm participation

### Phase 2 — Authority Ring signing (T+15 min to T+45 min)

Each Tier A operator:

1. Computes the genesis hash locally:
   ```
   H_genesis = SHA3-256("SUWAPPU-DAG-GENESIS-V1" || canonical_manifest_bytes)
   ```
2. Signs `H_genesis` with their ceremony ML-DSA-65 key
3. Uploads the signature + their authority_id to the
   foundation's S3 collection bucket via their pre-issued
   IAM credentials (same path the validator-operator
   onboarding flow uses, per `docs/testnet/VALIDATOR-OPERATORS.md`)
4. Cross-confirms via the `#ceremony-2027` Slack channel
   that the upload succeeded

The foundation aggregator script:

1. Pulls all signatures from S3
2. Verifies each signature against the published
   per-operator ceremony pubkey (manifest binds these)
3. Counts distinct signers; requires ≥ 27
4. If quorum is reached, publishes the bundled signature
   set as `mainnet-genesis-authority-sig-bundle.bincode`
   on the foundation's CDN
5. If quorum is NOT reached at the 30-minute mark, escalates
   to foundation board for the slip-condition decision

### Phase 3 — LTP corridor signing (T+45 min to T+60 min)

The 9 LTP corridor super-nodes additionally:

1. Compute the LTP genesis attestation payload:
   ```rust
   AttestationPayload {
       source_chain: SUWAPPU_DAG_MAINNET_CHAIN_ID,
       target_chain: 0,  // 0 = "genesis-bind", not a specific destination
       source_height: 0, // genesis is height 0
       state_root: initial_state_root_from_manifest,
       timestamp_round: 0,
   }
   ```
   per `crates/suwappu-ltp/src/attestation.rs:84-110`
2. Compute the canonical digest:
   `payload.canonical_digest()` (SHA3-256 with the
   `b"SUWAPPU-LTP-ATTEST-V1"` domain tag, per attestation.rs:101-109)
3. Each of the 9 signs the digest with their BLS12-381 G1
   ceremony key
4. Foundation aggregates the 7-of-9 BLS aggregate signature
   per `crates/suwappu-ltp/src/attestation.rs:182` (`attest`
   function) — quorum is met at 7 distinct signers
5. Publishes the aggregate as
   `mainnet-genesis-ltp-attestation.bincode`

### Phase 4 — Genesis bootstrap (T+60 min to T+120 min)

- Foundation publishes the FINAL canonical genesis package:
  - `genesis.toml` (manifest)
  - `mainnet-genesis-authority-sig-bundle.bincode`
  - `mainnet-genesis-ltp-attestation.bincode`
  - `mainnet-genesis-package.sha256` (defensive hash)
  All four files signed with the foundation's GPG key (the
  signing key separate from each operator's ceremony key)
- Foundation operates the **first 7 seed validators** in
  the 40-member active set; they boot up against the new
  genesis. Round 0 is empty; round 1 starts the chain
- Once `latest_committed_round` advances past round 50
  (~12s at 250ms rounds) on at least 27 of 40 active nodes,
  the chain is **alive**
- Foundation publicly declares "mainnet live" on the
  livestream + status page

### Phase 5 — Public close (T+120 min onwards)

- CEO closes the livestream with the mainnet-live
  declaration
- Faucet announcement (if any genesis distribution is open
  beyond the sale cohort)
- Validator-operator transition: the 33 non-foundation
  Authority operators bring up their nodes over the next
  4 hours (per their internal ops procedures)
- All 200 Tier B Genesis Validators bootstrap against
  mainnet over the following 24 hours

---

## 4. Post-ceremony archival

Within 7 days of the ceremony, the foundation publishes a
permanent archival package at
`https://archive.suwappu.bot/genesis-2027/`:

| Artifact | Purpose |
|---|---|
| Full livestream recording | Public-record provenance |
| Genesis manifest + signature bundle + LTP attestation | Cryptographic reproducibility |
| Per-operator signed participation attestation | Each operator's record of their part |
| Audit-firm observation report (Track A.4 wave-1 audit firm) | Independent verification |
| Foundation board ratification minute | Governance record |
| CEO + General Counsel statements | Legal record |
| `suwappu-strategy/docs/sources/` snapshot at the ceremony date | Sale-model + tokenomics record |
| Press releases + media coverage links | Public record |

These artifacts are preserved indefinitely; the foundation
maintains them via the same archival policy as the
sale-model + Tier-A subscription agreements (mainnet
operational documents).

---

## 5. Distributed key ceremony tooling

The above procedure uses standard tooling; no novel
cryptographic ceremony software is required:

- **Signing**: each operator uses their HSM-backed multi-sig
  setup (per E.2 custody requirements) to produce the
  ML-DSA-65 + BLS12-381 ceremony-key signatures
- **Aggregation**: the foundation aggregator script (Rust,
  single-binary, runs on the foundation's secured
  workstation) consumes the S3-uploaded signatures and
  produces the canonical bundle + LTP attestation
- **Verification**: any third party can verify the
  ceremony's integrity by:
  1. Downloading the manifest + sig bundle + LTP attestation
     + foundation GPG signature
  2. Verifying the GPG signature on the foundation's
     publication
  3. Verifying each per-operator ML-DSA-65 signature
     against the manifest-bound ceremony pubkeys
  4. Verifying the LTP attestation via `suwappu_ltp::verify_attestation`
     (per `crates/suwappu-ltp/src/attestation.rs:182+`)

No trusted setup ceremony (Powers of Tau / circuit-specific
trusted setup) is required for the genesis — the L2 STARK
proving system (SP1 + Plonky3) is FRI-based and needs no
trusted setup per the Track G design.

Operators who want extra cryptographic assurance MAY use
distributed-key-generation (DKG) protocols for their multi-
sig setup (FROST / GG18 / DKLS for ECDSA-shape; lattice-DKG
research is preliminary so v1 doesn't depend on it). Custody
partners (Fireblocks / BitGo / Anchorage per E.2 §5)
implement these natively.

---

## 6. Slip conditions

If the ceremony cannot reach quorum at any phase, the
foundation board invokes the slip-condition clause:

| Trigger | Slip target |
|---|---|
| Authority Ring quorum not reached at Phase 2 within 60 min | Reschedule within 14 days; resume at the same Phase 2 point |
| LTP corridor quorum not reached at Phase 3 | Defer mainnet by 30 days; re-evaluate corridor membership |
| Phase 4 bootstrap fails (chain doesn't advance past round 50 within 60 min of activating 27+ operators) | Hard slip to M21; engineering investigation required |
| Catastrophic security event (key compromise during the ceremony) | Hard slip to M24; full re-attestation pass required |

Each slip is publicly announced with a documented
incident report at `status.suwappu.bot`. The
M18 → M21 → M24 ladder is committed publicly per the
strategic plan; slip beyond M24 requires fresh foundation
board ratification + IQ + sale-model amendment.

---

## 7. Cross-references

- **Authority Ring resilience**: `docs/architecture/authority-ring-resilience.md`
  (E.5, PR #174) — quorum-tolerance math + standby program
  (the 5–10 foundation standby Authority Nodes are available
  to backfill at the ceremony if any sale-cohort operator
  drops out at the last minute)
- **Validator custody**: `docs/validator-custody-requirements.md`
  (E.2, PR #181) — operator key + multi-sig + HSM
  preparations
- **Validator SLA**: `docs/validator-sla-slashing.md` (E.3,
  PR #173) — slashing matrix applies to ceremony-key usage
  identically (an operator equivocating with their ceremony
  key = 100% safety bond forfeiture)
- **Testnet re-genesis runbook**: `docs/operations/testnet-regenesis-runbook.md`
  (B.3, PR #183) — operationally analogous procedure that
  the foundation will have rehearsed multiple times by M18
- **Hard-fork dry run**: Track B.4 (#122) — M15 dry run
  exercises this exact procedure against the testnet, so
  any operational gaps surface 3 months before mainnet
- **Sale model**: `suwappu-strategy/docs/sources/SUWAPPU-Node-Validator-Sale-Model-v2.0.xlsx`
  Sale Structure — names the 40 Tier A slots whose operators
  participate in this ceremony
- **Strategic plan**: `suwappu-strategy/docs/mainnet-plan.md`
  Track F §"Genesis ceremony" — the canonical reference

---

## 8. Change log

| Date | Change | Source |
|---|---|---|
| 2026-05-17 | Initial draft | F.1 (issue #148); two-layer ceremony spec (27-of-40 Authority Ring + 7-of-9 LTP corridor); operational target M18 with rehearsal M15 |
