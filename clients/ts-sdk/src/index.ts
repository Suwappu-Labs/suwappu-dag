/**
 * `@gsx/client` — TypeScript client SDK for the gsx-dag JSON-RPC query API.
 *
 * Wraps the JSON-RPC 2.0 methods exposed by `gsx-rpc` (bound into the
 * daemon by `crates/gsx-node/src/rpc_adapter.rs`). The current method
 * surface is read-only (Phase 2.1 MVP):
 *
 * - {@link Client.getEpoch}
 * - {@link Client.getAuthorityRegistry}
 * - {@link Client.getValidatorRegistry}
 * - {@link Client.getStake}
 *
 * Pattern after `viem` — small core, native `fetch`, zero runtime deps.
 *
 * @example
 * ```ts
 * import { Client } from "@gsx/client";
 *
 * const client = new Client("http://127.0.0.1:9092");
 * const epoch = await client.getEpoch();
 * console.log(`epoch=${epoch.current} rounds_per_epoch=${epoch.rounds_per_epoch}`);
 * ```
 */

export * from "./types.js";
export * from "./errors.js";

import {
  MalformedResponseError,
  RpcError,
  TransportError,
} from "./errors.js";
import type {
  AuthorityMemberView,
  BalanceView,
  BlockView,
  EpochView,
  EventView,
  JsonRpcRequest,
  JsonRpcResponse,
  StakeEntry,
  TransactionView,
  ValidatorMemberView,
} from "./types.js";

/**
 * Constructor signature for a platform-native `WebSocket` (browser
 * built-in, Node 22+ built-in, or the `ws` package's class). Kept
 * loose deliberately — different platforms ship slightly different
 * options, and the SDK only needs `new WebSocket(url)`.
 */
export interface WebSocketCtor {
  // eslint-disable-next-line @typescript-eslint/no-misused-new
  new (url: string): WebSocketLike;
}

/**
 * The narrow `WebSocket`-ish surface the SDK actually uses. Both the
 * platform built-in and the Node `ws` package satisfy this — kept
 * minimal so injection in tests is trivial.
 */
export interface WebSocketLike {
  addEventListener(
    type: "message",
    listener: (ev: WsMessageEvent) => void,
  ): void;
  addEventListener(
    type: "error",
    listener: (ev: WsErrorEvent) => void,
  ): void;
  addEventListener(type: "close", listener: () => void): void;
  close(): void;
}

export interface WsMessageEvent {
  data: unknown;
}

export interface WsErrorEvent {
  message?: string;
}

/**
 * Options for {@link Client.subscribeEvents}.
 */
export interface SubscribeEventsOptions {
  /** Invoked synchronously for each parsed event. */
  onEvent: (event: EventView) => void;
  /**
   * Optional error callback. Fires on:
   *   - server-side broadcast lag (`{error: "lagged", ...}` notices)
   *   - JSON parse failures on incoming text frames
   *   - WebSocket transport errors
   */
  onError?: (error: Error) => void;
  /**
   * Optional close callback. Fires when the socket closes for any
   * reason other than a caller-initiated `subscription.close()`.
   */
  onClose?: () => void;
  /**
   * Optional WebSocket constructor. Defaults to `globalThis.WebSocket`
   * — present in browsers and Node ≥ 22. On Node 20/21, pass the
   * `ws` package's exported class.
   */
  WebSocket?: WebSocketCtor;
}

/**
 * Handle returned by {@link Client.subscribeEvents}. Call `close()`
 * to shut the socket cleanly.
 */
export interface Subscription {
  close(): void;
}

export interface ClientOptions {
  /**
   * Optional `fetch` implementation. Defaults to the global `fetch`.
   * Useful for tests, or to inject an HTTP/2 / connection-pooling
   * implementation (e.g. `undici.fetch`).
   */
  fetch?: typeof fetch;

  /**
   * Optional timeout in milliseconds. Applied per request via
   * `AbortSignal.timeout`. `undefined` (the default) = no timeout.
   */
  timeoutMs?: number;

  /**
   * Optional headers to add to every request. Useful for auth tokens
   * if the operator fronts the RPC behind a gateway.
   */
  headers?: Record<string, string>;
}

/**
 * JSON-RPC client targeting a single gsx-dag node's RPC endpoint.
 *
 * Construction is cheap (no I/O); transport errors surface from the
 * first method call as {@link TransportError}. The auto-incrementing
 * JSON-RPC `id` is per-instance.
 */
export class Client {
  readonly #baseUrl: string;
  readonly #fetch: typeof fetch;
  readonly #timeoutMs: number | undefined;
  readonly #headers: Record<string, string>;
  #nextId: number;

  /**
   * @param baseUrl  e.g. `"http://127.0.0.1:9092"`. Should NOT include
   *                 a trailing path — the JSON-RPC endpoint is `/`.
   * @param options  See {@link ClientOptions}.
   */
  constructor(baseUrl: string, options: ClientOptions = {}) {
    this.#baseUrl = baseUrl;
    this.#fetch = options.fetch ?? globalThis.fetch;
    this.#timeoutMs = options.timeoutMs;
    this.#headers = options.headers ?? {};
    this.#nextId = 1;
  }

  /** Current epoch snapshot. */
  async getEpoch(): Promise<EpochView> {
    return this.call<EpochView>("gsx_getEpoch");
  }

  /** Ordered list of seated Authority Ring members. */
  async getAuthorityRegistry(): Promise<AuthorityMemberView[]> {
    return this.call<AuthorityMemberView[]>("gsx_getAuthorityRegistry");
  }

  /** Ordered list of seated Validator Ring members. */
  async getValidatorRegistry(): Promise<ValidatorMemberView[]> {
    return this.call<ValidatorMemberView[]>("gsx_getValidatorRegistry");
  }

  /**
   * Posted stake for a specific authority id. Returns `null` for the
   * application-level "not found" code (-32000); throws for any other
   * error class.
   */
  async getStake(authorityId: number): Promise<StakeEntry | null> {
    try {
      return await this.call<StakeEntry>("gsx_getStake", {
        authority_id: authorityId,
      });
    } catch (err) {
      if (err instanceof RpcError && err.code === -32000) return null;
      throw err;
    }
  }

  /**
   * Substrate balance for `address`. The address may be:
   *   - a 20-byte `Uint8Array`, or
   *   - a hex string (with or without `0x` prefix).
   *
   * Always returns a `BalanceView` — unknown addresses surface as
   * `balance: "0"` (the substrate doesn't distinguish absent from
   * explicit zero). For arithmetic, lift with `BigInt(view.balance)`.
   */
  async getBalance(address: Uint8Array | string): Promise<BalanceView> {
    const hexAddr =
      typeof address === "string"
        ? address.startsWith("0x") || address.startsWith("0X")
          ? address
          : `0x${address}`
        : `0x${bytesToHex(address)}`;
    return this.call<BalanceView>("gsx_getBalance", { address: hexAddr });
  }

  /**
   * Committed block at `round`. Returns `null` for the application-level
   * NotFound (no block at that round); throws on other errors. Switch
   * on each `intent.kind` to access variant-specific fields.
   */
  async getBlock(round: number): Promise<BlockView | null> {
    try {
      return await this.call<BlockView>("gsx_getBlock", { round });
    } catch (err) {
      if (err instanceof RpcError && err.code === -32000) return null;
      throw err;
    }
  }

  /**
   * Submit a signed intent for inclusion in the next block.
   *
   * **Low-level** — the caller is responsible for:
   *
   *   1. bincode-serializing the typed `Intent` into `intentBincode`.
   *      This SDK doesn't bundle bincode (a Rust serde wire format)
   *      yet; a typed helper that wraps this lands in a follow-up.
   *   2. Computing the signing digest
   *      `blake3(b"GSX_INTENT_V1" || network_id_bytes || intent_bincode)`
   *      and signing it with ML-DSA-65.
   *   3. Computing `blake3(public_key_bytes)` for `signerPubkeyHash`.
   *
   * Each parameter may be a `Uint8Array` or hex string (with or
   * without `0x` prefix). Returns the daemon's computed 32-byte
   * intent hash.
   */
  async submitIntentRaw(
    intentBincode: Uint8Array | string,
    signature: Uint8Array | string,
    signerPubkeyHash: Uint8Array | string,
  ): Promise<Uint8Array> {
    const params = {
      intent: hexParam(intentBincode),
      signature: hexParam(signature),
      signer_pubkey_hash: hexParam(signerPubkeyHash),
    };
    const ack = await this.call<{ tx_hash: string }>(
      "gsx_submitIntent",
      params,
    );
    return hexToBytes(ack.tx_hash);
  }

  /**
   * Committed transaction by intent hash. The hash may be:
   *   - a 32-byte `Uint8Array`, or
   *   - a hex string (with or without `0x` prefix).
   *
   * Returns `null` for the application-level NotFound; throws on
   * other errors.
   */
  async getTransaction(
    txHash: Uint8Array | string,
  ): Promise<TransactionView | null> {
    const hexHash =
      typeof txHash === "string"
        ? txHash.startsWith("0x") || txHash.startsWith("0X")
          ? txHash
          : `0x${txHash}`
        : `0x${bytesToHex(txHash)}`;
    try {
      return await this.call<TransactionView>("gsx_getTransaction", {
        tx_hash: hexHash,
      });
    } catch (err) {
      if (err instanceof RpcError && err.code === -32000) return null;
      throw err;
    }
  }

  /**
   * Subscribe to the daemon's live event stream over WebSocket
   * (`GET /ws` — same endpoint the Rust SDK's
   * `subscribe_events` consumes). The daemon emits one
   * `EventView`-shaped JSON object per WebSocket text message, plus
   * `{error: "lagged", skipped, skipped_total}` notices when the
   * server-side broadcast buffer overflows.
   *
   * Returns a `Subscription` object whose `close()` shuts the socket
   * cleanly. `onEvent` is invoked synchronously for each parsed
   * event; `onError` (optional) is invoked on lag notices, transport
   * errors, and JSON-parse failures so the caller can decide
   * whether to reconnect or escalate.
   *
   * The implementation uses the platform-native `WebSocket` class,
   * available in browsers and Node.js ≥ 22. For Node 20/21 callers,
   * pass `options.WebSocket` to inject the `ws` package's
   * implementation (or another constructor with the same surface).
   *
   * @example
   * ```ts
   * const sub = client.subscribeEvents({
   *   onEvent: (ev) => console.log(ev.event, ev.round, ev.cert_hash),
   *   onError: (e) => console.warn("ws:", e),
   * });
   * // ...later
   * sub.close();
   * ```
   */
  subscribeEvents(options: SubscribeEventsOptions): Subscription {
    const WebSocketImpl: WebSocketCtor =
      options.WebSocket ?? (globalThis as { WebSocket?: WebSocketCtor }).WebSocket!;
    if (typeof WebSocketImpl !== "function") {
      throw new TransportError(
        "no WebSocket implementation available — pass options.WebSocket " +
          "(e.g. from the `ws` package) on Node < 22, or upgrade Node",
      );
    }

    const url = this.#wsUrl();
    const socket = new WebSocketImpl(url);
    let closedByCaller = false;

    socket.addEventListener("message", (msg: WsMessageEvent) => {
      const raw = typeof msg.data === "string" ? msg.data : String(msg.data);
      let parsed: unknown;
      try {
        parsed = JSON.parse(raw);
      } catch (err) {
        options.onError?.(
          new MalformedResponseError(
            `ws: failed to parse event payload: ${err instanceof Error ? err.message : String(err)}`,
          ),
        );
        return;
      }
      // Server emits `{error: "lagged", skipped, skipped_total}` on
      // broadcast-buffer overflow. Surface as an error to the caller;
      // don't try to coerce it into an EventView.
      const obj = parsed as Record<string, unknown>;
      if (typeof obj.error === "string" && obj.error === "lagged") {
        options.onError?.(
          new TransportError(
            `ws: server reported lagged events (skipped=${String(obj.skipped)}, total=${String(obj.skipped_total)})`,
          ),
        );
        return;
      }
      options.onEvent(parsed as EventView);
    });

    socket.addEventListener("error", (ev: WsErrorEvent) => {
      // Browser WebSocket gives us an opaque Event on error; Node's
      // `ws` package gives us an `ErrorEvent` with `.message`. Both
      // are normalized into a TransportError for the caller.
      const msg =
        ev && typeof ev === "object" && "message" in ev && typeof (ev as { message?: unknown }).message === "string"
          ? ((ev as { message: string }).message)
          : "ws: transport error";
      options.onError?.(new TransportError(msg));
    });

    socket.addEventListener("close", () => {
      // Caller-initiated close → no callback needed; otherwise
      // propagate as a transport error so the caller can reconnect.
      if (!closedByCaller) {
        options.onClose?.();
      }
    });

    return {
      close: () => {
        closedByCaller = true;
        try {
          socket.close();
        } catch {
          // best-effort
        }
      },
    };
  }

  /**
   * Compute the WebSocket URL from the JSON-RPC base URL by swapping
   * the scheme (`http://` → `ws://`, `https://` → `wss://`) and
   * appending `/ws`. Used by `subscribeEvents`.
   */
  #wsUrl(): string {
    let base = this.#baseUrl;
    if (base.endsWith("/")) base = base.slice(0, -1);
    const lower = base.toLowerCase();
    if (lower.startsWith("https://")) {
      return `wss://${base.slice("https://".length)}/ws`;
    }
    if (lower.startsWith("http://")) {
      return `ws://${base.slice("http://".length)}/ws`;
    }
    // Already a ws:// / wss:// URL? Pass through.
    return `${base}/ws`;
  }

  /**
   * Generic JSON-RPC call. Public so callers can drive any method
   * that doesn't yet have a typed wrapper here.
   *
   * @throws {TransportError} on TCP/HTTP failure
   * @throws {RpcError} on JSON-RPC application-level error
   * @throws {MalformedResponseError} on protocol violation
   */
  async call<T>(method: string, params: unknown = null): Promise<T> {
    const id = this.#nextId++;
    const body: JsonRpcRequest = {
      jsonrpc: "2.0",
      id,
      method,
      params,
    };

    let response: Response;
    try {
      response = await this.#fetch(this.#baseUrl, {
        method: "POST",
        headers: {
          "content-type": "application/json",
          ...this.#headers,
        },
        body: JSON.stringify(body),
        signal:
          this.#timeoutMs !== undefined
            ? AbortSignal.timeout(this.#timeoutMs)
            : undefined,
      });
    } catch (err) {
      throw new TransportError(
        `fetch failed: ${err instanceof Error ? err.message : String(err)}`,
        err,
      );
    }

    if (!response.ok) {
      throw new TransportError(
        `HTTP ${response.status} ${response.statusText}`,
      );
    }

    let envelope: JsonRpcResponse<T>;
    try {
      envelope = (await response.json()) as JsonRpcResponse<T>;
    } catch (err) {
      throw new TransportError(
        `failed to decode JSON-RPC envelope: ${err instanceof Error ? err.message : String(err)}`,
        err,
      );
    }

    if (envelope.error !== undefined) {
      throw new RpcError(envelope.error.code, envelope.error.message);
    }
    if (envelope.result === undefined) {
      throw new MalformedResponseError(
        "response carried neither result nor error",
      );
    }
    return envelope.result;
  }
}

/**
 * Lower-case hex encoder for `Uint8Array`. No `0x` prefix.
 * Hand-rolled to keep `@gsx/client` zero-runtime-dep — `Buffer` is
 * Node-only and we want browser compatibility.
 */
function bytesToHex(bytes: Uint8Array): string {
  let out = "";
  for (let i = 0; i < bytes.length; i++) {
    const b = bytes[i];
    if (b !== undefined) {
      out += b.toString(16).padStart(2, "0");
    }
  }
  return out;
}

/**
 * Normalize a `Uint8Array | string` to a `0x`-prefixed lowercase hex
 * string suitable for the JSON-RPC params position. Strings are
 * passed through (and the prefix is auto-added if missing).
 */
function hexParam(input: Uint8Array | string): string {
  if (typeof input === "string") {
    return input.startsWith("0x") || input.startsWith("0X")
      ? input
      : `0x${input}`;
  }
  return `0x${bytesToHex(input)}`;
}

/**
 * Decode a `0x`-prefixed (or bare) hex string into a `Uint8Array`.
 * Throws if any character isn't a valid hex digit or the length is odd.
 */
function hexToBytes(hex: string): Uint8Array {
  const trimmed =
    hex.startsWith("0x") || hex.startsWith("0X") ? hex.slice(2) : hex;
  if (trimmed.length % 2 !== 0) {
    throw new Error(`hex string has odd length: ${trimmed.length}`);
  }
  const out = new Uint8Array(trimmed.length / 2);
  for (let i = 0; i < out.length; i++) {
    const byte = parseInt(trimmed.slice(i * 2, i * 2 + 2), 16);
    if (Number.isNaN(byte)) {
      throw new Error(`invalid hex at offset ${i * 2}`);
    }
    out[i] = byte;
  }
  return out;
}
