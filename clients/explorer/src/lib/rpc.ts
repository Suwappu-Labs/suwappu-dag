// RPC shapes that don't (yet) have a typed wrapper in `@suwappu/client`.
//
// `HeaderAttestationView` mirrors the Rust struct in
// `crates/suwappu-rpc/src/context.rs`. The SDK has no `getHeaderAttestation`
// method, so we drive it through the generic `client.call(...)` escape
// hatch. See `fetchHeaderAttestation` below.

import type { Client } from "@suwappu/client";
import { RpcError, MalformedResponseError } from "@suwappu/client";

/**
 * This node's ML-DSA-65 bridge-header side-attestation over its latest
 * finalized block. Byte-compatible with `HeaderAttestationView` on the
 * Rust side.
 */
export interface HeaderAttestationView {
  /** DAG round of the latest finalized block. */
  block_number: number;
  /** 32-byte BLAKE3 L1 state root after that block (0x-prefixed hex). */
  state_root: string;
  /** Attesting Authority Ring member id. */
  authority_id: number;
  /** Attesting validator's ML-DSA-65 public key (0x-prefixed hex). */
  pubkey: string;
  /** Detached ML-DSA-65 signature over the header digest (0x-prefixed hex). */
  signature: string;
  /** uint256 network id this signature binds to (0x-prefixed 32-byte hex). */
  network_id: string;
  /** Oracle address this signature binds to (0x-prefixed 20-byte hex). */
  oracle: string;
}

export type AttestationResult =
  | { status: "ok"; attestation: HeaderAttestationView }
  | { status: "unavailable"; reason: string }
  | { status: "error"; message: string };

/**
 * Fetch `suwappu_getHeaderAttestation`. The daemon returns JSON `null`
 * when no block has finalized yet or the node has no bridge signer
 * configured, and a JSON-RPC error (e.g. method-not-found) on endpoints
 * where the method isn't exposed. Both are surfaced as an
 * "unavailable" result — never a throw the caller has to catch.
 */
export async function fetchHeaderAttestation(
  client: Client,
): Promise<AttestationResult> {
  try {
    const view = await client.call<HeaderAttestationView | null>(
      "suwappu_getHeaderAttestation",
      null,
    );
    if (view == null) {
      return {
        status: "unavailable",
        reason: "no-attestation",
      };
    }
    return { status: "ok", attestation: view };
  } catch (err) {
    // Application-level JSON-RPC errors (method not found, not
    // configured) mean the feature isn't enabled here. A
    // MalformedResponseError can also arise if the daemon omits a
    // `null` result field entirely. Both are treated as a clean empty
    // state, not a hard error.
    if (err instanceof RpcError || err instanceof MalformedResponseError) {
      return { status: "unavailable", reason: "not-enabled" };
    }
    return {
      status: "error",
      message: err instanceof Error ? err.message : String(err),
    };
  }
}
