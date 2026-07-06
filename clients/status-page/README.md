# `@suwappu/status-page` — suwappu-devnet status page

Single-page vanilla HTML+JS app that surfaces devnet health and
live performance at `https://status.devnet.suwappu.bot/`.
Designed to be glance-able from a phone during an incident — no
framework, no chart library, no build step. Deployed as static
files. Design tokens mirror the block explorer so the two
surfaces read as one product.

## What it shows

- **Overall status banner**: All systems operational / Degraded /
  Major outage — derived from the component checks + chain
  freshness, shown with an icon **and** a text label (never colour
  alone), `aria-live` for screen readers.
- **Component health grid** (one card per endpoint): reachability,
  latency in ms, and a status pill. What each check actually
  verifies:
  - **JSON-RPC** — a real `suwappu_getEpoch` call whose JSON we
    parse and validate. `Healthy` here means we read a well-formed
    `EpochView` back. This is a genuine health check.
  - **WebSocket** — a short-lived socket; `onopen` ⇒ `Connected`
    (handshake accepted), `onerror`/timeout ⇒ `Unreachable`. We
    verify the upgrade, not stream health.
  - **Faucet / Explorer** — a `no-cors` GET yields an *opaque*
    response we cannot inspect, so a resolved fetch is labelled
    `Reachable` (host answered: DNS+TLS+HTTP), **not**
    verified-healthy. A thrown fetch ⇒ `Unreachable`.
- **Chain head**: `latest_committed_round`, current epoch, last
  boundary, rounds-per-epoch, plus a **freshness** pill computed
  from how long the committed round has been flat (advancing /
  stalling / halted).
- **Round-cadence sparkline**: inline SVG (single series, 2px line,
  no library) of `latest_committed_round` over the last N polls —
  a live "is it advancing" trace. Shows "Collecting samples…"
  until it has ≥2 points.
- **Performance**: honest framing.
  - *Round cadence* — **live-derived**: observed interval between
    two successive commits seen by this endpoint ("observed, this
    endpoint").
  - *Committed TPS* and *Fast-path finality* — **placeholders**
    (`pending — see perf run`). Not derivable from `getEpoch`, and
    not yet published from a live multi-region run, so we do not
    fabricate them.

Polls the RPC and endpoint set every 5 seconds. No state stored
anywhere — refresh the page for a clean slate. Endpoints are a
config const (`ENDPOINTS`) at the top of `app.js` for operators
to edit.

## Honesty contract

The page never shows a green/healthy state it did not verify and
never a performance number it did not measure. `Reachable`,
`observed on this endpoint`, and `pending` are the honest labels
for, respectively, opaque-response probes, live-derived metrics,
and not-yet-measured metrics.

## NOT in this version

- Per-region tip tiles. Needs a public proxy in front of
  CloudWatch's `GetMetricData` (G6's metrics land there). Without
  that, the page can only see the ALB-balanced cluster tip.
- Historical uptime graph. Needs persistent data; deferred.
- Published TPS / finality. Waiting on the multi-region perf run.
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
