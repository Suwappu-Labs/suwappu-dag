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
  EpochView,
  JsonRpcRequest,
  JsonRpcResponse,
  StakeEntry,
  ValidatorMemberView,
} from "./types.js";

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
