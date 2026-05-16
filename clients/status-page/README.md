# `@gsx/status-page` — gsx-devnet status page

Single-page vanilla HTML+JS app that surfaces devnet liveness at
`https://status.devnet.gsx.globalsettlement.com/`. Designed to be
glance-able from a phone during an incident — no framework, no
build step, ~250 LOC total.

## What it shows

- **Overall state**: green / yellow / red.
  - **Green**: chain tip advancing AND faucet up.
  - **Yellow**: chain advancing but faucet down (devs can read but
    not drip), OR tip flat <60s (warming up).
  - **Red**: tip flat >60s, OR RPC unreachable.
- **Chain head**: `latest_committed_round` + current epoch + last
  boundary + rounds-per-epoch.
- **Cluster tip tile**: cluster-wide `latest_committed_round` (via
  the ALB; whichever validator answers fastest).
- **Faucet tile**: `/health` up/down.

Polls `https://rpc.devnet.gsx.globalsettlement.com/gsx_getEpoch`
and `https://faucet.devnet.gsx.globalsettlement.com/health` every
5 seconds. No state stored anywhere — refresh the page for a
clean slate.

## NOT in this v0.1

- Per-region tip tiles. Needs a public proxy in front of
  CloudWatch's `GetMetricData` (G6's metrics land there). Without
  that, the page can only see the ALB-balanced cluster tip.
  Tracked as a follow-up.
- Historical uptime graph. Needs persistent data; deferred.
- Incident timeline / postmortem markup. Deferred to a hand-
  edited section once the devnet has actually had any incidents.

## Local development

```sh
cd clients/status-page
python3 -m http.server 8000   # http://localhost:8000
```

The page hits the public devnet directly — no local backend
needed.

## Deployment

Same pattern as the explorer (G7): S3 + CloudFront + Route53,
deployed by `.github/workflows/status.yml` on push to main.
Terraform in `terraform/devnet/status.tf`.

## See also

- [`../explorer/`](../explorer/) — companion block explorer.
- [`../../DEVNET.md`](../../DEVNET.md) — devnet quick-start.
- [`../../OPERATIONS.md`](../../OPERATIONS.md) — runbooks; if the
  overall state goes red, that's where to look first.
