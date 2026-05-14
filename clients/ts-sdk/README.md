# @gsx/client

TypeScript client SDK for the [`gsx-dag`](https://github.com/GlobalSettlementNetwork/gsx-dag) JSON-RPC query API.

Mirrors the [Rust SDK](../rust-sdk) surface. Targets Node ≥ 20 (uses native `fetch`, `AbortSignal.timeout`) and any browser that ships native `fetch`. Zero runtime dependencies.

## Install

```sh
npm install @gsx/client
# or
pnpm add @gsx/client
# or
bun add @gsx/client
```

## Quick start

```ts
import { Client } from "@gsx/client";

const client = new Client("http://127.0.0.1:9092");

const epoch = await client.getEpoch();
console.log(`epoch=${epoch.current} rounds_per_epoch=${epoch.rounds_per_epoch}`);

const auths = await client.getAuthorityRegistry();
console.log(`${auths.length} authorities seated`);

const stake = await client.getStake(0);
if (stake === null) {
  console.log("authority 0 has no stake recorded");
} else {
  console.log(`stake: ${stake.stake_gsx} GSX`);
}
```

## Method surface

| Method | Returns | Notes |
|---|---|---|
| `getEpoch()` | `EpochView` | Current epoch + last boundary round + rounds per epoch |
| `getAuthorityRegistry()` | `AuthorityMemberView[]` | Ordered by authority id |
| `getValidatorRegistry()` | `ValidatorMemberView[]` | `stake_gsx` is a decimal **string** (u128 doesn't fit in `number`) |
| `getStake(authority_id)` | `StakeEntry \| null` | `null` on application-level NotFound; throws on other errors |
| `call<T>(method, params?)` | `T` | Generic escape hatch for methods without a typed wrapper |

Deferred (not yet served by the daemon, see [#27](https://github.com/GlobalSettlementNetwork/gsx-dag/issues/27)):

- `gsx_getBlock`, `gsx_getTransaction` — need a queryable index on `state.blocks`.
- `gsx_getBalance` — needs a substrate-state read API.
- `gsx_submitIntent` — write path is the existing TCP-bincode wire today.
- `gsx_subscribeEvents` (WebSocket) — would require an additional transport here.

Once the daemon serves them, this client will gain typed wrappers without breaking changes.

## u128 / `bigint`

Stake values returned by `getValidatorRegistry()` and `getStake()` use a JSON-safe decimal-string encoding because JavaScript's `number` only safely holds integers up to 2^53. To do arithmetic, lift to `bigint`:

```ts
const stake = await client.getStake(0);
if (stake) {
  const asBigInt = BigInt(stake.stake_gsx);
  // ... now safe to add / multiply / compare to other bigints
}
```

## Configuration

```ts
const client = new Client("http://127.0.0.1:9092", {
  // Inject a custom fetch (e.g. for proxies, HTTP/2, retries)
  fetch: globalThis.fetch,

  // Per-request timeout in ms (default: none)
  timeoutMs: 5000,

  // Headers added to every request (e.g. for an auth gateway)
  headers: { "x-api-key": "..." },
});
```

## Error handling

All thrown errors extend `GsxClientError`:

```ts
import {
  Client,
  GsxClientError,
  TransportError,
  RpcError,
  MalformedResponseError,
} from "@gsx/client";

try {
  await client.getEpoch();
} catch (err) {
  if (err instanceof TransportError) {
    // TCP/TLS/HTTP-status failure — retry or fail over
  } else if (err instanceof RpcError) {
    // Application-level rejection from the server
    console.log(err.code, err.message);
  } else if (err instanceof MalformedResponseError) {
    // Server protocol violation — likely version skew
  } else if (err instanceof GsxClientError) {
    // Unknown subclass — defensive fallthrough
  } else {
    throw err;
  }
}
```

## Development

```sh
cd clients/ts-sdk
npm install
npm run typecheck   # tsc --noEmit
npm run build       # emits to ./dist
npm test            # node --test test/*.test.ts
```

No CI step in `.github/workflows/ci.yml` runs this — gsx-dag's CI is Rust-only. Publish/test wiring lands in a follow-up.

## License

Apache-2.0
