# BTX Chain (btxchain/btx) — competitor brief

**Date:** 2026-07-06
**Companion to:** [`../feature-parity-matrix.md`](../feature-parity-matrix.md) ·
[`2026-new-entrants-and-papers.md`](2026-new-entrants-and-papers.md) ·
[`../competitive-gap-analysis.md`](../competitive-gap-analysis.md)

A focused read on **BTX Chain** — a live, post-quantum, "AI-native"
settlement chain that overlaps our positioning more directly, in words,
than any competitor surveyed so far. Sources are inline. Repo facts are
from the public README and docs as of **v0.32.12 (June 2026)**; the chain
went to genesis **2026-03-19**.

> **Why this one matters despite a tiny GitHub:** BTX's marketing copy is
> nearly a paraphrase of ours — "a computational settlement system … under
> rules that are machine-verifiable, neutral, and durable under pressure,"
> "settlement without administrators," targeting "institutions, exchanges,
> bridges, and autonomous agents." It is **post-quantum from genesis** and
> shipped **lattice confidential transactions from genesis**. On paper it
> occupies our exact sentence. The gap is in the *architecture* under the
> sentence, and that gap is the whole argument.

Sources:
[repo](https://github.com/btxchain/btx) ·
[btx.dev](https://www.btx.dev/) ·
[docs/overview](https://www.btx.dev/docs/getting-started/overview/)

---

## 1. What BTX actually is (sourced)

A **Bitcoin Knots v29.2 fork** (C++ 68% / Python 17%, UTXO, Nakamoto
longest-chain) with four substitutions on top of the Bitcoin base:

- **Consensus — "MatMul PoW":** proof-of-work is a **512×512 matrix
  multiplication over F(2³¹−1)** instead of SHA-256d. Pitched as *useful
  work*: "the hardware securing the network is the same class used for AI
  training and numerical computation — security expenditure that remains
  productive outside mining." GPU/TPU-friendly; CUDA 12/13 miners. 90 s
  target block, per-block ASERT difficulty. Two-phase validation (O(1)
  header check, rate-limited O(n³) full multiply).
- **Signatures — post-quantum from genesis:** **ML-DSA-44** (FIPS 204,
  Dilithium; 1312 B pk / 2420 B sig) as the active primary, **SLH-DSA-SHAKE-128s**
  (FIPS 205, SPHINCS+; 32 B pk / 7856 B sig) as backup. Witness-v2 **P2MR**
  outputs, `btx1z…` bech32m addresses.
- **Confidentiality — lattice CT from genesis:** a **SMILE v2 / MatRiCT**
  shielded pool (lattice-based confidential transactions, ring signatures
  ring-size 8–32, Dandelion++ relay from block 250k, selective disclosure).
  **Value soundness has a real formal-verification suite** — an accounting
  firewall, "C-002 verifier-relation bindings," and "a reduction of forgery
  hardness to Module-SIS — with 21 machine-checked obligations."
- **Programmability:** covenants — `OP_CHECKTEMPLATEVERIFY` (CTV, vaults /
  payment trees) and `OP_CHECKSIGFROMSTACK` (CSFS, oracle signatures via
  ML-DSA-44 / SLH-DSA). BIP-110-style reduced-data limits (83 B OP_RETURN,
  34 B scriptPubKey).

Economics: Bitcoin-clone — 21M fixed supply, 20 BTX initial subsidy,
halving every 525,000 blocks. Not a stablecoin/RWA chain.

### The load-bearing caveat: the privacy pool was wound down in production

BTX shipped shielded-from-genesis and then **turned it off**. Verbatim:
"After block `125000`, consensus disables new shielded credits, private
shielded-output appends, bridge ingress/control rollover, and re-shielding,"
and from block 128000 even transparent-funded public-flow shielding is
disabled. What remains is **balance viewing + transparent exit** of the
existing shielded pool. So the flagship confidential feature is, in the
live chain, **effectively deprecated** — a launch-then-retreat, not a
standing capability. This is the single most important fact for us: their
strongest overlap with our CONF-1 thesis is a feature they walked back.

### Throughput, honestly

Their own headline blend: "`3,263 tx/block` and about **`36.26 TPS`**"
(24 MB block, 90 s cadence, a 199-shielded / 3,064-transparent mix).
Direct 1×2 shielded sends alone are ~199/block ≈ **2.2 TPS**. This is a
Bitcoin-class settlement chain, not a high-TPS payments rail — and they
don't pretend otherwise.

---

## 2. The "big community" — what it is and isn't

Two different things share the ticker **BTX**, and the community belongs to
the *older* one:

- **BitCore (BTX)** — a 2017 Bitcoin fork with a genuine, long-lived
  **social/holder community** (listed on CoinMarketCap, Coinbase price
  pages, Forbes; active Telegram/Discord/Reddit/VK, LIMXTEC's
  `awesome-bitcore-btx`). *This* is the "big community" — a token/brand
  following built over ~9 years.
  ([CMC](https://coinmarketcap.com/currencies/bitcore/) ·
  [Coinbase](https://www.coinbase.com/price/bitcore) ·
  [Telegram](https://t.me/s/bitcore_btx_official))
- **BTX Chain (btxchain/btx)** — the new post-quantum chain. Its
  **developer** community is tiny: **32 stars, 13 forks, 1 PR, 8 open
  issues** on GitHub. The PQ chain appears to be the BitCore lineage's
  reinvention, inheriting the brand/holders — not a new dev ecosystem.

**Implication for us:** the threat is **distribution and brand**, not code
velocity or an ecosystem of builders. The learnable is the same lesson
Tempo/Robinhood taught (G3 in the matrix): *a pre-existing audience is a
moat we don't have*. We will not out-community a 9-year token brand from a
standing start; we compete on being the thing their community's own
premise ("post-quantum, settlement") is actually asking for, done as a
real BFT settlement layer.

---

## 3. Head-to-head

| Axis | BTX Chain | suwappu-dag | Read |
|---|---|---|---|
| Chain shape | Bitcoin-Knots fork, UTXO, **Nakamoto PoW** | Mysticeti-C certificate-DAG, **dual-ring joint-quorum BFT** | Different animals |
| Finality | **Probabilistic**, 90 s blocks, reorg-able (no finality semantics in docs) | **Deterministic** DAG-commit; fast-path sub-second (design) | **AHEAD** — probabilistic 90 s settlement is a weak institutional claim |
| Safety model | Single PoW chain; 51%-reorg is the threat model | Fork needs Byzantine capture of **both** rings (Theorem 2) | **AHEAD** |
| PQ signatures | ML-DSA-44 (NIST L2) + SLH-DSA-128s backup, **from genesis, live** | ML-DSA-65 (NIST L3) on primary surfaces | **AHEAD on tier**, BEHIND on "live" |
| PQ confidentiality | SMILE v2 lattice CT **from genesis — but disabled at block 125k** | CONF-1 / Track H in flight (ML-KEM-768 + AEAD), unshipped | **Contested**: they shipped-then-retreated; we've shipped nothing yet |
| Formal verification | **Module-SIS reduction, 21 machine-checked obligations** on shielded soundness | proptests (10k-case exit gates); no machine-checked proofs | **BEHIND** — genuinely |
| Programmability | Covenants (CTV/CSFS) on a Bitcoin script base | Intent envelope + precompiles (DID, issuer, reserve-coverage) | Different surface; parity-ish |
| Fees / stablecoin / issuer | None (fixed 21M, no fee abstraction, no issuer) | FEE-1 sponsorship + registered-issuer + reserve-coverage precompiles | **AHEAD** |
| Throughput | ~36 TPS mixed (self-reported, honest) | 100 TPS submission demo; committed-TPS run pending | Comparable order; both honest |
| "Useful work" narrative | **MatMul PoW = AI-aligned productive security** (crisp, memorable) | none equivalent | **BEHIND on narrative**, N/A on mechanism |
| Live status | **Mainnet since 2026-03-19** | Public devnet | **BEHIND** |
| Community | Inherited 9-yr BitCore holder/brand base | none | **BEHIND** (business, not eng) |

---

## 4. Where we compete — and win

1. **Deterministic joint-quorum finality vs probabilistic PoW.** BTX
   *says* "settlement" but delivers Nakamoto probabilistic finality on 90 s
   blocks — reorg-able by construction, no finality semantics in the docs.
   For the regulated-settlement buyer this is the decisive difference: our
   dual-ring AND-gate (Theorem 2) gives deterministic, non-reverting
   commits. **When BTX borrows our sentence, this is the clause they can't
   back.** Lead with it.
2. **PQ parameter tier + no walk-back.** They run ML-DSA-44 (NIST L2); we
   run ML-DSA-65 (L3). More importantly, their PQ *confidentiality* was
   **disabled at block 125k** — the moment we ship CONF-1 Phase 2 we hold a
   *standing* PQ confidential path they retired. That reframes D1 from
   "behind" to "last one standing," if we ship.
3. **Payments/settlement surface they don't have.** Fee abstraction,
   stablecoin-denominated fees, registered-issuer + reserve-coverage
   precompiles — none exist on a fixed-supply Bitcoin clone. Our lane (FEE-1,
   C1) is simply not on their map.

## 5. What to learn / steal (concrete)

1. **A single, memorable "why our security is productive" hook.** MatMul-PoW
   → "the hardware securing the chain is the same class that trains AI" is a
   *genuinely* good line — one sentence, technically grounded, sticky. We
   don't have PoW and shouldn't add it, but we lack an equivalent crisp hook.
   Ours is available and stronger: **"every long-lived surface is
   post-quantum *and* a fork needs two independent rings to collude"** — but
   it isn't compressed to a single memorable line anywhere in our copy. **→
   GTM: write the one-liner.** (Feeds the explorer Capabilities page and the
   GTM-1 kit.)
2. **Publish machine-checked proofs, not just proptests.** BTX ships a
   "reduction of forgery hardness to Module-SIS — 21 machine-checked
   obligations" for shielded soundness, runnable via `run_all.py`. Our
   10k-case proptests are strong but *empirical*. A machine-checked
   obligation for the **joint-quorum AND-gate (Theorem 2)** and/or the
   **LTP value/soundness** relation would be a category-defining
   institutional artifact and directly answers a formal buyer.
   **→ Candidate IQ: "Machine-checked obligations for Theorem 2 + LTP
   soundness"** (scope: pick a proof assistant / bounded-model tool,
   target the two load-bearing invariants, publish a `run_all` entry point).
3. **Formal, versioned spec docs as a shipped artifact.** Their MatMul-PoW
   spec, PQ guides, shielded-ops handbook, and security-audit docs are part
   of the repo and referenced from marketing. Our IQ/architecture docs are
   good but internal; surfacing a curated **/spec** set (paper §-mapped)
   raises the institutional-credibility floor cheaply. **→ low-cost doc
   task**, complements the Capabilities page we just shipped.
4. **Covenant-style constrained spends (CTV/CSFS) for custody/oracles** are
   worth a *comparison note* against our precompile approach — not to copy
   (we're not Bitcoin-script), but because "vaults + oracle signatures" is a
   concrete institutional-custody ask the buyer will raise. Note how our
   Intent + DID/issuer precompiles cover the same jobs.

## 6. Don't chase

- **MatMul PoW / useful-work mining.** We are BFT-DAG, not PoW. Adding a
  mining layer contradicts the dual-ring validator model and Invariant 1.
  Admire the narrative; do not import the mechanism.
- **UTXO / Bitcoin-script / covenants as our programmability model.** Our
  write path is the Intent envelope; re-basing on script reintroduces a
  surface we deliberately don't have.
- **Halving / fixed-supply mining economics.** Out of segment.
- **Out-communitying the BitCore brand.** Not winnable head-on; win on
  substance the community's own premise implies.

## 7. Messaging-collision risk (act on this)

BTX's copy and ours are close enough to be confused by a skim-reading
buyer or journalist: both say "post-quantum," "settlement," "machine-verifiable,"
"neutral," "autonomous and institutional participants," "without
administrators," "narrow base layer." **We must not let "post-quantum
settlement chain" become a genericized, BTX-shaped category in the reader's
head.** The differentiator sentence, everywhere:

> Post-quantum is table stakes now — three chains claim it. suwappu-dag is
> the only one where **settlement is *deterministic and joint-quorum-safe***:
> a fork requires two independent validator rings to collude, and commits
> never revert. BTX and QoreChain are post-quantum *Nakamoto/BFT-single-set*
> chains; we are post-quantum *dual-ring settlement*.

## 8. Bottom line

BTX is the **closest positioning-twin** we've found and a **real live
mainnet** with an **inherited community** — three things worth respecting.
But under the shared sentence it is a **Bitcoin-Knots PoW fork** with
**probabilistic 90 s finality**, **NIST-L2 PQ**, **confidential transactions
it has already switched off**, and **no payments/issuer surface**. We are
behind on **live status, community, and (genuinely) machine-checked proofs**;
we are ahead on **finality determinism, safety model, PQ tier, a standing
confidential path (once CONF-1 ships), and the whole fee/issuer lane**. The
move is not to answer MatMul-PoW — it's to (a) sharpen the one-line
"deterministic joint-quorum" differentiator so we don't get folded into a
BTX-shaped "PQ chain" bucket, (b) match their **formal-verification** bar on
our two load-bearing invariants, and (c) ship **CONF-1** so that "PQ
confidential transfers" is a thing we *have* and they *retired*.
