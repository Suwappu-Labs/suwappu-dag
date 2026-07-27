# DagBft / DagBft-C: an independent technical writeup

Companion to [consensus.md](consensus.md) (the section-mapped "how it
works" doc) and [safety-liveness.md](safety-liveness.md) (the safety
argument for Theorem 2). This doc is different in purpose: it is a
standalone, citable description of **our** consensus design — the kind
of document you'd link from a whitepaper, cite in a security review, or
hand to an auditor who has never opened this repo — written explicitly
to stand apart from Mysten Labs' Mysticeti-C.

This repo's consensus was originally named "Mysticeti"/"Mysticeti-C"
internally. That collided with a real, deployed, published protocol:
Mysticeti-C from Mysten Labs (Babel, Chursin, Danezis, Kichidis,
Kokoris-Kogias, Koshy, Sonnino, Tian, **arXiv:2310.14821**, NDSS 2025),
live in `github.com/MystenLabs/sui/consensus/core` and securing
$1.5B+ of value on Sui mainnet. The internal names were changed to
`DagBftProtocol`/`DagBftAdapter`/`DagBft-C` across this workspace
(consensus classes and docs) to remove that collision. This document
is the substantive follow-up: not just a rename, but an honest
description of what our design actually is, where it is a close
derivative of Mysticeti-C's ideas (attributed, not hidden), and where
it concretely diverges.

**Status of this document:** engineering writeup, not peer-reviewed,
not a whitepaper chapter. Every claim below is checked against the
source in this repository as of 2026-07-27 (commit history through
PR #33, "Wire real cert signing + verify-before-admission into the
live daemon"). Where something is unverified or untested at scale,
this doc says so plainly rather than rounding up.

## 1. What DagBft-C is

DagBft-C is a DAG-based Byzantine-fault-tolerant consensus protocol
for an Authority Ring of `n` validators. Like Mysticeti-C, Narwhal-
Bullshark, and DAG-Rider before it, it separates **data dissemination**
from **ordering**:

- Every Authority Node produces at most one *certificate* per round.
  A certificate is a compact record — author, round, parent
  certificate hashes, and a 32-byte payload digest — not the block
  contents themselves (`crates/suwappu-consensus/src/cert.rs`).
- Certificates reference certificates from strictly earlier rounds as
  parents, forming a directed acyclic graph. There is no leader-first
  broadcast step and no separate "vote" message type for the DAG
  itself — a certificate's presence *is* its implicit vote, and a
  later certificate that includes an earlier one as a parent *is* its
  support for that certificate (`crates/suwappu-consensus/src/dag.rs`,
  `crates/suwappu-consensus/src/commit.rs`).
- Ordering (which certificates are finalized, and in what order) is
  derived after the fact by a deterministic **commit rule** run locally
  by every node over its own view of the DAG — no additional network
  round trips are needed to reach agreement on ordering once enough of
  the DAG has arrived. This is the "uncertified DAG, deterministic
  finality via a commit rule" idea that Mysticeti-C introduced over
  Bullshark's certified-DAG predecessor, and we adopt that structural
  idea directly and attribute it (see §3).

### 1.1 Rounds, certificates, and the quorum threshold

For an Authority Ring of size `n`, let `f = ⌊(n-1)/3⌋` be the maximum
tolerated Byzantine count. The quorum threshold is

```
q = 2f + 1 = n − ⌊(n-1)/3⌋
```

(`crates/suwappu-consensus/src/commit.rs::quorum_threshold`). This is
the standard BFT supermajority — "more than 2/3" expressed as an exact
integer rather than a ceiling of a fraction — and matches what Sui,
Aptos, and Bullshark ship in production. It is *not* identical to the
formula in early drafts of our own design paper; see §2.1, which is one
of the concrete divergence points this document exists to spell out.

A certificate at round `r` is well-formed only if every parent it cites
is already in the local `DagStore` and has round `< r` (round
monotonicity); round 0 certificates are genesis certificates with no
parents. These are pure structural invariants, checked on insertion
regardless of signature validity — signature checking is a separate,
composed step (§4).

### 1.2 The commit rule: direct and indirect decision

The leader at round `r` is the Authority Node with `AuthorityId ≡ r
(mod n)` — deterministic, round-robin, no leader-election protocol or
view-change machinery (`commit.rs::leader`).

**Direct rule.** The leader's certificate at round `r` is *directly
committed* iff at least `q` distinct Authority Nodes have, at round
`r+1`, produced a certificate that cites the leader's certificate hash
as a parent (`try_direct_decide`). This is the one-round confirmation
step: a leader cert becomes final as soon as enough of the next
round's certificates point back at it.

**Indirect rule.** If the direct rule doesn't fire, `decide_slot` scans
every *later* round `r' ≥ r+2` that has itself been directly decided
(an "anchor"), and asks whether the leader certificate at `r` is
reachable by walking the anchor's `causal_history` — its full ancestor
set, computed by a breadth-first parent walk (`commit.rs::causal_history`).
If reachable, `r`'s leader inherits a `Direct` decision from the anchor.
If no anchor's causal history ever reaches it, the slot is `Skip` —
permanently, since a directly-decided anchor's finality is itself
irrevocable (more anchors only extend the DAG, never retract a
decision). This inherited-decision structure — decide a later round
directly, then backfill earlier undecided rounds by walking causal
history — is the core Mysticeti-C idea (see §3) and we implement it
essentially as described, with one liveness-oriented divergence
detailed in §2.2.

Every leader's finalization pulls its entire causal history into the
linear commit order (`commit.rs::finalize`), deduplicated so already-
included ancestors aren't re-emitted. The resulting order is the
protocol's canonical transaction history.

**Safety property, verified.** `mysticeti_c_finality` (naming
predates the rename; the test itself is unchanged) checks at 10,000
random-DAG cases that a `Direct` decision, once reached, is never
retracted by subsequent DAG growth — the monotonicity property that
makes "decide later, backfill earlier" sound. A second proptest,
`index_vs_linearize_equivalence` (also 4,096–10,000 cases), checks
that the production (indexed) implementation of `cert_at`,
`supporters`, `causal_history`, `decide_slot`, and `finalize` is
byte-identical to a naive linearize()-based reference implementation
across randomly generated DAGs — including DAGs with equivocating
certificates, missing leaders, and deep multi-wave indirect-commit
chains. This is a real correctness oracle, not just a happy-path test.

## 2. Concrete divergences from Mysticeti-C

The point of this section is to name actual, checkable differences —
not to assert novelty in the abstract. Four are load-bearing enough to
be visible in the source and its accompanying design-decision records
(the "IQ" series under `docs/iq/`).

### 2.1 Quorum formula: integer encoding, not the paper's literal fraction

Our own design paper (not Mysticeti-C's) originally specified
`q = ⌈2n/3⌉ + 1`. That formula collapses to unanimity (`q = n`) whenever
`2n mod 3 = 2` — which happens at `n ∈ {1, 4, 7, 10, 13, …}`. A 4-node
perf testnet running that literal formula committed exactly once in
nine hours, because it required all four nodes online simultaneously
with zero fault tolerance — defeating the point of BFT consensus. The
fix, `q = 2f+1` with `f = ⌊(n-1)/3⌋`, is the formula every production
DAG-BFT system (Sui/Mysticeti-C included) actually ships; the paper's
`⌈2n/3⌉+1` was an under-specified shorthand for the same intended
property ("strict supermajority"), not a deliberate design choice we
are diverging from. Full derivation and ratification:
[IQ-001](../iq/IQ-001-quorum-formula.md).

This is not a difference *from* Mysticeti-C — Mysticeti-C already uses
`2f+1`. It is called out here because it is a place where our design
process (paper draft → implementation → correction, publicly tracked
via a ratified IQ) is visible and worth being honest about, rather than
presenting the current formula as having been correct from day one.

### 2.2 Late-arrival multi-anchor scan (IQ-004): a liveness fix Mysticeti-C's published description doesn't need to make

Our parent-set selection rule is "propose with what you currently
have": when a node builds its round-`R` certificate, it takes parents
from its own local view of round `R-1`, not a globally agreed set. If
a leader certificate at round `R` hasn't reached a peer by the time
that peer proposes at `R+1`, the peer's `R+1` certificate simply omits
it — and once shipped, that certificate can never retroactively cite
it. The original single-anchor implementation of `decide_slot` searched
only the *first* directly-decided anchor at `R' ≥ R+2`; if that
particular anchor's causal history didn't happen to reach the
orphaned leader certificate (because it arrived late, after that
anchor's DAG region had already been built), the slot was reported
`Skip` — permanently — even though a *later* anchor's causal history,
built after the orphaned certificate had propagated, could have
reached it.

The fix (issue #45, `commit.rs::decide_slot`) is to scan *every*
directly-decided anchor at `R' ≥ R+2` in ascending order and take the
first one whose causal history reaches the target, rather than
stopping at the first anchor found. `Skip` is now returned only if
*no* anchor ever reaches the leader certificate. This closes a real
liveness gap in orphan-window handling that is specific to this
implementation's proposal timing; it isn't a claim that Mysticeti-C's
published protocol has the same bug; we don't have visibility into
`consensus/core`'s exact proposal-timing implementation to make that
comparison. It is documented here as a concrete place where getting
the indirect-commit rule *right* required more care than a first
reading of the "decide later, backfill earlier" idea suggests. See
[IQ-004](../iq/IQ-004-decide-slot-orphan-window.md) for the full
incident writeup and the property-test coverage that now specifically
exercises the multi-anchor Skip→Direct path (`build_random_dag`'s
sparse per-author parent selection, engineered so sibling certificates
at the same round have divergent ancestor sets).

### 2.3 Joint-quorum AND-gate: a second, independent ring — not part of Mysticeti-C at all

This is the largest structural divergence, and it's an addition, not a
tweak. Mysticeti-C as published is a single-committee DAG-BFT protocol:
one set of validators, one quorum threshold, one commit decision.

Our design runs **two** separate rings and requires both to agree
before anything is final (`crates/suwappu-consensus/src/joint.rs`):

1. **Authority Ring** (Proof-of-Authority, the DAG-BFT committee
   described in §1) reaches its own `q = 2f+1` quorum on a candidate
   certificate via the commit rule in §1.2.
2. **Validator Ring** (Proof-of-Stake) independently casts stake-
   weighted votes for candidates; a candidate needs strictly more than
   two-thirds of total stake (`validator_quorum_threshold`) to be
   ratified on this leg.

`joint_commit` returns a finalized hash only if *both* legs agree on
the *same* candidate. The safety argument (paper Theorem 2,
[safety-liveness.md](safety-liveness.md)) is that a fork — two
different certificates both jointly ratified at the same slot —
requires simultaneous Byzantine corruption of both rings: an Authority-
side equivocator set reaching the Authority BFT threshold *and* a
Validator-side double-voting stake reaching the stake BFT threshold, at
the same time. `authority_equivocators` and
`validator_double_vote_stake` compute exactly those two overlap sets,
and `joint_quorum_safety` is a dedicated 10,000-case property test for
this composed safety property.

This two-ring joint-quorum design is not present in Mysticeti-C, which
finalizes on a single committee's quorum. It exists here because the
broader system (`docs/architecture/validator-rings.md`) separates
block-production authority (a smaller, permissioned Authority Ring)
from economic stake (an open, larger Validator Ring), and the AND-gate
is how those two separately-governed sets are tied into one safety
guarantee rather than one silently subsuming the other.

### 2.4 Post-quantum signing on the certificate path

Every certificate carries a detached **ML-DSA-65** (FIPS 204) signature
over its own canonical hash (`Certificate::sign` /
`Certificate::verify_signature`, `cert.rs`), using the same Authority
Ring keys already used for bridge-header attestation. `ingest_cert` in
the live daemon (`crates/suwappu-node/src/daemon.rs`, wired in PR #30 +
PR #33) verifies every gossip-received certificate against the
author's genesis-registered public key **before** admitting it into the
`DagStore` — rejection means the certificate is never inserted, never
orphan-cascaded, and never re-served via `GetCert`. `DagStore::insert`
itself checks only structural DAG invariants and does not check
signatures, by design, so that the DAG's own extensive proptest suite
can construct certificates directly without needing key material; the
composed safety property (signed-before-admitted) is instead proven at
the daemon integration layer.

Mysticeti-C as published operates over classical signatures (Sui uses
Ed25519 for consensus messages, per the public `consensus/core`
implementation and its documentation) — we are not aware of a
published post-quantum variant of Mysticeti-C at the time of writing.
Signing consensus certificates with a NIST-standardized lattice-based
scheme is, as far as this document's authors can determine from public
sources, a difference in kind from what's publicly described for
Mysticeti-C, not merely a parameter choice. We're careful not to
overclaim this as "the first PQ DAG-BFT" — we simply have not surveyed
every DAG-BFT implementation closely enough to assert uniqueness, and
this document does not make that claim. What we can say concretely:
this repository's consensus path signs and verifies every certificate
with ML-DSA-65, proven end-to-end (not just unit-tested in isolation)
by `four_node_main_lane_commits` — four real validators, four distinct
real keypairs, real cross-peer signature verification, consensus
commits reached.

### 2.5 Fast-path lane: attributed to FastPay, layered alongside, not part of the DAG-BFT commit rule itself

A parallel fast-path lane (`crates/suwappu-fastpath`,
[fast-path.md](fast-path.md)) handles single-owner-object transactions
without going through the main-lane DAG-BFT commit rule at all,
following the FastPay pattern (Baudet et al., 2020) that Sui's own
fast-path design also draws on. We attribute this to FastPay directly,
not to Mysticeti-C, since Mysticeti-C's academic description is about
the main-lane DAG ordering problem, and the fast-path idea predates it.
This is included here for completeness, not as a claimed point of
divergence from Mysticeti-C specifically.

## 3. What we deliberately did not change (attribution, not erasure)

The core structural idea — an uncertified DAG (no explicit certificate-
of-availability broadcast round), paired with a commit rule that
decides some rounds directly and backfills earlier undecided rounds by
walking a later anchor's causal history — is Mysticeti-C's central
contribution over its Bullshark/DAG-Rider predecessors, which required
certified DAGs (an extra round of broadcast-and-acknowledge before a
certificate could even enter the DAG). We use that same structural
idea, and we say so in the module-level docs
(`crates/suwappu-consensus/src/lib.rs`, `cert.rs`, `commit.rs`) with a
direct citation to arXiv:2310.14821. The renamed `DagBft`/`DagBft-C`
identifiers exist to remove a *name* collision with a real, deployed
protocol — not to claim the underlying ideas are unrelated to it. A
technical writeup that pretended otherwise would be less honest than
the naming collision it's trying to fix.

## 4. What's proven, what isn't

**Proven (source-grounded, test-covered):**

- Structural DAG invariants (parent existence, round monotonicity,
  genesis shape, no duplicate insertion) — unit-tested plus a 10k-case
  determinism property.
- Direct + indirect commit rule finality (no retraction once
  `Direct`) — `mysticeti_c_finality`, 10k cases; production-vs-reference
  equivalence — `index_vs_linearize_equivalence`, 4,096 cases including
  equivocation and multi-wave indirect chains.
- Joint-quorum AND-gate safety (Theorem 2: fork requires simultaneous
  Byzantine corruption of both rings) — `joint_quorum_safety`, 10k
  cases.
- ML-DSA-65 signing and verify-before-admission, end-to-end across
  real distinct validator keypairs on a real 4-node network —
  `four_node_main_lane_commits`.
- **Real, measured throughput on a local N=4 devnet** (epic item #4,
  `scripts/devnet-local-bench.sh`, real ML-DSA-65-signed intents, real
  DAG-BFT commit, RATE=180 for 30s across 4 targets): 4,800/4,800
  intents committed, **p50 = 575 ms, p95 = 845 ms, p99 = 2,602 ms**,
  **steady-state 162.5 TPS**. This number is honestly caveated as
  **submission-rate-bound, not network-saturation-bound** —
  `suwappu-mempool`'s hardcoded per-connection rate limit (50 tok/s
  refill / 100 burst) caps a single load-generator process at roughly
  200 TPS aggregate across 4 targets before mid-batch rejections start;
  it is not evidence of the network's actual saturation ceiling.

**Not yet proven — open, and stated as open:**

- **No N=7 (or larger) measurement exists.** Every throughput/latency
  number above is from a 4-node local loopback devnet. Scaling
  behavior at committee sizes closer to a real deployment (7, 13, or
  more Authority Nodes, real network latency instead of loopback) has
  not been measured and should not be inferred from the N=4 numbers.
- **No network-saturation throughput ceiling has been measured.** The
  162.5 TPS figure is a submission-side artifact of one client
  process's rate limit, not the protocol's actual capacity. Reaching a
  real saturation number requires either exposing the mempool rate
  limit via config or running multiple concurrent client connections —
  tracked as a follow-up, not done.
- **This document itself has not been externally reviewed.** It is an
  internal engineering writeup, checked against source by its authors,
  not audited or peer-reviewed. It should be treated as a starting
  point for external review (e.g. the Trail of Bits / Zellic audit
  scoping tracked as epic item #8), not as a substitute for one.
- **The IQ-004 late-arrival fix's interaction with the fast-path lane
  and joint-quorum AND-gate under adversarial network partitioning has
  not been separately proven** — the 10k-case property tests cover the
  DAG-BFT commit rule and joint-quorum safety in isolation; a combined
  adversarial-network liveness proof spanning all three subsystems
  together does not exist yet.

## 5. Primary sources cited in this document

- Babel, Chursin, Danezis, Kichidis, Kokoris-Kogias, Koshy, Sonnino,
  Tian. *Mysticeti: Reaching the Latency Limits with Uncertified DAGs.*
  **arXiv:2310.14821**, NDSS 2025. (Mysticeti-C — the protocol this
  design is explicitly distinguished from by name, and whose core
  uncertified-DAG/commit-rule idea we adopt and attribute per §3.)
- Baudet, Danezis, Sonnino. *FastPay: High-Performance Byzantine Fault
  Tolerant Settlement.* AFT 2020. (Fast-path lane, §2.5.)
- Danezis, Kokoris-Kogias, Sonnino, Spiegelman. *Narwhal and Tusk: A
  DAG-based Mempool and Efficient BFT Consensus.* **arXiv:2201.05677**,
  EuroSys 2022. (Certified-DAG/Bullshark lineage that Mysticeti-C
  itself improves on; cited here for the same reason it's cited in
  [IQ-002](../iq/IQ-002-indirect-commit.md) — the indirect-commit idea
  has roots in this line of work, not only in Mysticeti-C.)
- NIST FIPS 204 (ML-DSA) — the standardized signature scheme used for
  every certificate signature described in §2.4.
- Sui / Mysten Labs public engineering material (`consensus/core`,
  Mysticeti V2 blog post) — cited in [consensus.md](consensus.md) for
  the production-scale validation claim; not independently re-verified
  by this document beyond what's already checked in that doc.

Only citations that are actually load-bearing for a claim made in this
document are listed. Where a comparison to Mysticeti-C's implementation
details couldn't be verified against public source (§2.2, §2.4), this
document says so explicitly rather than asserting the comparison.
