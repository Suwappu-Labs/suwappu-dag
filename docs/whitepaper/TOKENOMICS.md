# SUWP Tokenomics — Genesis Pre-Mine, Fair-Launch Distribution

> **Status: adopted 2026-08-24.** This supersedes the token-deferral stance of
> [`suwappu-lattice-protocol/docs/economics/DEFERRED_TOKEN_ARCHITECTURE.md`](https://github.com/Suwappu-Labs/suwappu-lattice-protocol/blob/main/docs/economics/DEFERRED_TOKEN_ARCHITECTURE.md):
> SUWP launches with the chain rather than after it. Everything that document and
> [`UNIFIED_TOKENOMICS.md`](https://github.com/Suwappu-Labs/suwappu-lattice-protocol/blob/main/docs/economics/UNIFIED_TOKENOMICS.md)
> settled that is *not* about deferral still stands: SUWAPPU = SUWP is one asset
> (the dag chain's native gas/stake unit), max supply is 1,000,000,000, and the
> Seasons program's 30% commitment is honored unchanged.

## 1. The model in one paragraph

The entire fixed supply of **1,000,000,000 SUWP** is minted in the mainnet
genesis block — that is the pre-mine, and it is the *only* issuance event the
chain will ever have. This is enforced in code, not just promised: the genesis
manifest commits `max_supply_suwappu = 1_000_000_000`, the node seals an
on-chain **supply ledger** with it at height 0, and `Intent::MintInflation`
fail-closes against that ledger (`ExecutionError::MaxSupplyExceeded`). Because
the pre-mine equals the cap, every post-genesis mint fails by construction.
Distribution is a **fair launch**: there is **no team allocation, no investor
or private-sale tranche, and no foundation treasury carve-out**. 100% of
supply sits in protocol-owned pools, each distributed through an open,
published program, each at an address anyone can derive and audit.

## 2. Genesis allocation

Source of truth: [`scripts/tge/allocations.toml`](../../scripts/tge/allocations.toml),
validated by `python3 scripts/tge/gen-tge-prebalances.py --check` (sums must
equal supply exactly; addresses must match their domain tags).

| Pool | % | SUWP | Address (BLAKE3(tag)[:20]) |
|---|---:|---:|---|
| Fair-launch distribution | 42% | 420,000,000 | `0xf9e86688d4afeeff73b01067237e5529149905f0` |
| Seasons program | 30% | 300,000,000 | `0xae360caae624555b7fc6a2b7a96def76780d9e43` |
| Staking rewards (Authority Ring) | 10% | 100,000,000 | `0x1148457e50ba9ee1b9197e98dd0efc096063a50c` |
| Staking rewards (Validator Ring) | 10% | 100,000,000 | `0xef9bd42745ebdf4dbcb15e21426a670efbc407f5` |
| Testnet points | 8% | 80,000,000 | `0x9e28b89b1c3b49a75f3b782e0ac9ee5919b340f9` |
| **Total** | **100%** | **1,000,000,000** | |

Pool addresses use the chain's reserved-address scheme
(`crates/suwappu-execution/src/reserved.rs`): the leading 20 bytes of BLAKE3 of
a pinned domain tag, registered in `is_reserved` so ordinary `Intent::Transfer`
into or out of any pool is rejected at execution — no key custody stands
between genesis and the published programs. A Rust unit test
(`tge_pool_addresses_match_published_ledger`) pins each address above
byte-for-byte against the Python tooling's derivation, so ledger and chain
cannot drift apart silently.

### 2.1 Fair-launch distribution — 42%

Open public distribution at TGE. Pro-rata against public participation; no
allowlist, no private tranche, no price discrimination. Mechanism details
(LBP vs. fixed-window claim) are an implementation decision to publish
≥ 30 days before TGE; the *constraints* (open to all, uniform terms) are fixed
here and are not implementation details.

### 2.2 Seasons program — 30%

The existing commitment in
[`suwappubot/docs/economics/SEASONS_TOKENOMICS.md`](https://github.com/0xSoftBoi/suwappubot/blob/main/docs/economics/SEASONS_TOKENOMICS.md),
unchanged: `A = 300,000,000` over 8 seasons, geometric decay `δ = 0.75`,
fee-denominated points, revenue-capped emission, 40/60 vesting. The Seasons
pool address above is where that program draws from — the program was already
designed as a drawdown of a genesis allocation, and this document funds it.

### 2.3 Staking rewards — 20% (10% per ring)

Validator/authority rewards are a **drawdown of two finite pools, not
inflation** — and the pools are the chain's *existing* per-ring rewards pools
(`authority_rewards_pool_address`, `validator_rewards_pool_address`), pre-mined
at genesis instead of topped up by minting. Distribution therefore needs zero
new code: `Intent::DistributeRewards` already drains them per epoch,
replay-defended per ring, refusing reserved-address recipients. This closes
the reconciliation flagged in `UNIFIED_TOKENOMICS` §2.1: `Intent::MintInflation`
is dead on mainnet — not by policy but because the sealed supply ledger rejects
it. When the pools are exhausted, rewards are fee-funded only.

### 2.4 Testnet points — 8%

Converts testnet validator points per
[`docs/testnet/POINTS.md`](../testnet/POINTS.md). The 8% pool is the *ceiling*
of that document's [5%, 8%] range; the board still pins the final percentage
≥ 90 days before TGE. The gap between the pinned percentage and 8%, plus any
balances unclaimed 180 days after TGE, is **burned** — not redirected.

## 3. What "fair launch" is promised to mean

These are commitments, checkable against the genesis block itself — and each
is enforced by a specific mechanism, not by policy:

1. **No insider supply.** Zero tokens to team, investors, advisors, or a
   foundation treasury at genesis. Contributors acquire SUWP the same ways
   the public does. *(Checkable: the genesis `[[prebalances]]` contain only
   the five pool addresses above, each re-derivable from its domain tag.)*
2. **No hidden issuance.** Supply is exactly the sum of the published
   `[[prebalances]]`. *(Enforced in depth: `gen-tge-prebalances.py`
   refuses a ledger that doesn't sum to the cap; `GenesisManifest::from_path`
   refuses a manifest whose prebalances exceed `max_supply_suwappu` — and
   rejects unknown fields outright, so an outdated binary fails loudly
   instead of silently forking; `State::new` re-verifies the sums, refuses
   to boot on any skipped prebalance, and seals the ledger exactly once;
   after the seal, `Intent::MintInflation` fails with `MaxSupplyExceeded`,
   `Intent::GenesisAllocation` is rejected outright (round-0 blocks cannot
   re-open genesis), and `Intent::DistributeSlashedFunds` — the one other
   credit-without-debit arm — is held under the same cap.)*
3. **No pre-launch transfers.** *(Enforced: every pool address is in
   `reserved::is_reserved`, so `Intent::Transfer` from or to a pool is
   rejected by both substrate implementations.)*
4. **Testnet ≠ mainnet.** Testnet SUWAPPU balances (faucet-issued) are
   worthless and do not convert; only testnet *points* convert, per §2.4.

**The claim path is implemented — as the pattern live chains actually use.**
The fair-launch, Seasons, and testnet-points pools distribute through the
**MerkleDistributor mechanism** (`crates/suwappu-execution/src/tge_claim.rs`):
`Intent::SetTgeRoot` publishes a distribution round's Merkle root
(governance-gated — sponsor **plus a second, distinct seated authority**,
the same dual-signature wire rule as validator-set changes), and
`Intent::TgeClaim` is **permissionless**: anyone may submit a proof, funds
move pool → the leaf's committed account, each index claims once per round,
and a round can never pay out more than the pool holds. Claims are
transfers, not issuance — the sealed supply ledger is untouched. Rotating
the root starts a fresh round (the analogue of deploying a new distributor
per drop, which is how the Seasons schedule runs). Remaining before TGE:
an external audit of this path and the public root-publication ceremony —
tracked in `docs/testnet/LAUNCH-STATUS.md`. The staking pools distribute
via `Intent::DistributeRewards` instead.

## 4. Operational path to TGE

1. Ledger frozen in this repo (`scripts/tge/allocations.toml`) — any change
   after publication is a governance event, not an edit.
2. Mainnet genesis ceremony: run the ledger tool, embed the emitted fragment
   (`max_supply_suwappu` + the five `[[prebalances]]`) in the mainnet
   `genesis.toml` alongside the seed validator set
   (`docs/releasing-mainnet.md` owns the ceremony). The loader refuses the
   manifest if the fragment was tampered past the cap.
3. TGE claim path: implemented (`Intent::SetTgeRoot` / `Intent::TgeClaim`,
   §3) — remaining is an external audit plus the public root-publication
   ceremony: each round's full `(index, account, amount)` set is published
   before its root is set, so anyone can rebuild the root and their own
   proof (exactly how live airdrop distributors publish their trees).
4. Fair-launch mechanism spec published ≥ 30 days pre-TGE; testnet-points
   percentage pinned ≥ 90 days pre-TGE.
5. Third-party verification: anyone re-derives the five addresses from their
   domain tags and re-sums the ledger against the live genesis block.

## 5. Alignment with live chains and EIPs

This design is deliberately grounded in mechanisms that are running in
production with real value at stake, not in papers:

- **Distribution = MerkleDistributor.** The claim mechanism ports Uniswap's
  `MerkleDistributor` — verified and live on Ethereum mainnet at
  [`0x090D4613473dEE047c3f2706764f49E0821D256e`](https://eth.blockscout.com/address/0x090D4613473dEE047c3f2706764f49E0821D256e)
  since 2020-09-16 — the same shape Arbitrum's `TokenDistributor` and the
  Optimism airdrops used. Divergences are documented in
  `crates/suwappu-execution/src/tge_claim.rs`: SHA3-256 with distinct
  leaf/node domain tags (the chain's PQ-conservative hash surface; the
  domain split structurally prevents the leaf/internal-node second-preimage
  splice that Solidity distributors handle by OpenZeppelin-style double
  hashing), sorted-pair nodes per OpenZeppelin `MerkleProof`, and root
  rotation instead of one contract per drop.
- **Fee policy direction = EIP-1559.** The chain has no fee market yet
  (priority is a constant at the intent wire; the fee surface is scheduled
  work). When it lands, the committed direction is EIP-1559-shaped: a
  protocol base fee that is **burned**, priority tips to the proposer. With
  a fixed pre-mined supply and zero issuance, burn makes SUWP net-
  deflationary under load — the post-merge ETH dynamic, without an
  issuance offset. Burn accounting will extend the supply ledger
  (`issued` down, never up); no new supply path is created.
- **EVM-side SUWP = plain ERC-20.** On EVM chains SUWP travels through the
  existing bridge surface (`SuwappuDagQuorumHeaderOracle` /
  `SuwappuDagValidatorRegistry` in `suwappu-revm`) as a standard ERC-20 —
  no bespoke token standard.
- **Airdrop hygiene from observed history.** Per-operator caps and KYC in
  the points program (`docs/testnet/POINTS.md`), vesting + revenue-capped
  emission in Seasons — parameters chosen against the live airdrops'
  outcomes (ARB/OP/Jito/HYPE), as those documents already record.
