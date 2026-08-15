# Non-AWS testnet deploy (free tier)

Replaces the AWS `terraform/testnet/` path (no longer available — see
`LAUNCH-STATUS.md`). Stands up a foundation-operated public testnet at
**$0/month** using free-forever infrastructure. Nothing here needs AWS.

## Cost summary

| Piece | Free option | Notes |
|---|---|---|
| Seed validators (compute, 24/7, raw TCP) | **Oracle Cloud "Always Free" Ampere A1** (ARM64) | The only genuinely-free always-on VM with a public IP + open ports. 2 OCPU / 12 GB as of June 2026 (was 4/24). |
| Node image | **GHCR** `ghcr.io/suwappu-labs/suwappu-dag` | Now multi-arch (`amd64`+`arm64`) — runs on the A1 box directly. |
| Binaries | **GitHub Releases** | `aarch64-unknown-linux-gnu` tarball added for the A1 box. |
| `genesis.toml`, `peers.txt` | **GitHub Pages / Releases** | Plain static files. |
| Explorer + status SPAs | **GitHub Pages** (or Cloudflare Pages) | Static builds. |
| DNS zone + TLS | **Cloudflare (free)** | Free zone + free TLS proxy for RPC/faucet HTTP. |
| CI / release / image build | **GitHub Actions** | Existing workflows. |

**Not strictly free:** a domain name (~$1–12/yr; or use a free-subdomain
service / the raw IP). The DNS *zone* is free on Cloudflare.

## Honest caveats before you start

- **One box is not fault-independent.** Running all seeds on a single A1
  VM is fine for a demonstration testnet but collapses the dual-ring
  fault model. For real distribution, provision a second/third Oracle
  tenancy (separate free accounts, ideally different home regions) and
  split the seeds across them. The join path (late-join + dynamic peers)
  works the same across hosts.
- **Node RAM grows unbounded.** The DAG store never prunes and there is
  no persistence (`docs/testnet/LAUNCH-STATUS.md`, gap notes). A
  long-running seed will slowly grow memory — plan a **periodic
  regenesis** (already routine maintenance; `OPERATIONS.md` §10.4). Size
  the A1 box's 12 GB accordingly and watch RSS.
- **ARM "out of capacity."** Oracle frequently rejects A1 provisioning in
  busy regions (US-East). Frankfurt / Singapore usually succeed; a retry
  loop on the create call is the standard workaround.
- **Throughput.** `.sprint-state.md` records ~0.125 TPS p50 vs a 1–5k
  target. Launch **un-incentivized** (or finish S31) before attaching a
  points-per-cert program — see LAUNCH-STATUS item 7.

## 1. Provision the Oracle A1 VM

1. Oracle Cloud → Create Instance → shape **VM.Standard.A1.Flex**
   (Ampere/ARM), 2 OCPU / 12 GB, image **Ubuntu 22.04 (aarch64)**. Assign
   a public IPv4. Save the SSH key.
2. Open the p2p/client/RPC ports. In the subnet **Security List** (or an
   NSG) add ingress `0.0.0.0/0` TCP for **9090** (peer), **9091**
   (client), **9092** (RPC). Then on the box:
   ```bash
   sudo ufw allow 9090,9091,9092/tcp && sudo ufw enable
   # Oracle Ubuntu images also have iptables rules; persist an allow:
   sudo iptables -I INPUT -p tcp -m multiport --dports 9090,9091,9092 -j ACCEPT
   sudo netfilter-persistent save
   ```
3. Install Docker + compose plugin:
   ```bash
   curl -fsSL https://get.docker.com | sh
   sudo usermod -aG docker "$USER"   # re-login
   ```

## 2. Get the node binary/image (ARM64)

Two paths — pick one:

- **Pull the published image (turnkey, for external operators too):**
  ```bash
  docker pull ghcr.io/suwappu-labs/suwappu-dag:latest   # multi-arch; pulls arm64 on the A1 box
  ```
- **Build on the box (foundation only — needs the private `suwappu-db`
  deploy key present in `~/.ssh` + the git rewrite):** native ARM build,
  no emulation:
  ```bash
  git config --global url."git@github.com:".insteadOf "https://github.com/"
  git clone git@github.com:Suwappu-Labs/suwappu-dag.git && cd suwappu-dag
  DOCKER_BUILDKIT=1 docker compose build --ssh default
  ```

External operators (no `suwappu-db` key) **must** use the published image
or the release binary — they cannot build from source. This is why the
multi-arch image / `aarch64` release tarball exist.

## 3. Genesis ceremony (real keys)

`suwappu-keygen` must be on PATH (from the release, or `cargo build
--release -p suwappu-crypto --bin suwappu-keygen`). Placeholder keys
cannot sign — the chain will not produce certificates.

```bash
# Testnet genesis: real ML-DSA/BLS keys, prebalances (funds the faucet),
# per-validator node.toml. Writes to ./out/.
python3 scripts/testnet/gen-genesis.py --out ./out
```

This emits `genesis.toml` (with `[[prebalances]]` — the faucet funding
fix on this line), each validator's `.sk`, and a `node.toml` per seed.
For post-genesis external joiners, their `node.toml` sets
`allow_post_genesis_join = true` (see `VALIDATOR-OPERATORS.md`).

## 4. Run the seeds

Adapt the committed `docker-compose.yml` (a local 4-node cluster) for the
public box: bind **all three ports** on the seed you expose as the RPC
entry point (compose currently host-exposes only 9091/9092 on v0 — add
9090 and expose it on the box's public IP), point the volumes at your
`./out/<vN>` config + the shared `genesis.toml`, and set each node's
`peers` to the box's public IP (not the compose-internal 172.30.x
addresses) so external validators can dial in. Then:

```bash
docker compose up -d
docker compose ps
# readiness:
curl -sX POST http://<box-ip>:9092 -H 'content-type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"suwappu_getEpoch","params":null}'
```

Faucet: run `suwappu-faucet` alongside (it's in the image); it spends
from the genesis-funded faucet address.

## 5. TLS + DNS (Cloudflare free)

- Add your domain to Cloudflare (free plan). Create an `A` record for
  e.g. `rpc.testnet.<domain>` → the A1 public IP, **proxied** (orange
  cloud) → free TLS for the HTTP RPC/faucet.
- The **raw p2p port 9090 cannot go through Cloudflare's HTTP proxy** —
  publish the seeds' `peer` addresses as `<box-ip>:9090` (or a
  `DNS-only`, grey-cloud `A` record) in `peers.txt`.
- Alternative to Cloudflare proxy: run **Caddy** on the box for automatic
  Let's Encrypt TLS in front of 9092/faucet.
- The double-`suwappu` FQDN fixes on this branch apply to whatever
  subdomain scheme you choose.

## 6. Publish genesis, peers, explorer, status

- Commit `genesis.toml` + `peers.txt` to a public repo and serve via
  **GitHub Pages**, or attach to the `v0.1.0` **Release**. Point
  `VALIDATOR-OPERATORS.md`'s URLs at wherever you host them.
- `clients/explorer` and `clients/status-page` are static SPAs — build
  and deploy to **GitHub Pages / Cloudflare Pages** instead of the old
  S3/CloudFront path. Set their RPC/WS URL to `rpc.testnet.<domain>`.

## 7. Cut the release

Tag `suwappu-dag-v0.1.0` → `release.yml` builds the `x86_64` +
`aarch64` Linux + macOS tarballs, `docker.yml` publishes the multi-arch
image. Both are prerequisites for external operators to obtain the node
without the private source.

---

**Result:** a foundation-operated public testnet — RPC, faucet, explorer,
status, and an externally-joinable validator mesh — on free-forever
infrastructure, no AWS. Scale to real geographic distribution by adding
Oracle tenancies; the code path is identical.
