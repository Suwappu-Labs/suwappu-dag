# IQ-014 — Machine-checked obligations for Theorem 2 + LTP soundness

**Status:** Recommendation, pending sign-off. No implementation
scheduled; this IQ fixes a *target shape and scope* for adding
machine-checked formal-verification artifacts to the two load-bearing
invariants that carry the most weight with an institutional/formal buyer.
**Owner:** consensus / crypto
**Date:** 2026-07-06
**Tracking:** Prompted by the **BTX Chain** competitor brief
([`../research/briefs/btx-chain.md`](../research/briefs/btx-chain.md)) and
threaded into the parity matrix F3 row
([`../research/feature-parity-matrix.md`](../research/feature-parity-matrix.md)).
Refs [`IQ-009`](./IQ-009-ltp-aggregate-pq-migration.md) (the LTP aggregate
is mid-migration — a soundness proof must not lock in classical BLS) and
[`IQ-012`](./IQ-012-threshold-mldsa-checkpoint-cosignature.md) (Theorem-2
accountability boundary).

## Question

**Should suwappu-dag add machine-checked formal-verification artifacts —
not just property tests — for its two load-bearing invariants (the
joint-quorum AND-gate, Theorem 2 / Invariant 1; and LTP attestation value
soundness, Invariant 3's quorum-unforgeability core), and if so with what
tool, at what scope, and against which surface (the Rust code, or an
abstract model)?**

Two things make this worth a written decision rather than a ticket:

1. **It is a credibility question with a concrete competitive prompt, not
   a correctness gap.** Our invariants are already backed by 10,000-case
   `proptest` exit gates (CLAUDE.md sprint table). Those are strong but
   **empirical** — they sample the input space, they do not *prove* over
   it. **BTX Chain ships a machine-checked artifact** for its shielded
   value soundness: "a reduction of forgery hardness to Module-SIS — with
   **21 machine-checked obligations**," runnable via
   `python3 formal-verification/run_all.py` (BTX README). For the
   regulated-settlement / formal-methods buyer this is a category the
   field is starting to compete in, and "we proptest it" answers a weaker
   question than "we discharge a machine-checked obligation for it." This
   IQ decides whether — and how narrowly — we meet that bar.

2. **The obvious approach (mechanize the paper proof in a proof assistant)
   has a model-code gap that can quietly make the artifact *worse than
   honest*.** A Lean/Coq proof of an abstract Theorem-2 statement proves
   the *math*, not that `joint_commit` implements the math. If we publish
   "Theorem 2 is machine-checked" while the checked object is a hand-model
   that may drift from `joint.rs`, we overclaim in exactly the way this
   repo's honesty discipline forbids. The scope decision (verify the
   *code* vs verify a *model*, and how to bind them) is the crux.

## Evidence — what is proven today, and where the real obligations live

### The invariants and their current backing

- **Invariant 1 — joint-quorum AND-gate (Theorem 2).** Live in
  [`crates/suwappu-consensus/src/joint.rs`](../../crates/suwappu-consensus/src/joint.rs).
  The safety-critical objects are small and *already pure functions over
  integers/sets*, which is what makes them tractable to verify:
  - `validator_quorum_threshold(stake_table)` — "strictly greater than
    two-thirds of total stake" (`joint.rs:116-122`, paper Definition 2).
  - `voting_stake` / `validator_quorum_met` — dedup-by-id stake
    aggregation (`joint.rs:124-141`).
  - `joint_commit` — returns `Some(hash)` iff the Authority `commit_leader`
    ratifies a candidate *and* the Validator Ring stake quorum votes for
    that same candidate (`joint.rs:142-160`). This is the AND-gate.
  - `authority_equivocators` (`joint.rs:162-190`) and
    `validator_double_vote_stake` (`joint.rs:196-216`) — the
    accountability side: the *named* signer sets a safety violation would
    have to force.
  - Exit gate: `tests/proptest_joint_quorum.rs`, 4 properties × 10k
    (DAG-S5). Empirical.
- **Invariant 3 — LTP attestation (constant-size, and beneath it,
  quorum-unforgeable).** Live in
  [`crates/suwappu-ltp/src/attestation.rs`](../../crates/suwappu-ltp/src/attestation.rs).
  The *soundness* core (distinct from the byte-count property IQ-009
  governs) is: a `CorridorAttestation` verifies only if
  ≥ `LTP_ATTESTATION_QUORUM_THRESHOLD` (7) of the 9 distinct super-nodes
  signed `payload.canonical_digest()` (SHA3-256, `attestation.rs:99-101`),
  and the aggregate cannot be forged below that threshold. Exit gate:
  `tests/proptest_attestation.rs`, 4 × 10k (DAG-S15). Empirical.

### The honest decomposition of "prove soundness"

An LTP soundness statement factors into two very different obligations,
and conflating them is the classic formal-methods overclaim:

- **(a) The protocol-logic obligation** — *given* an unforgeable
  signature primitive, the 7-of-9 quorum/threshold/dedup/aggregation logic
  admits no attestation without ≥7 distinct honest signers. This is a
  finite, discharge-able statement over the *code's* set/threshold logic.
  **This is the tractable, high-value target.**
- **(b) The cryptographic-hardness obligation** — the signature primitive
  itself is unforgeable (BTX's "reduction to Module-SIS" is exactly this
  layer, for their lattice CT). For us this currently bottoms out in
  **classical BLS12-381** (`suwappu-crypto/src/bls.rs`), which **IQ-009 is
  actively migrating** to an O(1)-in-signers PQ aggregate. Formalizing a
  hardness reduction against BLS would (i) duplicate the enormous existing
  literature and (ii) **verify a primitive we are removing.** Obligation
  (b) should be *cited*, not re-proven, and should target the PQ successor
  IQ-009 lands, not the classical incumbent.

This decomposition is the single most important output of the IQ:
**verify (a) on our own code; cite (b) from the primitive's literature;
never publish a claim that blurs the two.**

## Options surveyed

For each: (i) what object is actually verified; (ii) the model-code gap;
(iii) cost / tooling; (iv) how strong the resulting claim honestly is.

### Option A — Kani (Rust → CBMC bounded model checking) on the pure predicates (RECOMMENDED first artifact)

Annotate the pure quorum/threshold functions in `joint.rs` and
`attestation.rs` with `#[kani::proof]` harnesses and let Kani (the
Rust-native bounded model checker) discharge them **against the real
compiled functions**.

- **What is verified.** The *actual* `validator_quorum_met`,
  `voting_stake` (dedup correctness), `joint_commit` AND-gate, and the
  LTP 7-of-9 threshold/dedup logic — the real functions, no reimplementation.
  Concretely: "no vote multiset with < quorum distinct-signer stake makes
  `validator_quorum_met` return true"; "`joint_commit` returns `Some` only
  when *both* rings ratify the same hash"; "no signer multiset with < 7
  distinct ids passes the attestation threshold check."
- **Model-code gap.** **Minimal — this is the point.** Kani verifies the
  MIR of the actual code, so there is no separate model to drift. The
  residual gap is only Kani's *bounds* (it is bounded model checking:
  proofs hold up to a stake-table size / vote-count bound), which must be
  stated honestly in the artifact.
- **Cost / tooling.** Lowest. `cargo kani`, harnesses live next to the
  code, runs in CI beside the proptests. Bounded, so it does not need a
  proof-engineer skillset the way a proof assistant does.
- **Claim strength.** "The quorum and threshold logic is machine-checked
  (bounded) against the shipping code." Honest, verifiable, and directly
  comparable to BTX's runnable `run_all.py`. Weaker than an unbounded
  proof, but with **no model-code gap** — the more dangerous weakness to
  have.

### Option B — Proof assistant (Lean 4 / Coq / Isabelle): mechanize the paper Theorem 2 abstractly (headline artifact, higher cost)

Mechanize the paper §11 Theorem-2 statement — "a safety violation forces
≥⌈|A|/3⌉ named Authority equivocators **and** > total/3 named Validator
double-voters simultaneously" — as an unbounded theorem over abstract
rings, then argue the Rust `joint_commit` / `authority_equivocators` /
`validator_double_vote_stake` refine it.

- **What is verified.** The *mathematics* of the AND-gate, unbounded (all
  ring sizes). This is the strongest possible statement of the safety
  argument and the natural companion to the academic paper.
- **Model-code gap.** **Real and load-bearing.** The theorem is about a
  hand-written model; binding it to `joint.rs` is a separate,
  usually-informal refinement argument. Without that binding the artifact
  proves the paper, not the product — which must be stated, or it
  overclaims.
- **Cost / tooling.** High. Needs a proof-assistant skill set and weeks,
  not days. This is a "publish alongside the paper" investment, not a
  sprint task.
- **Claim strength.** "Theorem 2 is machine-checked (unbounded, abstract
  model; code refinement argued separately)." Strongest on the math,
  honest only if the refinement caveat is loud.

### Option C — SMT-backed deductive verification of the real code (Verus / Creusot / Prusti)

Write pre/post-condition contracts on the actual Rust functions and
discharge them with an SMT backend — unbounded over the logic (unlike
Kani), on the real code (unlike Option B).

- **What is verified.** The real functions, unbounded, against rich specs
  ("`joint_commit` returns `Some(h)` ⇒ both rings ratified `h`").
- **Model-code gap.** Minimal (contracts on real code), like Kani, but
  without the bound.
- **Cost / tooling.** Medium-high and **maturity-risky**: these tools
  constrain the Rust you can write (Verus is a dialect; Creusot/Prusti
  have coverage gaps), and retrofitting contracts onto existing code
  often forces refactors. Best-in-class *if* it takes; a real chance it
  fights the codebase.
- **Claim strength.** Strongest honest option — unbounded *and* on the
  code — when it works.

### Option D — Status quo: 10k-case proptests only

Change nothing; keep the empirical exit gates.

- **What is verified.** Nothing, in the formal sense — high-confidence
  sampling. **This remains correct and is not a safety gap.**
- **Claim strength.** "Property-tested at 10k cases." Honest, but answers
  a weaker question than the buyer (and BTX) now pose.

## Recommendation

**A phased pair, pending sign-off — Option A first, Option B as the
headline follow-on — with the LTP hardness layer explicitly out of scope
(cited, per IQ-009), and Option C parked as a stretch.**

1. **Phase 1 (recommended to schedule): Option A — Kani harnesses on the
   pure predicates**, run in CI beside the proptests, with a `run_all`
   entry point mirroring BTX's ergonomics. This is the cheapest artifact
   with the *smallest model-code gap*, it verifies the shipping code, and
   it converts "we proptest the AND-gate" into "the AND-gate and the 7-of-9
   threshold logic are machine-checked (bounded) against the code you can
   read." It closes the parity gap the BTX brief flags at the lowest cost
   and the highest honesty.
2. **Phase 2 (recommended as the headline, separately resourced): Option B
   — mechanize the abstract Theorem-2 statement** in a proof assistant, as
   the artifact that sits next to the academic paper — **but only published
   with the code-refinement caveat stated explicitly**, and ideally with
   the Phase-1 Kani harnesses cited as the code-side half of the binding
   (Kani checks the code refines the predicate; Option B checks the
   predicate entails safety — together they narrow the gap Option B alone
   leaves open).
3. **Scope fence — LTP soundness = obligation (a) only.** Verify the
   quorum/threshold/dedup *logic* (Phase 1 Kani covers this). **Do not**
   attempt a hardness reduction for the signature primitive: it is
   classical BLS today and **being migrated by IQ-009**, so any hardness
   artifact must target the PQ successor and is gated behind that
   decision. Cite the primitive's security from its standard literature;
   state the (a)/(b) split in the artifact so it cannot be read as a
   hardness proof.
4. **Option C parked.** Revisit deductive SMT verification (unbounded, on
   real code) only if Kani's bounds prove too weak to satisfy a specific
   buyer *and* a spike shows Verus/Creusot does not force a codebase
   refactor. Do not lead with it.

Rationale: the failure mode to avoid is not "too little proof" — the
proptests already make these correct with high confidence — it is
**publishing a machine-checked *claim* that quietly checks the wrong
object** (an abstract model presented as if it covered the code, or a
BLS hardness proof presented as if it were the PQ system). Phase 1 has no
such gap; Phase 2 is worth its cost only if the caveat is loud and the
Kani refinement is cited under it.

## Implementation sketch (Phase 1 / Option A; not scheduled)

1. **Kani harness module.** Add `#[cfg(kani)]` proof harnesses beside the
   targets — `crates/suwappu-consensus/src/joint.rs` (a `kani` submodule)
   and `crates/suwappu-ltp/src/attestation.rs`. Harnesses:
   - `verify_validator_quorum_monotone` — no vote set with distinct-signer
     stake ≤ `validator_quorum_threshold` returns `true` from
     `validator_quorum_met`; dedup means duplicate `ValidatorId`s cannot
     inflate `voting_stake` (bounded stake-table size / vote count).
   - `verify_joint_and_gate` — `joint_commit` returns `Some(h)` **only**
     when the Authority leader ratifies `h` *and* the Validator quorum
     votes `h`; disagreement or a single-ring quorum yields `None`.
   - `verify_ltp_threshold_floor` — a `CorridorAttestation` with < 7
     distinct signers is rejected; distinct-signer counting is not
     inflatable by duplicates; exactly the 9-slot membership is enforced.
2. **`run_all` entry point.** A `scripts/verify/run_all.sh` (or
   `xtask verify`) that runs `cargo kani` over the harness set and prints
   an obligation summary — matching BTX's `run_all.py` ergonomics so the
   artifact is *demonstrable*, not just present.
3. **CI wiring.** A non-blocking `kani` CI job first (Kani is slower than
   unit tests), promoted to required once stable. Keep the 10k proptests
   as the fast gate; Kani is the *proof* layer above them, not a
   replacement.
4. **Honesty doc.** A short `docs/audit/formal-verification.md` stating
   precisely: which functions are machine-checked, the **bounds** Kani
   used, the (a)/(b) soundness split, and that Phase 2's abstract theorem
   (when it lands) is model-side with the Kani harnesses as its code-side
   binding. This doc is the artifact the GTM kit and Capabilities page
   link to — it must never outrun what is actually discharged.
5. **Subagents (mandatory per CLAUDE.md):** `consensus-reviewer` (that the
   harnessed properties are the *right* Theorem-2 obligations and the
   bounds are defensible) and `crypto-reviewer` (that the LTP artifact
   claims obligation (a) only and correctly cites (b) to the primitive's
   literature, with the IQ-009 migration noted).

## Open questions

1. **Kani bounds vs meaningful coverage.** What stake-table size / vote-count
   bound is large enough that the bounded proof is *convincing* for the
   real Authority (≤40) and Validator (≤500) ring envelopes? If the real
   envelopes fit inside a tractable Kani bound, the "bounded" caveat nearly
   vanishes — quantify this before committing.
2. **Does Phase 2 earn its cost before mainnet?** The abstract Theorem-2
   mechanization is a large investment whose main payoff is
   institutional/academic credibility, not correctness. Is it a
   pre-mainnet artifact or a post-mainnet "alongside the v-next paper"
   deliverable? Phase 1 may satisfy the near-term parity need alone.
3. **Where does the group/threshold key interaction land?** If IQ-012
   (threshold ML-DSA checkpoint) or IQ-009's PQ aggregate ships, the
   verification targets shift (a threshold predicate replaces N named
   sigs). Sequence Phase 1 so its harnesses target *stable* surfaces
   (the quorum arithmetic is stable; the signature representation is in
   flux) — verify the logic that is not about to be re-shaped.
4. **Tool longevity.** Kani/Verus/Creusot are moving targets. Pin
   versions and treat the harnesses as maintained code, not a one-shot
   artifact, or the "run_all" story rots.

## Decision

**Pending sign-off.** Recommended: **Phase 1 Option A (Kani bounded model
checking on the pure quorum/threshold predicates in `joint.rs` and
`attestation.rs`, with a `run_all` entry point and an honesty doc)** as
the near-term artifact that closes the formal-verification parity gap the
BTX brief flags, at the lowest cost and with **no model-code gap**;
**Phase 2 Option B (abstract Theorem-2 mechanization in a proof
assistant)** as a separately-resourced headline artifact **published only
with an explicit code-refinement caveat and the Phase-1 harnesses cited as
its code-side binding**; **LTP soundness scoped to obligation (a) — the
quorum logic — with the (b) hardness layer cited, not re-proven, and
targeted at the IQ-009 PQ successor rather than classical BLS**; **Option C
(deductive SMT verification) parked** behind a maturity spike; **Option D
(proptests only) remains the correct baseline** and is not removed.

## See also

- [`crates/suwappu-consensus/src/joint.rs`](../../crates/suwappu-consensus/src/joint.rs) —
  the joint-quorum AND-gate; the pure predicates Phase 1 verifies.
- [`crates/suwappu-ltp/src/attestation.rs`](../../crates/suwappu-ltp/src/attestation.rs) —
  the 7-of-9 corridor attestation; the (a) protocol-logic obligation.
- [`IQ-009`](./IQ-009-ltp-aggregate-pq-migration.md) — why a hardness
  proof must target the PQ aggregate, not the classical BLS being removed.
- [`IQ-012`](./IQ-012-threshold-mldsa-checkpoint-cosignature.md) — the
  Theorem-2 accountability boundary; a caution on which surfaces a
  threshold/anonymous signature may touch.
- [`docs/research/briefs/btx-chain.md`](../research/briefs/btx-chain.md) §5.2 —
  the competitive prompt: BTX's Module-SIS reduction + 21 machine-checked
  obligations, and the "match their formal-verification bar" move.
