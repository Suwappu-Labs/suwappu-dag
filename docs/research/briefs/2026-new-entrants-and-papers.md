# 2026 new entrants + recent papers — refresh brief

**Date:** 2026-07-03
**Companion to:** [`../feature-parity-matrix.md`](../feature-parity-matrix.md) ·
[`../competitive-gap-analysis.md`](../competitive-gap-analysis.md) ·
[`landscape.md`](landscape.md) (the earlier, broader survey)

A focused refresh on what's **new** — chains announced/launched in 2026
(Q2–Q3 weighted) and 2025–2026 papers that bear on suwappu-dag's design.
Deliberately skips the collapsed/stale chains (Plasma et al.) and does not
re-cover Tempo/Arc/Robinhood except for genuinely new developments. Source
URLs are inline; items not directly fetched are flagged `[search-index]`,
rumored items `[R]`.

The one-line reason this refresh matters: **the "only PQ chain in the
category" framing is now too loose** — three PQ-settlement competitors
emerged/sharpened in 2026, and the PQ-aggregation literature puts our
constant-size LTP claim (Invariant 3) on a collision course. Both are
addressed below and threaded into the parity matrix and IQ-009.

---

## Part 1 — New chains & networks (2026)

### 1a. Direct post-quantum competitors (our axis)

**QoreChain** — the closest *live* PQ L1. Mainnet **2026-06-07**; on
**2026-07-02** claimed the first end-to-end all-three-NIST-PQ transaction
on a live public mainnet. PQ surface: **ML-DSA-87 / ML-KEM-1024 /
SHAKE-256** — deliberately *higher* parameter tiers than our
ML-DSA-65/ML-KEM-768, led as a "highest NIST level" marketing hook. Tech:
"PRISM" consensus, triple VM (EVM + CosmWasm + SVM), Cosmos-SDK-derived;
targets 5,000+ TPS / sub-second finality (explicitly "pending multi-node
validation" — unproven). Model: cross-chain **attestation revenue** for
validators, Swiss non-profit association, QOR token ($90M FDV).
[Quantum Insider](https://thequantuminsider.com/2026/07/03/qorechain-first-post-quantum-blockchain-transaction/) ·
[GlobeNewswire](https://www.globenewswire.com/news-release/2026/06/04/3306991/0/en/qorechain-launches-quantum-safe-ai-native-layer-1-mainnet-on-june-7-as-community-presale-opens.html)
— **Not** a joint-quorum/dual-ring safety design and not
payments/settlement-specialized. It is the project most likely to be cited
against us on "who's first / whose PQ is stronger."

**BTQ QSSN (Quantum Secure Settlement Network)** — the closest *product*
competitor to our institutional pitch, and the one **actively selling PQ
to banks**. Dual-signs privileged issuer functions (mint/burn/upgrade/admin)
with **ECDSA + Falcon-512** via "CASH" hardware; commercial-grade Q1 2026;
selected May 2026 as core security for **South Korea's first bank-led KRW
stablecoin PoC** (iM Bank, Finger); cited by the US Post-Quantum Financial
Infrastructure Framework (PQFIF).
[BTQ](https://www.btq.com/blog/future-proofing-stablecoins-how-btqs-qssn-secures-digital-money-for-the-quantum-era) ·
[PR Newswire](https://www.prnewswire.com/news-releases/btq-technologies-qssn-selected-as-core-security-infrastructure-for-south-koreas-first-bank-led-krw-stablecoin-proof-of-concept-302763840.html)
— Crucially, QSSN is a **dual-sign overlay on issuer functions**, *not* a
full PQ BFT chain: it protects the admin surface, not consensus or the
user write path. Narrower than us; but it reaches the buyer first.

**Autheo** (mainnet 2026-06-30) and **Naoris** (mainnet 2026-04-01) —
both PQ (Autheo: ML-KEM + ML-DSA + **SLH-DSA**; Naoris: unnamed "NIST"
algos) but **off-target** — decentralized-OS/coordination and
cybersecurity-DePIN respectively, not settlement/payments. Noise in the
PQ-marketing pool, not competitors.
[Cointrust/Autheo](https://www.cointrust.com/market-news/autheo-launches-mainnet-with-post-quantum-blockchain-security) ·
[Quantum Insider/Naoris](https://thequantuminsider.com/2026/04/01/naoris-protocol-launches-mainnet-introducing-post-quantum-layer-1-blockchain/)

**BTX Chain** (`btxchain/btx`, mainnet **2026-03-19**) — a live
post-quantum "AI-native settlement" chain and our **closest positioning-twin**
(same "computational settlement / machine-verifiable / without administrators
/ institutions-exchanges-bridges-agents" copy). Under the shared sentence:
a **Bitcoin-Knots PoW fork** ("MatMul PoW" 512×512 over F(2³¹−1), 90 s
blocks, **probabilistic** finality), **ML-DSA-44** (NIST-L2) + SLH-DSA
backup from genesis, and a **SMILE v2 lattice confidential pool it shipped
at genesis then *disabled* at block 125k**. Inherits a real 9-year
**BitCore** holder/brand community (its GitHub dev community is tiny: 32★).
Genuinely ahead of us on **machine-checked formal verification** (Module-SIS
reduction, 21 obligations) and **live status**; behind on finality
determinism, safety model, PQ tier, and any payments/issuer surface. Full
analysis: [`btx-chain.md`](btx-chain.md).
[repo](https://github.com/btxchain/btx) · [btx.dev](https://www.btx.dev/)

**"Lattice: A Post-Quantum Settlement Layer"** (arXiv
[2603.07947](https://arxiv.org/abs/2603.07947), 2026-03-10) — flagged for
**name-collision** with our Lattice Transfer Protocol. On inspection it is
a **Monero-style RandomX PoW coin with ML-DSA-44** (weakest ML-DSA tier),
*not* a DAG-BFT settlement design — low competitive threat, but watch the
naming in search/SEO.

**PQ adoption among existing majors (context, not competitors):**
**Algorand** executed the first PQ *mainnet* transaction (Falcon, Nov 2025)
— the clearest shipped-PQ claim among majors; **Solana** (Dilithium
testnet w/ Project Eleven, Dec 2025); **NEAR** (ML-DSA for implicit
accounts, testnet end-Q2 2026); **Stellar**, **XRPL**, **Cardano**,
**Hedera** all roadmap-stage; **Bitcoin** BIP-360 merged (2026-02-11),
BIP-361 migration/freeze proposal (2026-04-14, contentious);
**Ethereum** EIP-8141 account-level PQ slated for the Hegotá fork (H2 2026),
consensus-layer BLS→leanXMSS on the ~2029 "Strawmap." See Part 3 for the
aggregation mechanics.

### 1b. New payments / stablecoin chains

**Maroo** (Hashed Open Finance) — public testnet **2026-05-07**; Korea's
first **won-denominated** public chain — part of a distinct *sovereign
non-USD* wave. Notable: a **Programmable Compliance Layer** (KYC, transfer
limits, blacklist, volume caps) with dual **"Open Path" vs "Regulated
Path"** tracks — a real compliance-hook story; **Shielded Pool** privacy
planned; agent-native (ERC-8004 + MCP). Consensus/throughput/gas
undisclosed; **no PQ**.
[The Block](https://www.theblock.co/amp/post/400449/hashed-open-finance-launch-testnet-of-maroo-first-sovereign-l1-blockchain-for-krw-stablecoins-and-ai-agents)

Dollar-native pack unchanged from the earlier landscape brief (Codex live,
Stable testnet, Converge H1-2026 target) — none PQ; not re-chased here.

### 1c. Institutional / regulated-settlement lane (our lane)

- **The Clearing House 17-bank tokenized-deposit network** — announced
  **2026-06-05** (JPMorgan, BofA, Citi, Wells Fargo, HSBC, PNC, Truist,
  U.S. Bank, TD, BNY, BMO, Citizens, Fifth Third, KeyBank, Regions,
  Santander, Huntington). On-chain settlement bridged to **CHIPS + RTP
  (>$2T/day)**, programmable controls, target **H1 2027**, **blockchain
  vendor not yet selected**. The most important new entrant to our lane —
  and a possible *integration target*, not just a rival, since the vendor
  slot is open. **No PQ.**
  [CoinDesk](https://www.coindesk.com/markets/2026/06/05/jpmorgan-bank-of-america-and-citi-are-going-on-the-blockchain-offensive-with-a-shared-tokenized-network) ·
  [Ledger Insights](https://www.ledgerinsights.com/us-banks-tap-the-clearing-house-for-tokenized-deposit-network/)
- **Cari Network** — 5 US regionals (KeyBank, Huntington, First Horizon,
  M&T, Old National) on **Prividium (private ZKsync L2)**; pilot Q3 2026,
  production Q4 2026 — shipping faster than the big-bank net. **No PQ.**
  [CoinDesk](https://www.coindesk.com/business/2026/03/17/u-s-regional-banks-building-tokenized-deposit-network-on-zksync-to-rival-stablecoins)
- **Canton Network** — the institutional-L1 benchmark: ~780 validators,
  ~$9T/month volume, ~$65M/month fees, DTCC+Euroclear-co-chaired; **JPMD
  (JPM Coin) going native on Canton** (Jan 2026); DTCC tokenizing DTC
  Treasuries on Canton in 2026. **Parity bar features:** sub-transaction
  privacy (Daml), atomic cross-app composability, deterministic finality.
  **No PQ roadmap surfaced.**
  [Genfinity](https://genfinity.io/2026/01/29/canton-network-institutional-blockchain-overview/)
- **Kinexys (JPMorgan)** — expanded to **8 currencies** (2026-06-29),
  >$4T processed, >$7B/day. **Fnality** — $136M Series C, USD/EUR pending;
  differentiator = **central-bank-money** settlement. **Partior**,
  **HSBC** (tokenized deposits to US, Apr 2026), **SG-FORGE EURCV**, **DBS**
  — all active, none PQ.
- **CBDC-adjacent:** **Project Agorá** (BIS + 7 central banks, testing
  through 2026) vs **mBridge** (BIS-exited, live, ~$55.5B cumulative) — the
  2026 framing is a **geopolitical split** (G7 bloc vs China bloc, no
  overlapping membership), relevant to how we pitch neutral cross-chain
  settlement.

---

## Part 2 — Regulatory clock (the "why now")

- **Executive Order 14412, "Securing the Nation Against Advanced
  Cryptographic Attacks," signed 2026-06-22** — converts PQC aspiration
  into **dated federal mandates**: key establishment by **2030-12-31**,
  digital signatures by **2031-12-31**; OMB guidance in 90 days, first FAR
  contractor rule in 180 days. **National-security systems carved out** —
  CNSA 2.0 still governs them (procurement preference **2027-01-01**,
  exclusive use ~2033–2035). Net: **two dated horizons — ~2030–31 civilian
  / ~2035 NSS** — and PQC entering FAR procurement + counterparty
  due-diligence language.
  [White House](https://www.whitehouse.gov/presidential-actions/2026/06/securing-the-nation-against-advanced-cryptographic-attacks/) ·
  [postquantum.com](https://postquantum.com/post-quantum/us-federal-pqc-mandate-2026/)
  — This is a materially stronger "why now" than the CNSA-only line we had.
  It gives an institutional buyer a *dated, contractual* trigger.
- **Eurosystem (BIS + Banque de France, Bundesbank, Banca d'Italia)**
  tested **PQC signatures inside TARGET2-like wholesale liquidity
  transfers** — proof the settlement pipes are migratable.
  [BIS Papers No. 158](https://www.bis.org/publ/bppdf/bispap158.pdf)
- Standards: FIPS 203/204/205 final (Aug 2024); **FIPS 206 (FN-DSA/Falcon)**
  draft; **HQC** selected (Mar 2025) as the code-based KEM, draft ~2026–27
  — a potential **algorithm-diversity hedge** alongside ML-KEM-768 on our
  confidentiality surface.

---

## Part 3 — Papers that bear on our design (2025–2026)

### 3a. The constant-size-aggregate problem (highest relevance — Invariant 3)

**Headline finding: no 2025–2026 PQ construction reproduces BLS's ~96-byte
constant aggregate.** The realistic PQ replacements are:

- **SNARK/STARK-recursed hash-based aggregates** → *constant-but-large*
  proofs (tens–hundreds of KB, amortized over many signers). This is the
  Ethereum Beam Chain direction:
  - **"Hash-Based Multi-Signatures for Post-Quantum Ethereum"** (Drake,
    Khovratovich, Kudinov, Wagner), IACR [2025/055](https://eprint.iacr.org/2025/055) /
    Communications in Cryptology — *peer-reviewed*; the leanXMSS blueprint
    (XMSS one-time keys aggregated via SNARK).
  - **HAPPIER** (XMSS + Risc0 zkVM, **multi-level/incremental**
    aggregation) — [Springer](https://link.springer.com/chapter/10.1007/978-3-032-15541-2_1)
    `[search-index]`; multi-level maps naturally onto a **two-ring**
    topology (aggregate within each ring, then aggregate the two ring
    proofs).
  - **Loquat** (~145 KB constant aggregate, Legendre-PRF, SNARK-friendly),
    IACR [2024/868](https://eprint.iacr.org/2024/868.pdf); **CAPSS**
    framework IACR [2025/061](https://eprint.iacr.org/2025/061.pdf).
  - **Flock** — PQ proof system for aggregating thousands of hash-based
    sigs fast (~661k Blake3/s on 10 cores) — the mid-2026 "make it fast
    enough" result.
- **Lattice multisignatures** → tens of KB:
  - **Lemur** (LaBRADOR-based), IACR [2026/1161](https://eprint.iacr.org/2026/1161.pdf)
    `[search-index]` — **~73 KB aggregate for 1,024 signers**, ~15 ms
    verify. The number to beat for a lattice (not hash) validator-set
    aggregate.
- **Threshold ML-DSA** → standard **3.3 KB** signatures, **unmodified
  FIPS-204 verifiers**:
  - **"FIPS 204-Compatible Threshold ML-DSA via Shamir Nonce DKG"** (Kao),
    arXiv [2601.20917](https://arxiv.org/abs/2601.20917) — *directly
    applicable to our Authority-ring joint checkpoint co-signature*: a true
    t-of-n threshold with no custom verifier, reducing reliance on the BLS
    exception zone.

**Decision this forces (see IQ-009):** post-quantum, "constant-size" can
mean **O(1)-in-signers** but **not** the ~96-byte byte-count. Invariant 3
must be re-stated as O(1)-in-participants with an explicit, larger PQ byte
budget, or the LTP commitment must adopt a SNARK-recursed aggregate. This
is the surface a reviewer will probe first.

### 3b. DAG-BFT consensus (informs PERF + commit rule)

- **Simple-IT** (arXiv [2606.14404](https://arxiv.org/abs/2606.14404),
  Jun 2026) — **signature-free** BFT over PQ-authenticated channels;
  removes per-vote ML-DSA verification from the hot path. The cheapest PQ
  consensus story — relevant to keeping ring voting fast under PQ.
- **Shoal++** (NSDI 2025, [usenix](https://www.usenix.org/system/files/nsdi25-arun.pdf))
  — pipelined multi-anchor Bullshark; best current throughput-robustness
  point; the baseline for our leader-timeout/fallback behavior.
- **Sailfish** (IACR [2024/472](https://eprint.iacr.org/2024/472.pdf),
  CCS 2025) — 3δ leader-vertex commit; benchmark for our commit path.
- **Mahi-Mahi** (IEEE 2025) — sub-second **asynchronous** DAG BFT;
  reference for an async fallback. **Odontoceti** (arXiv
  [2510.01216](https://arxiv.org/abs/2510.01216)) — 2-round commit but at
  **n=5f+1 / 20% BFT**, which would *collide with our joint-quorum
  AND-gate* — a cautionary data point, not an adoption target.

### 3c. Confidential / private stablecoin payments (informs CONF-1)

- **"A Practical Post-Quantum Distributed Ledger Protocol for Financial
  Institutions"** (arXiv [2603.05005](https://arxiv.org/abs/2603.05005),
  Mar 2026) — lattice-based **publicly-verifiable + auditable** confidential
  transfers, a re-commitment primitive, and a **compact PQ range proof**;
  argues Ring-CT is unsuitable for institutions. The closest published match
  to CONF-1's target (ML-KEM-encrypted amounts + PQ range proofs + regulator
  viewing path).
- Note the migration gap: **ERC-7984** confidential-token standard and
  Solana Confidential Transfers are **classical** (twisted-ElGamal + range
  proofs) — a PQ-confidential design is genuinely differentiated.
- Caution: lattice range-proof commitment size can be **linear in message
  size** (Esgin et al., IACR [2021/1674](https://eprint.iacr.org/2021/1674.pdf))
  — a real obstacle for amount-sized values; the 2603.05005 "compact"
  claim is the thing to verify.

### 3d. Cross-chain attestation (informs BRIDGE-1 / IQ-010)

- **Random-Sampling Light Clients** (IACR [2025/057](https://eprint.iacr.org/2025/057.pdf))
  — succinct, security-parameterized attestation replacing full
  sync-committee verification; a direct size-vs-security knob for our
  constant-size cross-chain budget.
- **QLink** (arXiv [2512.18488](https://arxiv.org/abs/2512.18488)) — a PQ
  bridge, but leans on **QKD** (physical-layer) for validator links — an
  assumption we should *not* inherit; contrast for the LTP approach.

### 3e. Fee abstraction (informs FEE-1 / IQ-007)

Little new *academic* work — the movement is standards: **EIP-7702**
(shipped in Ethereum Pectra 2025; EOA acts as smart account for sponsored
gas/batching) and **ERC-4337 paymasters** (USDC ≈62% of paymaster volume).
**Design steer for IQ-007:** prefer **protocol-native fee-payer separation**
(a first-class sponsor field) over bolting on ERC-4337 — Ethereum itself is
pushing abstraction down into the protocol (EIP-7702 trajectory). This
*confirms* the IQ-007 Option-A recommendation.

### 3f. Migration framing (defends Invariant 2)

- **"Quantum Disruption: An SoK…"** (Northeastern, arXiv
  [2512.13333](https://arxiv.org/abs/2512.13333), Dec 2025) — systematizes
  that PQ is **not a drop-in swap**; forces architectural redesign. The
  best single citation to justify our PQ-conservative surface to reviewers.
- **"Assessing the Impact of PQ Digital Signatures on Blockchains"**
  (IEEE TrustCom 2025, arXiv [2510.09271](https://arxiv.org/abs/2510.09271))
  — per-signature cost benchmarks (ML-DSA can beat ECDSA on verify at high
  security on ARM) — sizing data for our verification budget.

---

## Part 4 — Implications for suwappu-dag

1. **Sharpen the moat claim. We are not "the only PQ chain" — QoreChain is
   live, BTQ QSSN is selling PQ to banks, Algorand shipped the first PQ
   mainnet tx.** The defensible, precise claim is: **the only PQ *dual-ring
   joint-quorum BFT settlement L1*.** Each PQ rival is narrower on a
   specific axis — QoreChain (no joint-quorum, not settlement-specialized),
   BTQ QSSN (admin-overlay, not a chain), Algorand (account-level PQ, not
   dual-ring settlement), the "Lattice" paper (PoW coin, ML-DSA-44,
   academic). Update the parity matrix F3 row and the GTM kit accordingly.
2. **Invariant 3 is on a collision course with PQ reality** — no ~96-byte
   PQ aggregate exists. IQ-009 must (a) re-state "constant-size" as
   O(1)-in-signers with an explicit larger PQ byte budget, and (b) adopt
   the SNARK-recursed hash-based aggregate as the migration target (the
   field has converged here). This is our most-probed surface — get ahead
   of it in the paper.
3. **Threshold ML-DSA (arXiv 2601.20917) is a near-term win** — it lets the
   Authority-ring checkpoint co-signature become a true t-of-n with
   standard 3.3 KB sigs and unmodified verifiers, chipping away at the BLS
   exception zone without waiting for full SNARK aggregation.
4. **EO 14412 upgrades the "why now"** — thread the two-horizon
   (civilian 2030-31 / NSS 2035) framing through the GTM kit and any
   institutional pitch; it is a dated, contractual buyer trigger, not a
   slide.
5. **The institutional parity bar is Canton's feature set** (sub-tx
   privacy, atomic composability, deterministic finality) + Fnality's
   central-bank-money settlement — and **none of the incumbents has PQ.**
   That is the open flank; the three PQ specialists (QoreChain/BTQ/Lattice)
   are *not* in the wholesale-bank lane. Our wedge is the intersection:
   PQ **and** dual-ring-safe **and** institutional-settlement-shaped.
6. **The Clearing House network (vendor undecided) is a possible integration
   target, not only a rival** — worth noting in GTM outreach.
