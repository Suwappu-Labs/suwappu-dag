# `@suwappu/explorer` — suwappu-devnet block explorer

Single-page React + Vite app that browses the suwappu-devnet chain via
the `@suwappu/client` SDK. Deployed as a static SPA to S3 + CloudFront
behind `explorer.devnet.suwappu.globalsettlement.com`.

## Routes (hash-based; no server-side routing)

| Path | What it shows |
|---|---|
| `#/` | Recent 30 blocks + live tip (polls `suwappu_getEpoch` every 3 s). |
| `#/block/<round>` | Block detail: cert hash + intent list with tx-hash links. Skip rounds render a friendly "no block here" message. |
| `#/tx/<0x-hash>` | Transaction detail: `suwappu_getTransaction` lookup; cross-link to its block. |

A top-of-page search box accepts a round number or a 0x-tx-hash and
routes accordingly.

## Local development

```sh
cd clients/explorer

# Build the TS SDK first (the explorer depends on its dist/).
( cd ../ts-sdk && npm install && npm run build )

npm install
npm run dev      # http://localhost:5173
```

Override the RPC URL for local-devnet development:

```sh
VITE_RPC_URL=http://127.0.0.1:9092 npm run dev
```

## Build for deploy

```sh
npm run build       # writes dist/
npm run preview     # serves dist/ locally to smoke-test
```

`npm run build` emits a static `dist/` directory. The release
workflow in `.github/workflows/explorer.yml` syncs `dist/` to
`s3://suwappu-dag-devnet-explorer/` and invalidates the CloudFront
distribution.

## NOT yet implemented (deferred to a v0.2)

- Address page (`#/address/<0x>`) — requires the indexer's
  `GET /address/:addr/txs` endpoint to actually return data; the
  endpoint stub lands in this PR but the indexer doesn't yet
  populate an address index. The Home page's search box rejects
  20-byte addresses for now.
- Live tip via `suwappu_subscribeEvents` WebSocket — current
  implementation polls. WebSocket support requires CloudFront +
  ALB origin tweaks for sticky upgrades; deferred.
- Pagination beyond the last 30 blocks — needs the indexer.
- Charts (TPS, latency) — needs metrics export, deferred.

## See also

- [`../ts-sdk/`](../ts-sdk/) — the SDK this app consumes.
- [`../../DEVNET.md`](../../DEVNET.md) — devnet quick-start that
  links here as the canonical explorer URL.
- [`../../terraform/devnet/explorer.tf`](../../terraform/devnet/explorer.tf) —
  S3 + CloudFront + Route53 deployment.
