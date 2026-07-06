# Formal verification — machine-checked obligations

**Status:** Phase 1 landed (Kani harnesses in-tree; CI job non-blocking).
**Governs:** [`IQ-014`](../iq/IQ-014-machine-checked-invariant-proofs.md).
**Date:** 2026-07-06

This document states **precisely** what is machine-checked, under what
bounds, and — as important — what is **not**. It is the artifact the
explorer Capabilities page and the GTM kit link to when they mention
formal verification. **It must never outrun what is actually discharged.**
If you cite "machine-checked" anywhere externally, cite it against this
page, not against a claim from memory.

Prompted by the **BTX Chain** competitor brief
([`../research/briefs/btx-chain.md`](../research/briefs/btx-chain.md) §5.2):
BTX ships a machine-checked shielded-soundness artifact (a Module-SIS
reduction, 21 obligations). Our invariants were backed only by 10,000-case
`proptest` exit gates — strong but *empirical*. Phase 1 closes that gap for
the tractable core with **no model-code gap**.

## What "machine-checked" means here (and what it does not)

- **Kani** ([model-checking.github.io/kani](https://model-checking.github.io/kani/))
  is a **bounded model checker** for Rust: it compiles the *actual*
  functions to a goto-program and proves the asserted properties for **all**
  inputs **up to the stated bounds**. Because it verifies the shipping
  code — not a hand-written model of it — there is **no model-code gap**
  (this is exactly why Phase 1 chose Kani over an abstract proof-assistant
  mechanization; see IQ-014 §Options).
- "Bounded" is the honest caveat: where a harness constructs inputs of a
  fixed size (e.g. a 3-seat stake table), the proof holds for that size and
  the size-agnostic properties it exercises — **not** a symbolic proof over
  all ring sizes. The bounds are stated per-harness below.
- These are **not** unbounded proofs, **not** a proof assistant, and — for
  LTP — **not** a proof of cryptographic hardness (see the (a)/(b) split).

## Running it

```
scripts/verify/run_all.sh        # runs every harness over both crates
```

Requires `cargo-kani`
(`cargo install --locked kani-verifier && cargo kani setup`). CI runs the
same script in the `kani` job of `.github/workflows/ci.yml`, currently
**non-blocking** (`continue-on-error: true`) while the harness set and CI
timings stabilize (IQ-014 Phase-1 plan). The harnesses are gated on
`#[cfg(kani)]` and are excluded from every normal `cargo build` / `clippy`
/ `test`.

## Obligation 1 — joint-quorum AND-gate (Theorem 2 / Invariant 1)

Target: `crates/suwappu-consensus/src/joint.rs`, module `kani_proofs`.
These verify the **Validator-leg** quorum predicates — the pure, stable
arithmetic surface of the AND-gate.

| Harness | Property discharged | Bound |
|---|---|---|
| `quorum_threshold_is_least_strict_supermajority` | `validator_quorum_threshold` is the least integer strictly greater than ⅔ of total stake (paper Definition 2): `3·thr > 2·total` and `3·(thr−1) ≤ 2·total`. | 3 seats; per-seat weight ≤ 2⁴⁰ (so `2·total`, `3·thr` cannot overflow `u128`). |
| `met_quorum_implies_threshold_stake` | No false-positive quorum: `validator_quorum_met ⇒ voting_stake ≥ threshold`, and (composed with non-inflation) `⇒ total ≥ threshold` — the ring cannot ratify a candidate it does not collectively hold the stake for. The safety-critical direction of the Validator leg. | 3 seats, 3 votes, 2 conflicting candidates. |
| `voting_stake_never_exceeds_total` | Tallies cannot exceed the ring: `voting_stake ≤ total` for any (possibly duplicate) vote multiset. | 3 seats, 3 votes. |
| `duplicate_vote_does_not_inflate` | The dedup property: appending a copy of a present vote leaves `voting_stake` unchanged. | 1 duplicated vote. |
| `double_vote_stake_is_bounded` | The Theorem-2 Validator-equivocation quantity is well-formed: `validator_double_vote_stake ≤ total`. | 3 seats, 3 votes. |

**Not yet discharged on this surface (scoped follow-ups, IQ-014 OQ3):**

- The **Authority leg** (`commit_leader` over the `DagStore`). It needs a
  symbolic-DAG harness, a materially larger effort; it remains covered by
  the DAG-S4 commit proptests (`tests/proptest_mysticeti_commit.rs`, 10k)
  and DAG-S5 joint-quorum proptests (`tests/proptest_joint_quorum.rs`, 10k).
  The quorum arithmetic above is the *stable* surface; the signature/DAG
  representation is still in flux (IQ-009 / IQ-012), so verifying it first
  is deliberate.
- The full `joint_commit` composition end-to-end (both legs together)
  follows once the Authority-leg harness lands.

## Obligation 2 — LTP 7-of-9 attestation (Invariant 3)

Target: `crates/suwappu-ltp/src/attestation.rs`, module `kani_proofs`.

**This is scoped to obligation (a) only.** LTP soundness factors into two
very different obligations, and conflating them is the classic
formal-methods overclaim:

- **(a) Protocol-logic obligation** — *given* an unforgeable signature
  primitive, the 7-of-9 quorum/threshold logic admits no attestation below
  threshold. **This is what is machine-checked below.**
- **(b) Cryptographic-hardness obligation** — the signature primitive
  itself is unforgeable. For LTP this is currently classical **BLS12-381**,
  which **IQ-009 is actively migrating** to an O(1)-in-signers PQ aggregate.
  We **cite** BLS unforgeability from its standard literature and do **not**
  re-prove it: a hardness reduction against a primitive we are removing
  would be effort spent verifying the wrong object. When the PQ aggregate
  lands (IQ-009), obligation (b) is re-opened against the successor.

| Harness | Property discharged | Bound |
|---|---|---|
| `quorum_reached_matches_threshold` | The predicate both `attest` and `verify_attestation` gate on admits a signer count iff `≥ 7`, and never below — verified against the real `quorum_reached`. | distinct signers ≤ 9 (corridor size). |
| `threshold_is_bft_supermajority` | 7-of-9 is a genuine Byzantine supermajority — strictly greater than two-thirds (`3·7 > 2·9`, the actual BFT bound, not merely `>½`) — and tolerates `f = 9−7 = 2` faulty witnesses with `9 ≥ 3f+1`. | constants only. |

To keep the verified predicate identical to the enforced one, the inline
`< LTP_ATTESTATION_QUORUM_THRESHOLD` checks in `attest` /
`verify_attestation` were refactored into the single `quorum_reached`
function the harness targets — a behaviour-preserving extraction.

**Not discharged on this surface (by design):** the BLS aggregate's
unforgeability (obligation (b), above); the `BTreeSet`-based
distinct-signer dedup inside `attest` (enforced by `BTreeSet` insert
semantics + the DAG-S15 proptests — `tests/proptest_attestation.rs`, 10k);
signature-byte and payload-tamper rejection (proptest + the
`tampered_payload_breaks_verification` unit test).

## The honest one-line summary

The **quorum and threshold arithmetic** of both load-bearing invariants is
now **machine-checked (bounded) against the shipping code**, with no
model-code gap. The **cryptographic-hardness** layer is **cited, not
proven** (and deliberately so, pending the IQ-009 PQ migration), and the
**Authority-leg DAG** logic remains **proptest-backed** pending a
symbolic-DAG harness. Phase 2 (an unbounded abstract Theorem-2
mechanization in a proof assistant, published only with an explicit
code-refinement caveat) is a separately-resourced follow-on, not yet
started.
