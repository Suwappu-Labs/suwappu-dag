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
chain will ever have: the mainnet genesis manifest carries no inflation budget,
so `Intent::MintInflation` cannot create supply beyond it. Distribution is a
**fair launch**: there is **no team allocation, no investor or private-sale
tranche, and no foundation treasury carve-out**. 100% of supply sits in four
protocol-owned pools, each distributed through an open, published program, each
at an address anyone can derive and audit.

## 2. Genesis allocation

Source of truth: [`scripts/tge/allocations.toml`](../../scripts/tge/allocations.toml),
validated by `python3 scripts/tge/gen-tge-prebalances.py --check` (sums must
equal supply exactly; addresses must match their domain tags).

| Pool | % | SUWP | Address (BLAKE3(tag)[:20]) |
|---|---:|---:|---|
| Fair-launch distribution | 42% | 420,000,000 | `0xf9e86688d4afeeff73b01067237e5529149905f0` |
| Seasons program | 30% | 300,000,000 | `0xae360caae624555b7fc6a2b7a96def76780d9e43` |
| Staking rewards | 20% | 200,000,000 | `0x1749c422bc9da089f43ef0c3628b5344c35fba12` |
| Testnet points | 8% | 80,000,000 | `0x9e28b89b1c3b49a75f3b782e0ac9ee5919b340f9` |
| **Total** | **100%** | **1,000,000,000** | |

Pool addresses use the chain's reserved-address scheme
(`crates/suwappu-execution/src/reserved.rs`): the leading 20 bytes of BLAKE3 of
a pinned domain tag. Reserved addresses reject ordinary `Intent::Transfer`, so
pool funds can only move through the dedicated distribution paths — no key
custody stands between genesis and the published programs.

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

### 2.3 Staking rewards — 20%

Validator/authority rewards are a **drawdown of this finite pool, not
inflation**. This closes the reconciliation flagged in `UNIFIED_TOKENOMICS`
§2.1: open-ended per-epoch `Intent::MintInflation` is retired for mainnet;
when the pool is exhausted, rewards are fee-funded only. Per-epoch drawdown
schedule is set with mainnet genesis parameters.

### 2.4 Testnet points — 8%

Converts testnet validator points per
[`docs/testnet/POINTS.md`](../testnet/POINTS.md). The 8% pool is the *ceiling*
of that document's [5%, 8%] range; the board still pins the final percentage
≥ 90 days before TGE. The gap between the pinned percentage and 8%, plus any
balances unclaimed 180 days after TGE, is **burned** — not redirected.

## 3. What "fair launch" is promised to mean

These are commitments, checkable against the genesis block itself:

1. **No insider supply.** Zero tokens to team, investors, advisors, or a
   foundation treasury at genesis. Contributors acquire SUWP the same ways
   the public does.
2. **No hidden issuance.** Supply is exactly the sum of the published
   `[[prebalances]]`; the genesis manifest is public and the ledger tool
   re-verifies it byte-for-byte.
3. **No pre-launch transfers.** Pool addresses are reserved; nothing moves
   before the published programs activate.
4. **Testnet ≠ mainnet.** Testnet SUWAPPU balances (faucet-issued) are
   worthless and do not convert; only testnet *points* convert, per §2.4.

## 4. Operational path to TGE

1. Ledger frozen in this repo (`scripts/tge/allocations.toml`) — any change
   after publication is a governance event, not an edit.
2. Mainnet genesis ceremony: run the ledger tool, embed the emitted
   `[[prebalances]]` fragment in the mainnet `genesis.toml` alongside the
   seed validator set (`docs/releasing-mainnet.md` owns the ceremony).
3. Fair-launch mechanism spec published ≥ 30 days pre-TGE; testnet-points
   percentage pinned ≥ 90 days pre-TGE.
4. Third-party verification: anyone re-derives the four addresses from their
   domain tags and re-sums the ledger against the live genesis block.
