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

Gaps 1–3 below were closed on this branch (2026-08-15):

1. ~~**Late join.**~~ CLOSED: `allow_post_genesis_join` boots a
   non-genesis node in passive-sync mode; the new wire sync protocol
   (`GetTip`/`GetCertsByRound`/`GetBlock`) backfills forward through
   the ordinary verified ingest path, and authoring/voting begin only
   once the node observes itself seated. Caveat: catch-up replays from
   peers' in-memory history — nodes still have no persistence, so a
   joiner can only sync back to what its peers have held since their
   own boot. Snapshot/checkpoint sync remains future work.
2. ~~**Peer discovery.**~~ CLOSED (minimally): seeds now accept up to 64
   dynamic inbound peers full-duplex on the joiner's own connection —
   no seed config edit or restart needed for a joiner to sync and, once
   seated, to submit certs. Adding the joiner to seeds' static configs
   (for push gossip toward it) can happen at the next convenient
   restart; until then it tails via pull.
3. ~~**Single-signature governance admit.**~~ CLOSED at the ingress
   wires: `AdmitAuthority` now requires a seated sponsor plus the
   candidate's proof-of-possession co-signature; Exit/Eject require two
   distinct seated authorities (client wire v3). Residual risk: a
   Byzantine seated AUTHORITY can still embed intents directly in its
   own authored blocks, bypassing client ingress — block-level intent
   auth is tracked below.
4. ~~**Block-level governance intent authentication.**~~ CLOSED
   (IQ-007): governance intents now carry an on-chain `GovAuth` envelope
   (sponsor + second distinct seated authority + candidate PoP) in the
   `BlockPayload`, re-verified deterministically at the epoch-boundary
   apply against each node's own seated registry. No single key — client
   or node — can reshape the validator set; it takes two distinct seated
   authorities. Residual (future): stake is still a claimed integer, not
   an escrowed bond.
5. **Validator Ring ≠ Authority Ring.** `AdmitAuthority` mirrors the
   same identity into both registries; the paper's open PoS Validator
   Ring has no join path. Either implement it or present the testnet as
   single-ring PoA and reconcile `VALIDATOR-OPERATORS.md`.
6. **Corridor daemon (lattice repo).** `src/ltp/corridor/` is a
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
explorer, status → announce. The external-validator join path (late
join + dynamic peers + dual-sig admit) landed on this branch; before
seating a real third party, close gap 4 (block-level governance intent
auth) and re-verify the flow end-to-end on the live testnet. The
corridor/bridge re-legging (8) can proceed in parallel once RPC is
stable.
