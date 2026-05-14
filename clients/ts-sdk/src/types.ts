/**
 * View types served by the `gsx-rpc` server, mirrored on the TS side.
 *
 * These shapes MUST stay byte-compatible with the Rust definitions in
 * `crates/gsx-rpc/src/context.rs`. Any field added/removed there needs
 * a matching update here.
 */

/**
 * Snapshot of the current epoch state.
 */
export interface EpochView {
  /** Current epoch index (monotonic, increments at every boundary cross). */
  current: number;
  /** Round at which the current epoch began. */
  last_boundary_round: number;
  /** Rounds per epoch (constant across an epoch, set at genesis). */
  rounds_per_epoch: number;
}

/**
 * JSON-safe projection of an Authority Ring member.
 */
export interface AuthorityMemberView {
  /** Authority id (zero-indexed slot in the published set). */
  id: number;
  /** Posted stake in GSX (u64 — fits in `number` for stakes ≤ 2^53). */
  stake_gsx: number;
  /** ML-DSA-65 public key bytes, hex-encoded (1952 B canonical → 3904 hex chars). */
  public_key_hex: string;
}

/**
 * JSON-safe projection of a Validator Ring member.
 *
 * Stake is encoded as a decimal string to survive JSON's 53-bit integer
 * ceiling (Validator stakes use u128 on the Rust side — see
 * `gsx_consensus::Stake`). Parse with `BigInt(stake_gsx)` if you need
 * to do arithmetic.
 */
export interface ValidatorMemberView {
  id: number;
  stake_gsx: string;
}

/**
 * Return shape for {@link Client.getStake}.
 *
 * `stake_gsx` is a decimal string (same rationale as
 * `ValidatorMemberView.stake_gsx`).
 */
export interface StakeEntry {
  id: number;
  stake_gsx: string;
}

/**
 * Return shape for {@link Client.getBalance}.
 *
 * `balance` is a decimal string (u128 on the Rust side; JS `number`
 * tops out at 2^53). An unknown address surfaces here as
 * `balance: "0"` — the substrate doesn't distinguish absent from
 * explicit-zero. Parse with `BigInt(balance)` if you need to do
 * arithmetic.
 */
export interface BalanceView {
  /** Hex-encoded 20-byte address with `0x` prefix. */
  address: string;
  /** Balance in the substrate's smallest unit, as a decimal string. */
  balance: string;
}

/**
 * JSON-RPC 2.0 request envelope.
 *
 * The SDK builds this internally — callers don't normally construct one
 * directly, but it's exported so power users can use {@link Client.call}
 * for methods that don't yet have a typed wrapper.
 */
export interface JsonRpcRequest {
  jsonrpc: "2.0";
  id: number | string;
  method: string;
  params?: unknown;
}

/**
 * JSON-RPC 2.0 response envelope. Exactly one of `result` / `error`
 * is present per the spec.
 */
export interface JsonRpcResponse<T = unknown> {
  jsonrpc: "2.0";
  id: number | string | null;
  result?: T;
  error?: JsonRpcErrorBody;
}

export interface JsonRpcErrorBody {
  code: number;
  message: string;
}
