# Validator rings

Paper §5. The validator set is decomposed into two concentric quorums.

## Authority Ring 𝒜 (Proof-of-Authority)

- **Size:** 30–50 licensed institutional entities (`AUTHORITY_RING_MIN`–`AUTHORITY_RING_MAX`).
- **Stake threshold:** 100,000 GSX per member (`AUTHORITY_STAKE_THRESHOLD_GSX`).
- **Role:** produce certificates into the DAG, sign compliance attestations,
  participate in the fast-path quorum.
- **Admission:** under the Authority-Phase Matrix (paper §14):
  - **Phase G2:** GSX entity admits members against a published qualification
    rubric — regulatory licensure in the candidate's jurisdictional corridor,
    operational maturity, posted base-chain PoS stake at the per-Authority-Node
    threshold, and a signed corridor mandate.
  - **Phase G3:** Concord Council holds binding 2/3-supermajority authority
    over admission.
- **Cap rationale:** the cap of 50 is anchored on regulatory-licensing
  tractability, not consensus throughput (which scales further).

Implementation in [`gsx-authority`](../../crates/gsx-authority).

## Validator Ring 𝒱 (Proof-of-Stake)

- **Size:** 100–500 stake-weighted open participants.
- **Genesis stake threshold:** 25,000 GSX (`VALIDATOR_STAKE_THRESHOLD_GSX`).
- **Role:** ratify ordering, vote on Mysticeti commit rounds, enforce slashing.
- **Admission:** open to any party meeting the stake threshold, the operational
  uptime requirement, and the cryptographic key-management standard.

Implementation in [`gsx-validator`](../../crates/gsx-validator).

## Quorum (Definition 2)

| Quorum | Definition |
|---|---|
| Authority quorum Q_𝒜 | subset of 𝒜 with |Q_𝒜| > (2/3)|𝒜| |
| Validator quorum Q_𝒱 | subset of 𝒱 whose aggregate stake exceeds (2/3) of total stake in 𝒱 |

Cross-corridor LTP settlement additionally assumes Byzantine fault tolerance
within each corridor's super-node attestation quorum, sized to require
seven-of-nine corridor witnesses (paper §10, `gsx-ltp`).

## Why two rings

The dual-ring construction decouples:

- **Regulated counterparty trust** — concentrated in the licensed,
  KYC-verified Authority Ring.
- **Open-market economic security** — distributed broadly across the open
  Validator Ring.

The institutional property this yields is structural: a regulator can identify
the precise legal entity that signed any compliance-relevant attestation,
while an open-market validator can monitor and challenge the ring's behavior
under cryptographic and economic enforcement that does not need permission to
operate.

**Remark 1 (paper).** A single-ring chain that consolidates compliance
authority and economic security in one validator set faces a structural
trade-off: either the validator set is closed (compliance trust at the cost of
distributed economic security), or open (economic security at the cost of
identifiable regulated counterparties). The dual-ring construction sidesteps
this trade-off by separating the two functions across two quorums whose
admission gates and corruption profiles are independent.

## Joint-quorum AND-gate (Theorem 2)

A safety violation of the GSX DAG L1 requires Byzantine corruption of *both*
the Authority Ring and the Validator Ring simultaneously: there exist
$f_𝒜$, $f_𝒱$ such that any proposed conflicting commit must be signed by an
$f_𝒜 ≥ (1/3)|𝒜|$ Byzantine subset of the Authority Ring **and** an
$f_𝒱 ≥ 1/3$ stake-weighted subset of the Validator Ring.

This is the load-bearing safety claim of the chain. It is verified in
DAG-S5 by the property test `joint_quorum_safety` at 10,000 cases.

## Adversary independence

Probabilities of the two ring-corruption events are treated as independent
because:

1. **Admission gates differ** — regulatory licensure vs. open stake.
2. **Operator populations differ** — licensed institutions vs. open participants.
3. **Slashing incentives are independent** — per-Authority-Node 100% +
   expulsion vs. 5–30% stake-weight slashing on the Validator Ring.

The AND-gate is the structural property institutional counterparties demand.
