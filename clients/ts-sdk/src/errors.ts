/**
 * Error taxonomy for the `@gsx/client` SDK.
 *
 * All errors inherit from `GsxClientError`. Catch the base class to
 * handle every failure uniformly, or `instanceof`-narrow to one of the
 * subclasses below for granular handling.
 */

export class GsxClientError extends Error {
  override readonly name: string = "GsxClientError";
  constructor(message: string) {
    super(message);
  }
}

/**
 * TCP / TLS / HTTP-status-code failure. The underlying cause is in
 * `cause` (e.g. `TypeError: fetch failed`, or a stringified non-2xx
 * status code).
 */
export class TransportError extends GsxClientError {
  override readonly name = "TransportError";
  constructor(message: string, public override readonly cause?: unknown) {
    super(message);
  }
}

/**
 * JSON-RPC application-level error response from the server. The `code`
 * matches the JSON-RPC 2.0 reserved range plus gsx-rpc's app codes
 * (currently just -32000 NotFound).
 */
export class RpcError extends GsxClientError {
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
export class MalformedResponseError extends GsxClientError {
  override readonly name = "MalformedResponseError";
  constructor(message: string) {
    super(`malformed response: ${message}`);
  }
}
