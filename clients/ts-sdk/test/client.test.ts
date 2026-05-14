/**
 * Tests for the `@gsx/client` SDK.
 *
 * Uses node's native `node:test` runner (no vitest/jest dep) and a
 * fake `fetch` so we don't bind a real TCP socket. The server
 * round-trips integration test for this surface lives in the Rust
 * `clients/rust-sdk/tests/integration.rs` — it's the canonical
 * end-to-end check; this file pins down TS-specific behavior:
 *
 *   - the auto-incrementing id
 *   - error-class narrowing
 *   - headers + timeout configuration plumbing
 */

import { test } from "node:test";
import assert from "node:assert/strict";

import { Client, RpcError, TransportError } from "../src/index.js";

interface CapturedRequest {
  url: string;
  init: RequestInit;
}

function makeFetchMock(
  responder: (req: CapturedRequest) => unknown,
): {
  fetch: typeof fetch;
  captured: CapturedRequest[];
} {
  const captured: CapturedRequest[] = [];
  const mock: typeof fetch = async (input, init) => {
    const url = typeof input === "string" ? input : input.toString();
    const req = { url, init: init ?? {} };
    captured.push(req);
    const body = responder(req);
    return new Response(JSON.stringify(body), {
      status: 200,
      headers: { "content-type": "application/json" },
    });
  };
  return { fetch: mock, captured };
}

test("getEpoch round-trips the result envelope", async () => {
  const { fetch } = makeFetchMock(() => ({
    jsonrpc: "2.0",
    id: 1,
    result: { current: 7, last_boundary_round: 7168, rounds_per_epoch: 1024 },
  }));
  const client = new Client("http://localhost:0", { fetch });
  const epoch = await client.getEpoch();
  assert.deepEqual(epoch, {
    current: 7,
    last_boundary_round: 7168,
    rounds_per_epoch: 1024,
  });
});

test("getStake returns null on NotFound (-32000)", async () => {
  const { fetch } = makeFetchMock(() => ({
    jsonrpc: "2.0",
    id: 1,
    error: { code: -32000, message: "no stake recorded for authority_id 999" },
  }));
  const client = new Client("http://localhost:0", { fetch });
  const stake = await client.getStake(999);
  assert.equal(stake, null);
});

test("non-NotFound RpcError is thrown", async () => {
  const { fetch } = makeFetchMock(() => ({
    jsonrpc: "2.0",
    id: 1,
    error: { code: -32601, message: "method not found: gsx_bogus" },
  }));
  const client = new Client("http://localhost:0", { fetch });
  await assert.rejects(
    () => client.call("gsx_bogus"),
    (err: unknown) => err instanceof RpcError && (err as RpcError).code === -32601,
  );
});

test("TransportError on fetch rejection", async () => {
  const flakyFetch: typeof fetch = async () => {
    throw new TypeError("fetch failed");
  };
  const client = new Client("http://localhost:0", { fetch: flakyFetch });
  await assert.rejects(
    () => client.getEpoch(),
    (err: unknown) => err instanceof TransportError,
  );
});

test("id auto-increments across calls", async () => {
  const { fetch, captured } = makeFetchMock(() => ({
    jsonrpc: "2.0",
    id: 1,
    result: { current: 0, last_boundary_round: 0, rounds_per_epoch: 1024 },
  }));
  const client = new Client("http://localhost:0", { fetch });
  await client.getEpoch();
  await client.getEpoch();
  await client.getEpoch();
  const ids = captured.map((r) => JSON.parse(r.init.body as string).id);
  assert.deepEqual(ids, [1, 2, 3]);
});

test("custom headers and method are propagated to fetch", async () => {
  const { fetch, captured } = makeFetchMock(() => ({
    jsonrpc: "2.0",
    id: 1,
    result: [],
  }));
  const client = new Client("http://localhost:0", {
    fetch,
    headers: { "x-api-key": "secret" },
  });
  await client.getAuthorityRegistry();
  const first = captured[0];
  assert.ok(first, "captured at least one request");
  assert.equal(first.init.method, "POST");
  const headers = first.init.headers as Record<string, string>;
  assert.equal(headers["content-type"], "application/json");
  assert.equal(headers["x-api-key"], "secret");
});

test("positional vs object params encode correctly via the generic call()", async () => {
  const { fetch, captured } = makeFetchMock(() => ({
    jsonrpc: "2.0",
    id: 1,
    result: null,
  }));
  const client = new Client("http://localhost:0", { fetch });
  await client.call("gsx_getStake", { authority_id: 0 });
  await client.call("gsx_getStake", [0]);
  const bodies = captured.map((r) => JSON.parse(r.init.body as string));
  assert.deepEqual(bodies[0]?.params, { authority_id: 0 });
  assert.deepEqual(bodies[1]?.params, [0]);
});

test("getBalance accepts Uint8Array and serializes as 0x-prefixed hex", async () => {
  const { fetch, captured } = makeFetchMock(() => ({
    jsonrpc: "2.0",
    id: 1,
    result: { address: `0x${"aa".repeat(20)}`, balance: "1000" },
  }));
  const client = new Client("http://localhost:0", { fetch });
  const addr = new Uint8Array(20).fill(0xaa);
  const view = await client.getBalance(addr);
  assert.equal(view.balance, "1000");

  const body = JSON.parse(captured[0]!.init.body as string);
  assert.equal(body.params.address, `0x${"aa".repeat(20)}`);
});

test("getBalance passes through hex string with 0x prefix unchanged", async () => {
  const { fetch, captured } = makeFetchMock(() => ({
    jsonrpc: "2.0",
    id: 1,
    result: { address: `0x${"bb".repeat(20)}`, balance: "0" },
  }));
  const client = new Client("http://localhost:0", { fetch });
  await client.getBalance(`0x${"bb".repeat(20)}`);
  const body = JSON.parse(captured[0]!.init.body as string);
  assert.equal(body.params.address, `0x${"bb".repeat(20)}`);
});

test("getBalance adds 0x prefix to bare hex string", async () => {
  const { fetch, captured } = makeFetchMock(() => ({
    jsonrpc: "2.0",
    id: 1,
    result: { address: `0x${"cc".repeat(20)}`, balance: "0" },
  }));
  const client = new Client("http://localhost:0", { fetch });
  await client.getBalance("cc".repeat(20));
  const body = JSON.parse(captured[0]!.init.body as string);
  assert.equal(body.params.address, `0x${"cc".repeat(20)}`);
});

test("getBalance unknown address returns BalanceView with balance='0'", async () => {
  // Substrate doesn't distinguish absent from explicit-zero — the SDK
  // should NOT translate this into null the way getStake does.
  const { fetch } = makeFetchMock(() => ({
    jsonrpc: "2.0",
    id: 1,
    result: { address: `0x${"dd".repeat(20)}`, balance: "0" },
  }));
  const client = new Client("http://localhost:0", { fetch });
  const view = await client.getBalance(new Uint8Array(20).fill(0xdd));
  assert.equal(view.balance, "0");
});

test("getBlock returns typed BlockView for known round", async () => {
  const { fetch } = makeFetchMock(() => ({
    jsonrpc: "2.0",
    id: 1,
    result: {
      round: 42,
      cert_hash: `0x${"ab".repeat(32)}`,
      intents: [
        {
          kind: "transfer",
          from: `0x${"11".repeat(20)}`,
          to: `0x${"22".repeat(20)}`,
          amount: "100",
        },
      ],
    },
  }));
  const client = new Client("http://localhost:0", { fetch });
  const b = await client.getBlock(42);
  assert.notEqual(b, null);
  assert.equal(b!.round, 42);
  assert.equal(b!.intents.length, 1);
  const first = b!.intents[0]!;
  assert.equal(first.kind, "transfer");
  if (first.kind === "transfer") {
    assert.equal(first.amount, "100");
  }
});

test("getBlock returns null on NotFound (-32000)", async () => {
  const { fetch } = makeFetchMock(() => ({
    jsonrpc: "2.0",
    id: 1,
    error: { code: -32000, message: "no committed block at round 999" },
  }));
  const client = new Client("http://localhost:0", { fetch });
  assert.equal(await client.getBlock(999), null);
});

test("getTransaction accepts Uint8Array and round-trips", async () => {
  const { fetch, captured } = makeFetchMock(() => ({
    jsonrpc: "2.0",
    id: 1,
    result: {
      tx_hash: `0x${"cd".repeat(32)}`,
      round: 42,
      cert_hash: `0x${"ab".repeat(32)}`,
      index: 0,
      intent: {
        kind: "transfer",
        from: `0x${"11".repeat(20)}`,
        to: `0x${"22".repeat(20)}`,
        amount: "100",
      },
    },
  }));
  const client = new Client("http://localhost:0", { fetch });
  const h = new Uint8Array(32).fill(0xcd);
  const t = await client.getTransaction(h);
  assert.notEqual(t, null);
  assert.equal(t!.round, 42);
  assert.equal(t!.index, 0);
  // Sent param uses 0x-prefixed hex regardless of input type.
  const body = JSON.parse(captured[0]!.init.body as string);
  assert.equal(body.params.tx_hash, `0x${"cd".repeat(32)}`);
});

test("getTransaction returns null on NotFound (-32000)", async () => {
  const { fetch } = makeFetchMock(() => ({
    jsonrpc: "2.0",
    id: 1,
    error: {
      code: -32000,
      message: "no committed transaction with hash 0xff...",
    },
  }));
  const client = new Client("http://localhost:0", { fetch });
  assert.equal(
    await client.getTransaction(new Uint8Array(32).fill(0xff)),
    null,
  );
});
