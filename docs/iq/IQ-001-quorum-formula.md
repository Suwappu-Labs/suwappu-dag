# IQ-001 — Commit-rule quorum formula

**Status:** Ratified 2026-05-14 via [suwappu-papers#1](https://github.com/Suwappu-Labs/suwappu-papers/pull/1)
**Owner:** consensus
**Date:** 2026-05-13 (ratified 2026-05-14)
**Sprint:** DAG-S21.1 ✅

## Question

What is the correct integer encoding of the Mysticeti-C commit-rule quorum
threshold for an Authority Ring of size `n`?

## Background

Paper §6.4 specifies:

> "Eligible transactions are certified by a fast-path quorum of `⌈(2/3)|A|⌉ + 1`
> Authority Ring members."

Definition 2 (paper §3.4) defines:

> "An Authority-Ring quorum is a subset `Q_A ⊆ A` with `|Q_A| > (2/3)|A|`."

These two statements are *consistent for real-valued stake* but **diverge for
integer cardinality** when `2n mod 3 = 2` (i.e., n ∈ {1, 4, 7, …}):

| n | `⌈2n/3⌉ + 1` (paper §6.4) | `n − ⌊(n−1)/3⌋ = 2f+1` (canonical BFT) |
|--:|:--:|:--:|
| 4 | **4** (unanimity) | **3** |
| 7 | **6** | **5** |
| 10 | **8** | **7** |
| 13 | **10** | **9** |

For n=4 the paper formula collapses to unanimity — no Byzantine fault
tolerance, despite Definition 2 only requiring strict-majority-of-2/3.

## Evidence

- **Mysticeti canonical reference** (Babel, Chursin, Danezis, Kichidis,
  Kokoris-Kogias, Koshy, Sonnino, Tian — arXiv:2310.14821, NDSS 2025): uses
  `q = 2f+1` where `n = 3f+1`. For n=4, q=3.
- **Sui Lutris production implementation**
  (`consensus/config/src/committee.rs:50-65`): ships
  `quorum_threshold = total_stake − ⌊(total_stake − 1)/3⌋`, which is the
  integer encoding of `2f+1`. For n=4, this is 3.
- **DAG-Rider / Bullshark / AptosBFT**: all use `2f+1`. None use `⌈2n/3⌉+1`.
- **Quorum-sizes survey** (arXiv:2504.08048, 2025): confirms `2f+1` is
  canonical for DAG-BFT with `n = 3f+1`.
- **Our production perf testnet** (2026-05-13): 4-region cluster with
  paper's `⌈2n/3⌉+1 = 4` formula committed exactly **once** in 9 hours
  (round 0). With one validator slow or restarting, the formula demands
  every other validator — mathematically incompatible with any f≥1.

## Options considered

1. **Adopt canonical `q = 2f+1`** with `f = ⌊(n−1)/3⌋`. Matches Sui +
   broader Mysticeti literature.
2. **Keep paper's `⌈2n/3⌉+1`** and amend the protocol to require n ≥ 7 at
   genesis. Preserves paper verbatim but locks us out of small committees
   and small test networks.
3. **Use `⌈2n/3⌉+1` for n ≥ 7 and `2f+1` for n < 7.** Hybrid formula —
   surprising and undocumented elsewhere. Rejected.

## Recommendation

**Option 1.** Replace `crates/suwappu-consensus/src/commit.rs:52-55` and
`crates/suwappu-fastpath/src/quorum.rs:39` with the canonical
`n − ⌊(n−1)/3⌋` form. Update paper §6.4 + Definition 2 to read
`q = 2f+1` with `n = 3f+1`, and explicitly state that the strict-majority
inequality in Definition 2 is *implied by* (not equivalent to) `2f+1` for
the discrete integer case.

The Joint-Quorum AND-gate (Theorem 2) safety proof relies only on
`q > 2n/3`, which `2f+1 = ⌈(2n+1)/3⌉` also satisfies for `n = 3f+1`. No
safety regression.

## Decision

- [x] Approved by: tomasuwappu (operator)
- [x] Date: 2026-05-14
- [x] Paper §6.4 + Definition 2 amendment landed in suwappu-papers PR: [Suwappu-Labs/suwappu-papers#1](https://github.com/Suwappu-Labs/suwappu-papers/pull/1)

**Ratification context.** Code shipped at `crates/suwappu-consensus/src/commit.rs:61-66`
(`quorum_threshold(n) = n - (n-1)/3`) with unit test at lines 293-310.
Paper amendments append a paragraph to Definition 2 and replace the §6.4
fast-path quorum formula with the canonical `2f_A + 1`. Ratified
alongside IQ-002 in the same suwappu-papers PR. Tracked at
[Suwappu-Labs/suwappu-dag#23](https://github.com/Suwappu-Labs/suwappu-dag/issues/23).

## Implementation

- `suwappu-consensus/src/commit.rs::quorum_threshold` rewrite
- `suwappu-fastpath/src/quorum.rs` mirror change
- Proptest `quorum_threshold_matches_paper` → renamed
  `quorum_matches_canonical_bft`, asserts `q == n − (n−1)/3` for
  n ∈ [1, 50].
- Joint-quorum stake-weighted side (`suwappu-consensus/src/joint.rs:110-114`)
  unchanged — already uses real-valued `(2·total)/3 + 1`.
