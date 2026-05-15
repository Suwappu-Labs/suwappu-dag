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
 * Polymorphic intent shape. The `kind` discriminant tells you which
 * variant you got; switch on it to access variant-specific fields.
 * Hex fields are `0x`-prefixed lowercase. u128 fields are decimal
 * strings (same convention as `ValidatorMemberView.stake_gsx`).
 */
export type IntentView =
  | {
      kind: "transfer";
      /** 20-byte sender address, `0x`-prefixed hex. */
      from: string;
      /** 20-byte recipient address, `0x`-prefixed hex. */
      to: string;
      /** u128 amount, decimal string. */
      amount: string;
    }
  | {
      kind: "admit_authority";
      authority_id: number;
      /** Stake as a decimal string. */
      stake_gsx: string;
      /** ML-DSA-65 public key, hex-encoded (no `0x` prefix). */
      mldsa_public_key_hex: string;
      /** BLS12-381 G1 public key, hex-encoded (no `0x` prefix). */
      bls_public_key_hex: string;
    }
  | {
      kind: "exit_authority";
      authority_id: number;
    }
  | {
      kind: "eject_authority";
      authority_id: number;
      /** 32-byte slashing-proof reference, `0x`-prefixed hex. */
      proof_ref: string;
    };

/**
 * Return shape for {@link Client.getBlock}.
 */
export interface BlockView {
  /** DAG round this block was committed at. */
  round: number;
  /** 32-byte cert hash (`0x`-prefixed hex). */
  cert_hash: string;
  /** Ordered intents in this block. Empty for governance-only blocks. */
  intents: IntentView[];
}

/**
 * Return shape for {@link Client.getTransaction}.
 */
export interface TransactionView {
  /** 32-byte intent hash (`0x`-prefixed hex). */
  tx_hash: string;
  /** DAG round of the committing block. */
  round: number;
  /** 32-byte committing cert hash (`0x`-prefixed hex). */
  cert_hash: string;
  /** Position within `BlockView.intents`. */
  index: number;
  /** The intent payload itself. */
  intent: IntentView;
}

/**
 * JSON-safe projection of a single event from the daemon's NDJSON
 * event log. One per WebSocket text message emitted by the daemon's
 * `gsx_subscribeEvents` endpoint (`GET /ws`). Mirrors
 * `gsx_rpc::context::EventView` on the server side.
 *
 * Optional fields follow `serde(skip_serializing_if = "Option::is_none")`
 * on the Rust side — they're omitted from the JSON envelope, so the
 * TypeScript type uses `?:` rather than `| null`.
 */
export interface EventView {
  /** Unix milliseconds. */
  t_ms: number;
  /** Validator label (matches the daemon's `NodeConfig::self_id`). */
  region: string;
  /** Lane name: `"main" | "fastpath" | "ltp" | "client"`. */
  lane: string;
  /** Action verb (e.g. `"proposed"`, `"committed"`, `"submitted"`). */
  event: string;
  round?: number;
  cert_hash?: string;
  tx_hash?: string;
  peer?: string;
  intent_hashes?: string[];
  authority_id?: number;
  kind?: string;
  /** Rolling 60-second receive count on `wire_metrics` events. */
  received_60s?: number;
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
