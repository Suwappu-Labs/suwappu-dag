# IQ-007 — Governance authorization as a consensus rule

**Status:** Recommendation, implemented on branch
`claude/lattice-chain-quality-parity-x78o7d`; pending specialist
(consensus-reviewer + crypto-reviewer) sign-off before merge.
**Owner:** consensus
**Date:** 2026-08-15
**Tracking:** external-validator-join effort; supersedes the
Issue #28 "dual-signature deferred" note in `crates/suwappu-node/src/client.rs`.

## Question

How should authority-management intents (`AdmitAuthority`,
`ExitAuthority`, `EjectAuthority`) be authorized so that no single party
can reshape the validator set — and so that the authorization is
enforced by consensus, not merely by the ingress API?

## Background

Before this change, governance intents were authorized only at ingress:
the client wire (`SubmitGoverned`) and JSON-RPC verified a signature and
then discarded it. `Intent` carries no signatures, so once an intent was
in a committed block, `apply_governance_intent` mutated the Authority and
Validator registries at the epoch boundary with **no authorization
re-check**.

An adversarial consensus review of the initial dual-signature work
identified two independent single-party paths to controlling both rings —
the exact failure class Load-bearing Invariant 1 (joint-quorum AND-gate,
Paper Theorem 2) exists to exclude:

1. **Block-author bypass.** A Byzantine *seated* authority skips the
   client wire entirely: it authors its own validly-signed cert whose
   block contains an un-cosigned `AdmitAuthority`. Honest nodes accept
   the cert (author is seated, signature valid), the leader-commit sweeps
   it into causal history, and every honest node admits the attacker's
   candidate with attacker-chosen stake. The ingress gate never runs.

2. **Candidate self-co-sign.** Even at the gate, the original
   `AdmitAuthority` co-signer was the *candidate* — a key the attacker
   generates. One compromised seated key could sponsor-sign an admit for
   its own candidate key and co-sign it as that candidate: fully
   gate-compliant, single party.

## Decision

Governance authorization becomes a **commit-time consensus rule** carried
on-chain, with a two-distinct-authority requirement for every governance
action.

### Envelope (`GovAuth`, `crates/suwappu-node/src/client.rs`)

Each governance intent carries an authorization envelope:

- `sponsor` — a seated Authority Ring member; ML-DSA-65 signature over
  `intent_signing_digest(network_id, intent)`.
- `co_signer` — a **second, distinct** seated Authority Ring member;
  signature over the same digest. Required for `AdmitAuthority` too, not
  only Exit/Eject.
- `candidate_pop` — `AdmitAuthority` only: the admitted key signs the
  same digest (proof of possession, so an admit cannot be forged for a
  key nobody holds). This is *in addition to*, not a substitute for, the
  second authority.

Both authority signatures bind the same digest, so the domain-separation
(`SUWAPPU_INTENT_V1`) and `network_id` replay defenses apply to each.

### On-chain carriage and re-verification

- The envelope travels inside `BlockPayload.governance_auth`, keyed by
  the governance intent's index within `intents`. The authoring node
  attaches the envelope it verified at ingest (retained in a bounded
  `State` map, consumed at block build).
- `apply_governance_intent` re-verifies the envelope against **this
  node's** seated Authority Ring and the manifest `network_id` before any
  registry mutation, and drops the intent if the envelope is missing or
  invalid.

### Determinism argument

Every honest node reaches a given epoch boundary having committed the
same blocks, so it holds the same Authority Registry and the same
manifest `network_id`. `verify_governed_intent` is a pure function of
(intent, envelope, registry, network_id). Therefore all honest nodes make
the identical apply/drop decision for each queued governance intent, in
the identical drained order — no state divergence. A Byzantine block
author gains nothing by forging or omitting the envelope: the envelope is
*verified, not trusted*, and a failure is a deterministic drop everywhere.

## Consequences

- **Closed:** both single-party paths above. Reshaping the validator set
  now requires two distinct seated Authority Ring members, enforced at
  commit. A leaked client signing key alone, or a single Byzantine
  authoring node alone, is insufficient.
- **Wire:** `CLIENT_WIRE_VERSION = 3`. `BlockPayload` gains a
  `#[serde(default)]` `governance_auth` field (older blocks decode with
  an empty vector — and any governance intent in such a block is dropped,
  which is the safe direction).
- **Not addressed here (future work):**
  - Stake in `AdmitAuthority` is still a claimed integer, not an escrowed
    bond. A two-authority collusion can still admit with arbitrary
    declared stake; bonding is a separate change.
  - BLS material in the admit envelope is still discarded by the daemon
    (as at genesis); BLS aggregation of the co-signatures is a possible
    future compaction.
  - The Validator Ring has no independent join path
    (`AdmitAuthority` mirrors into both registries).

## Alternatives considered

- **Embed the signatures in the `Intent` enum.** Cleaner conceptually
  (auth intrinsic to the intent) but ripples through ~40 construction
  sites and both apply paths (daemon registry + substrate stake records)
  across crates; higher risk for no security gain over the
  `BlockPayload` envelope.
- **Keep it an ingress-only gate.** Rejected: does not bind a Byzantine
  block author, i.e. does not make the guarantee a consensus property.
- **Require a full quorum co-sign.** Stronger, but a two-authority
  threshold already defeats the single-party attack and keeps the
  operational path (foundation + one other seat) tractable for the
  testnet; the threshold can be raised later without a wire change.
