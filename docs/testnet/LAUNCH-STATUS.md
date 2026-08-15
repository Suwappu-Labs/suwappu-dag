# Testnet launch status

Working tracker for getting the SUWAPPU DAG testnet live and externally
joinable, and for re-legging the LTP bridge against it. Grounded in a
2026-08-15 cross-repo audit (suwappu-dag, suwappu-lattice-protocol,
suwappubot). Update this file as items close; delete it when the testnet
is live and `VALIDATOR-OPERATORS.md` describes reality.

## Ground truth (2026-08-15)

- **No network is live.** The devnet endpoints in `DEVNET.md` and the
  testnet endpoints in `VALIDATOR-OPERATORS.md` do not exist yet; zero
  releases have been cut (`git tag` is empty).
- The original lattice-protocol "SUWAPPU testnet" (chain `103115120`,
  the PoA EVM devnet recorded in that repo's `DEPLOYED_CONTRACTS.md`) is
  **offline** — its RPC hostname no longer resolves. The LTP bridge's
  Base Sepolia leg is live and verified on-chain (registry v6). This
  chain is the natural replacement for the dead leg.
- `terraform/testnet/` is substantially complete (7-region seeds, ALB,
  ACM, DNS, WAF, faucet, explorer, status, points-program RDS) and is
  more complete than its README previously claimed.
- SuwappuBot's integration contract is
  `suwappubot/docs/pq-settlement-profile.md`: a quote-only, default-off
  bridge provider may merge dark; activation is gated on a live chain,
  real bridge contracts, conformance vectors, and an observed end-to-end
  testnet transfer.

## Fixed on this branch (2026-08-15)

- Genesis funding: `GenesisManifest` gained a `prebalances` section
  applied deterministically at genesis; `gen-genesis.py` embeds it into
  `genesis.toml`. Previously the generated `prebalances.toml` was
  consumed by nothing and the faucet started with zero balance.
- Faucet address derivation reconciled between `gen-genesis.py` and
  `suwappu-faucet` (they disagreed: blake2b vs blake3 — the funded
  address was not the address the faucet spent from).
- `release.yml` now builds and packages `suwappu-keygen` and
  `suwappu-validator-program` (operators previously could not obtain
  either).
- Container image publishing to GHCR (`docker.yml`) with deploy-key
  forwarding, so outsiders can run a node without building against the
  private `suwappu-db` dependency.
- Domain defects fixed: `*.testnet.suwappu.suwappu.bot` /
  `*.devnet.suwappu.suwappu.bot` doubles in terraform + status page;
  `status-testnet.yml`'s unconditionally-failing sed; CloudWatch
  namespace no longer pinned to `suwappu-devnet` for testnet nodes.
- Doc corrections: wrong terraform module paths, wrong release download
  glob, "forthcoming" claims about the already-implemented points
  daemon, stale terraform READMEs.

## Remaining code gaps (in-repo, non-trivial)

Ordered by how hard they block "others able to join":

1. **Late join.** `GenesisManifest::validate_against`
   (`crates/suwappu-node/src/config.rs`) rejects any `authority_id` not
   in genesis, and there is no state sync — a validator admitted
   post-genesis via `AdmitAuthority` (ids ≥ 8 per
   `VALIDATOR-OPERATORS.md`) cannot boot. Needs: manifest validation
   that tolerates post-genesis members + a catch-up path.
2. **Peer discovery.** `peers` is a static TOML list; seeds only learn
   of a new node when a human re-renders configs and restarts all seven.
   Needs: dynamic peer registration, or an automated seed-side reconfig
   triggered by the admit intent.
3. **Single-signature governance admit.**
   `crates/suwappu-node/src/client.rs` accepts ANY one seated
   authority's signature for `AdmitAuthority`/`EjectAuthority` — one
   compromised seed can reshape the validator set. Dual-signature (or
   quorum) admit must land before any external party is seated.
4. **Validator Ring ≠ Authority Ring.** `AdmitAuthority` mirrors the
   same identity into both registries; the paper's open PoS Validator
   Ring has no join path. Either implement it or present the testnet as
   single-ring PoA and reconcile `VALIDATOR-OPERATORS.md`.
5. **Corridor daemon (lattice repo).** `src/ltp/corridor/` is a
   byte-parity library; there is no membership registry, PoP exchange,
   or partial-signature transport for the 7-of-9 super-node quorum, and
   no relayer transport (`Relayer.relay()` returns an in-process
   object). "Joining a corridor" is currently a human arrangement.

## Requires human action (cannot be done from a repo)

1. AWS: profile `gsn` (account 492042618949), terraform bootstrap state,
   `BILLING_ALARM_EMAIL`, operator SSH key + CIDR. Then
   `scripts/testnet/deploy.sh` (≈$672/mo seeds + RDS/ALB, $2k/mo alarm).
2. DNS: delegate `suwappu.bot` NS into the account (one-time manual step
   gating ACM validation → all TLS).
3. Cut the first release (`suwappu-dag-v*` tag) so binaries and the GHCR
   image exist; GitHub secrets `SUWAPPU_DB_DEPLOY_KEY`,
   `STATUS_TESTNET_DEPLOY_ROLE`, `EXPLORER_TESTNET_DEPLOY_ROLE`.
4. Decide `suwappu-db` visibility: while private, external source builds
   are impossible and the GHCR image is the only outsider path.
5. Genesis ceremony: run `scripts/testnet/gen-genesis.py` with real keys
   (`suwappu-keygen` on PATH — placeholder fallback keys cannot sign),
   publish `genesis.toml` + `peers.txt` at the documented URLs.
6. Program scaffolding: apply form, Discord, leaderboard host, and a
   decision on the points→token conversion claim
   (`POINTS.md` vs the deferred-token architecture).
7. Performance go/no-go: `.sprint-state.md` records 0.125 TPS p50
   against a 1–5k target with S31 partially landed. An incentivized
   points-per-cert testnet at that throughput is self-defeating —
   finish S31 or launch un-incentivized first.
8. Bridge re-legging (lattice repo): deploy `LTPAnchorRegistry` (+
   bridge pair) on this chain once live, regenerate + register the
   gateway keypair (2-of-2 multisig ceremony), fund operators, then run
   `scripts/bridge_live.py` end-to-end. Until the v7 contracts deploy,
   the bridge trust model remains 2-of-2 discretionary with zero bonds —
   fine for testnet demonstration, not value-bearing settlement (that
   repo's `BRIDGE_TRUST_MODEL.md` says the same).
9. SuwappuBot activation: keep `lattice_bridge_enabled=false` until the
   seven gates in `suwappubot/docs/pq-settlement-profile.md` pass,
   finishing with an observed end-to-end testnet transfer.

## Sequence to a public (foundation-operated) testnet

Genesis funding + keygen shipping (done on this branch) → cut
`v0.1.0` → GHCR image publishes → human: AWS + DNS + terraform apply →
genesis ceremony + publish genesis/peers → verify 7-seed mesh, faucet,
explorer, status → announce. External validators join only after gaps
1–3 above close; the corridor/bridge re-legging (8) can proceed in
parallel once RPC is stable.
