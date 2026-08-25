# `@suwappu/provider-portal` — Suwappu Compute provider portal

Single-page vanilla HTML+JS app (same conventions as
`clients/status-page`: no framework, no build step) that is the
**product front door for compute providers** — the Suwappu equivalent
of Akash's "become a provider" surface, with one structural
difference in the pitch: on Akash a provider hunts for tenants in a
reverse auction; here **the protocol itself is the buyer**. Validator
work (certificates, attestations, storage, DA serving) is metered by
proofs and paid per epoch.

Deployment: `compute.testnet.suwappu.bot` — S3 + CloudFront defined
in `terraform/testnet/compute-portal.tf`, synced by
`.github/workflows/compute-portal-testnet.yml` on pushes to `main`
touching this directory (devnet hostnames in the source are
sed-rewritten to testnet at upload, same scheme as the status page).
The workflow needs the one-time `COMPUTE_TESTNET_DEPLOY_ROLE` repo
secret, and the terraform is applied by an operator via
`scripts/deploy-aws.sh` — until both happen the workflow verifies the
build and skips the deploy. The page also runs from any static server.

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
6. **Live points lookup** — enter an authority id, read the operator's
   real points + breakdown from the public leaderboard API
   (`leaderboard.<env>.suwappu.bot`, the TLS front defined in
   `terraform/testnet/leaderboard-cdn.tf` over the validator-program
   daemon, which stamps CORS on its public routes), plus a TGE share
   estimate honoring the 2%-of-allocation cap. Degrades to a link to
   the leaderboard page when unreachable.
7. **Live tiles** — polls `rpc.devnet.suwappu.bot/suwappu_getEpoch`
   every 15 s for tip/epoch; degrades to "unreachable from here"
   without breaking the page (the calculator works fully offline).

## NOT in v0.1 (deliberate)

- **No wallet connection / registration flow in-page.** Admission is
  the points-program KYC path today (`docs/testnet/POINTS.md`); an
  in-page application flow needs the admission service that
  `suwappu-lattice-protocol`'s external-validator-onboarding plan
  (Phase 2) describes, which doesn't exist yet.
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
