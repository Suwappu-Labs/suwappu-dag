# Consensus

Paper §6. Implemented in [`gsx-consensus`](../../crates/gsx-consensus).

## Topology

Throughput and ordering are decoupled. Validators produce certificates into a
directed-acyclic-graph gossip layer in parallel; a Mysticeti-style BFT
linearization protocol orders the DAG into a canonical linear history
[Babel et al., 2023].

Target mainnet parameters:

| Metric | Target |
|---|---|
| Certificate production | sub-second |
| Deterministic finality (p95, honest quorum) | ≤ 3 s |
| Transaction-throughput headroom (general availability) | > 10,000 TPS |
| Transaction-throughput peak (reduced TX size) | 50,000 TPS |

p95 finality budget breakdown:

- ≈500 ms certificate production
- ≈500 ms reliable broadcast
- ≈500 ms DAG advancement to the subsequent round
- ≈500 ms leader commit
- ≈1 s network-delay variance

## Mysticeti-C selection

Mysticeti-C is the consensus base. Five reasons (paper §6.2):

1. **Apache 2.0 license** — removes the IP-contamination risk of earlier
   Monad-derived paths.
2. **Production-validated at Sui-mainnet scale** [Sui Consensus, 2024].
3. **Deterministic finality through an uncertified DAG with a novel commit
   rule** — eliminates probabilistic-reorganization concerns of weakly-finalizing
   DAG protocols.
4. **Consensus path is hash-based** — natively post-quantum on the safety
   surface.
5. **Mysticeti v2 [Sui Mysticeti V2, 2025]** is the upstream evolution target.

## Inter-validator transport

Inter-validator transport runs on SCION [SCION Book, 2017] with a
SCION-IP-Gateway fallback for external clients. SCION's path-authenticated
routing eliminates the BGP-class attack vector that has produced multiple
production blockchain incidents on flat IP infrastructure [Birgi et al., 2022].
Trust Root Configuration governance over the validator mesh's Isolation
Domain provides cryptographically anchored route-authority rotation. Block
propagation uses RaptorQ erasure coding (RFC 6330) [RFC 6330, 2011].

Detailed transport spec: [transport.md](transport.md). Implementation:
[`gsx-transport`](../../crates/gsx-transport).

## Fast-path lane

A fast-path lane runs in parallel with main-lane consensus for single-owner-
object operations [FastPay, 2020]. Eligibility is restricted to transactions
whose read-write footprint is a single owned Move object with the owner as
sole signer and lineage grounded in a main-lane path. Eligible transactions
are certified by a fast-path quorum of ⌈(2/3)|𝒜|⌉ + 1 Authority Ring members,
achieving 95th-percentile end-to-end finality of 100–200 ms.

A fast-path certificate is binding subject to main-lane confirmation within
K rounds (target K=4, ≈2 s); equivocation is slashable at 100% of the
offending Authority Node's bonded stake plus expulsion.

Detailed spec: [fast-path.md](fast-path.md). Implementation:
[`gsx-fastpath`](../../crates/gsx-fastpath).

## Sprint exit gates

| Sprint | Exit gate |
|---|---|
| DAG-S3 | `dag_topological_order_unique` @ 10k |
| DAG-S4 | `mysticeti_c_finality` @ 10k |
| DAG-S5 | `joint_quorum_safety` @ 10k (Theorem 2) |
