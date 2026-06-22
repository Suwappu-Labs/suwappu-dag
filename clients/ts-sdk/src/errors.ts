/**
 * Error taxonomy for the `@suwappu/client` SDK.
 *
 * All errors inherit from `SuwappuClientError`. Catch the base class to
 * handle every failure uniformly, or `instanceof`-narrow to one of the
 * subclasses below for granular handling.
 */

export class SuwappuClientError extends Error {
  override readonly name: string = "SuwappuClientError";
  constructor(message: string) {
    super(message);
  }
}

/**
 * TCP / TLS / HTTP-status-code failure. The underlying cause is in
 * `cause` (e.g. `TypeError: fetch failed`, or a stringified non-2xx
 * status code).
 */
export class TransportError extends SuwappuClientError {
  override readonly name = "TransportError";
  constructor(message: string, public override readonly cause?: unknown) {
    super(message);
  }
}

/**
 * JSON-RPC application-level error response from the server. The `code`
 * matches the JSON-RPC 2.0 reserved range plus suwappu-rpc's app codes
 * (currently just -32000 NotFound).
 */
export class RpcError extends SuwappuClientError {
  override readonly name = "RpcError";
  constructor(
    public readonly code: number,
    message: string,
  ) {
    super(`rpc error ${code}: ${message}`);
  }
}

/**
 * The response envelope was syntactically valid JSON-RPC but carried
 * neither a `result` nor an `error` — protocol violation server-side.
 */
export class MalformedResponseError extends SuwappuClientError {
  override readonly name = "MalformedResponseError";
  constructor(message: string) {
    super(`malformed response: ${message}`);
  }
}
