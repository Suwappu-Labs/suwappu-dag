/**
 * Tests for {@link Client.subscribeEvents}.
 *
 * Uses node's native `node:test` runner and a mock `WebSocket`
 * implementation so we don't bind a real socket. End-to-end
 * integration against a running gsx-node lives in
 * `clients/rust-sdk/tests/integration.rs` — this file pins down the
 * TS-side parsing + error-classification behavior.
 */

import { test } from "node:test";
import assert from "node:assert/strict";

import {
  Client,
  MalformedResponseError,
  TransportError,
} from "../src/index.js";
import type {
  EventView,
  WebSocketCtor,
  WebSocketLike,
  WsErrorEvent,
  WsMessageEvent,
} from "../src/index.js";

/**
 * Mock WebSocket that buffers listeners and exposes inject methods to
 * fire events. Constructor URL is captured so we can assert on the
 * `http://...` → `ws://...` rewrite the SDK performs.
 */
class MockWebSocket implements WebSocketLike {
  static lastUrl: string | null = null;
  #messageListeners: Array<(ev: WsMessageEvent) => void> = [];
  #errorListeners: Array<(ev: WsErrorEvent) => void> = [];
  #closeListeners: Array<() => void> = [];
  closed = false;

  constructor(url: string) {
    MockWebSocket.lastUrl = url;
  }

  addEventListener(
    type: "message",
    listener: (ev: WsMessageEvent) => void,
  ): void;
  addEventListener(
    type: "error",
    listener: (ev: WsErrorEvent) => void,
  ): void;
  addEventListener(type: "close", listener: () => void): void;
  addEventListener(type: string, listener: unknown): void {
    if (type === "message") {
      this.#messageListeners.push(listener as (ev: WsMessageEvent) => void);
    } else if (type === "error") {
      this.#errorListeners.push(listener as (ev: WsErrorEvent) => void);
    } else if (type === "close") {
      this.#closeListeners.push(listener as () => void);
    }
  }

  close(): void {
    this.closed = true;
    for (const l of this.#closeListeners) l();
  }

  // -- Test injection helpers --
  injectText(data: string): void {
    for (const l of this.#messageListeners) l({ data });
  }
  injectError(message: string): void {
    for (const l of this.#errorListeners) l({ message });
  }
  injectClose(): void {
    for (const l of this.#closeListeners) l();
  }
}

const MockWebSocketCtor = MockWebSocket as unknown as WebSocketCtor;

test("subscribeEvents rewrites http:// to ws://", () => {
  MockWebSocket.lastUrl = null;
  const client = new Client("http://127.0.0.1:9092");
  const sub = client.subscribeEvents({
    onEvent: () => {},
    WebSocket: MockWebSocketCtor,
  });
  assert.equal(MockWebSocket.lastUrl, "ws://127.0.0.1:9092/ws");
  sub.close();
});

test("subscribeEvents rewrites https:// to wss://", () => {
  MockWebSocket.lastUrl = null;
  const client = new Client("https://rpc.example.com");
  const sub = client.subscribeEvents({
    onEvent: () => {},
    WebSocket: MockWebSocketCtor,
  });
  assert.equal(MockWebSocket.lastUrl, "wss://rpc.example.com/ws");
  sub.close();
});

test("subscribeEvents passes through ws:// / wss:// URLs", () => {
  MockWebSocket.lastUrl = null;
  const client = new Client("ws://node.local:9092");
  const sub = client.subscribeEvents({
    onEvent: () => {},
    WebSocket: MockWebSocketCtor,
  });
  assert.equal(MockWebSocket.lastUrl, "ws://node.local:9092/ws");
  sub.close();
});

test("subscribeEvents parses EventView JSON and invokes onEvent", () => {
  let captured: MockWebSocket | undefined;
  class TrackingMock extends MockWebSocket {
    constructor(url: string) {
      super(url);
      captured = this;
    }
  }
  const client = new Client("http://localhost:0");
  const events: EventView[] = [];
  const sub = client.subscribeEvents({
    onEvent: (ev) => events.push(ev),
    WebSocket: TrackingMock as unknown as WebSocketCtor,
  });
  assert.ok(captured, "mock WebSocket must have been constructed");
  captured!.injectText(
    JSON.stringify({
      t_ms: 1_700_000_000_000,
      region: "v0",
      lane: "main",
      event: "committed",
      round: 42,
      cert_hash: "0xdeadbeef",
    }),
  );
  assert.equal(events.length, 1);
  assert.equal(events[0]?.event, "committed");
  assert.equal(events[0]?.round, 42);
  assert.equal(events[0]?.cert_hash, "0xdeadbeef");
  sub.close();
});

test("subscribeEvents surfaces 'lagged' notices as TransportError", () => {
  let captured: MockWebSocket | undefined;
  class TrackingMock extends MockWebSocket {
    constructor(url: string) {
      super(url);
      captured = this;
    }
  }
  const client = new Client("http://localhost:0");
  const errors: Error[] = [];
  const sub = client.subscribeEvents({
    onEvent: () => {
      assert.fail("onEvent must not fire for lag notice");
    },
    onError: (e) => errors.push(e),
    WebSocket: TrackingMock as unknown as WebSocketCtor,
  });
  captured!.injectText(
    JSON.stringify({ error: "lagged", skipped: 12, skipped_total: 12 }),
  );
  assert.equal(errors.length, 1);
  assert.ok(errors[0] instanceof TransportError);
  assert.match(errors[0]!.message, /skipped=12/);
  sub.close();
});

test("subscribeEvents reports malformed JSON via onError", () => {
  let captured: MockWebSocket | undefined;
  class TrackingMock extends MockWebSocket {
    constructor(url: string) {
      super(url);
      captured = this;
    }
  }
  const client = new Client("http://localhost:0");
  const errors: Error[] = [];
  const sub = client.subscribeEvents({
    onEvent: () => {
      assert.fail("onEvent must not fire for malformed JSON");
    },
    onError: (e) => errors.push(e),
    WebSocket: TrackingMock as unknown as WebSocketCtor,
  });
  captured!.injectText("{not valid json");
  assert.equal(errors.length, 1);
  assert.ok(errors[0] instanceof MalformedResponseError);
  sub.close();
});

test("subscribeEvents throws if no WebSocket implementation is available", () => {
  const client = new Client("http://localhost:0");
  // Force no global WebSocket and no injection.
  const originalWs = (globalThis as { WebSocket?: unknown }).WebSocket;
  (globalThis as { WebSocket?: unknown }).WebSocket = undefined;
  try {
    assert.throws(
      () => client.subscribeEvents({ onEvent: () => {} }),
      /no WebSocket implementation available/,
    );
  } finally {
    (globalThis as { WebSocket?: unknown }).WebSocket = originalWs;
  }
});

test("Subscription.close() suppresses onClose", () => {
  let captured: MockWebSocket | undefined;
  class TrackingMock extends MockWebSocket {
    constructor(url: string) {
      super(url);
      captured = this;
    }
  }
  const client = new Client("http://localhost:0");
  let closeFired = false;
  const sub = client.subscribeEvents({
    onEvent: () => {},
    onClose: () => {
      closeFired = true;
    },
    WebSocket: TrackingMock as unknown as WebSocketCtor,
  });
  sub.close();
  assert.equal(closeFired, false, "caller-initiated close suppresses callback");
  void captured;
});
