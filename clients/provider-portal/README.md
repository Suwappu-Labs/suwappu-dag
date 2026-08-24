# `@suwappu/provider-portal` — Suwappu Compute provider portal

Single-page vanilla HTML+JS app (same conventions as
`clients/status-page`: no framework, no build step) that is the
**product front door for compute providers** — the Suwappu equivalent
of Akash's "become a provider" surface, with one structural
difference in the pitch: on Akash a provider hunts for tenants in a
reverse auction; here **the protocol itself is the buyer**. Validator
work (certificates, attestations, storage, DA serving) is metered by
proofs and paid per epoch.

Intended deployment: static hosting behind
`compute.suwappu.bot` (S3 + CloudFront, like the explorer), but the
page runs from any static server.

## Product scope (v0.1)

What a prospective provider gets, in order:

1. **Positioning** — get paid in reserve-backed stablecoins for
   post-quantum infrastructure work; the four provider-facing trust
   points (protocol-as-customer, dollar-denominated, provably backed,
   proof-gated for everyone).
2. **Role table** — DAG validator vs LTP commitment node, and what
   each is paid for.
3. **Earnings calculator**, two modes:
   - *Testnet points (live now)* — implements the published formula
     from `docs/testnet/POINTS.md` verbatim (uptime tier 100/50/0,
     certs/1000 capped at 50, commits capped at 30, ~17-minute
     epochs) and states the 5–8%-of-supply TGE conversion and the 2%
     per-operator cap.
   - *Mainnet stablecoin (preview)* — mirrors the
     `ltp.incentives.IncentiveConfig` defaults ($0.02/GiB-month,
     $0.01/GiB served) and uses clearly-labeled placeholder values
     for the governance-set `RewardParams` (per-certificate,
     per-attestation). Labeled illustrative everywhere; "a model, not
     an offer."
4. **"How the pay stays honest"** — proof-gated, reserve-backed
   (coverage breaker at post-mint supply), budget-capped, bond-secured.
   These map 1:1 to the mechanisms in
   `suwappu-precompiles::rewards` / `reserve::mint_with_coverage` and
   `suwappu-lattice-protocol` `ltp/incentives.py`.
5. **Join steps** — hardware, devnet sync (`DEVNET.md`), points-program
   KYC + leaderboard, TGE conversion, mainnet epochs (badged "next").
6. **Live tiles** — polls `rpc.devnet.suwappu.bot/suwappu_getEpoch`
   every 15 s for tip/epoch; degrades to "unreachable from here"
   without breaking the page (the calculator works fully offline).

## NOT in v0.1 (deliberate)

- **No wallet connection / registration flow in-page.** Admission is
  the points-program KYC path today (`docs/testnet/POINTS.md`); an
  in-page application flow needs the admission service that
  `suwappu-lattice-protocol`'s external-validator-onboarding plan
  (Phase 2) describes, which doesn't exist yet.
- **No live earnings from the leaderboard API.** Wiring
  `validator-program`'s leaderboard into a per-operator earnings view
  is the obvious v0.2 once CORS is opened on that endpoint.
- **No mainnet rate table.** `RewardParams` are governance-set and
  have no ratified numbers; publishing a table now would just get
  screenshotted as a promise. The calculator labels them placeholder
  instead.
- **No GPU marketplace.** Suwappu buys validator work, not general
  compute; an Akash-style tenant-side marketplace is a different
  product and out of scope.

## Local development

```sh
cd clients/provider-portal
python3 -m http.server 8000   # http://localhost:8000
```

No dependencies, no build. Keep it that way as long as possible.
