//! Read-only view of the gsx-dag node state that the RPC methods consume.
//!
//! Defined as a trait so `gsx-rpc` doesn't need a runtime dependency on
//! `gsx-node` — the node implements `StateView` for its concrete `State`
//! type in a thin adapter. The trait uses Rust 1.75+ async-fn-in-trait;
//! it is consumed via generic bounds (`S: StateView`), not as
//! `dyn StateView` (the trait is intentionally not dyn-compatible).

use serde::{Deserialize, Serialize};

/// Snapshot of the current epoch state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EpochView {
    /// Current epoch index (monotonic, increments at every boundary cross).
    pub current: u64,
    /// Round at which the current epoch began.
    pub last_boundary_round: u64,
    /// Rounds per epoch (constant across an epoch, set at genesis).
    pub rounds_per_epoch: u64,
}

/// JSON-safe projection of an Authority Ring member.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthorityMemberView {
    /// Authority id (zero-indexed slot in the published set).
    pub id: u32,
    /// Posted stake in GSX (u64 fits — Authority stakes are bounded
    /// by the licensure cap).
    pub stake_gsx: u64,
    /// ML-DSA-65 public key bytes, hex-encoded (1952 B canonical).
    pub public_key_hex: String,
}

/// JSON-safe projection of a Validator Ring member. Stake is encoded
/// as a decimal string to survive JSON's 53-bit integer ceiling
/// (Validator stakes use u128 — see `gsx_consensus::Stake`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValidatorMemberView {
    pub id: u32,
    pub stake_gsx: String,
}

/// JSON-safe projection of a substrate balance lookup. The address is
/// hex-encoded (20 bytes → 40 hex chars, prefixed with `0x`); the
/// balance follows the same u128-as-decimal-string convention as
/// `ValidatorMemberView::stake_gsx`. A zero balance is a valid
/// response — the server only returns NotFound if the underlying
/// substrate is unable to answer the lookup at all (which it never
/// is for `InMemorySubstrate`, but might be for future substrates).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BalanceView {
    /// Hex-encoded 20-byte address with `0x` prefix.
    pub address: String,
    /// Balance in the substrate's smallest unit, as a decimal string.
    pub balance: String,
}

/// JSON-safe projection of an Intent — the daemon's typed enum,
/// translated to a polymorphic shape so the SDK doesn't need to know
/// about the Rust-side enum discriminants. Each Intent kind sets
/// `kind` to the discriminant name in snake_case and includes its own
/// fields. Wallets/explorers should switch on `kind`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum IntentView {
    /// `Intent::Transfer { from, to, amount }`. Addresses are 20-byte
    /// hex (0x-prefixed). Amount is a decimal string (u128).
    Transfer {
        from: String,
        to: String,
        amount: String,
    },
    /// `Intent::AdmitAuthority`. Stake is a decimal string for u128
    /// future-compat; the underlying field is u64.
    AdmitAuthority {
        authority_id: u32,
        stake_gsx: String,
        /// ML-DSA-65 public key bytes, hex-encoded.
        mldsa_public_key_hex: String,
        /// BLS12-381 G1 public key bytes, hex-encoded.
        bls_public_key_hex: String,
    },
    /// `Intent::ExitAuthority { authority_id }`.
    ExitAuthority { authority_id: u32 },
    /// `Intent::EjectAuthority { authority_id, proof_ref }`. proof_ref
    /// is 32-byte hex (0x-prefixed).
    EjectAuthority {
        authority_id: u32,
        proof_ref: String,
    },
}

/// JSON-safe projection of a committed block. `cert_hash` is the
/// authoritative key — clients can fetch a block by round (which
/// resolves via the round index) and the response includes the cert
/// hash so a follow-up by-hash request stays consistent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlockView {
    /// DAG round this block was committed at.
    pub round: u64,
    /// 32-byte cert hash (0x-prefixed hex). Identity for cross-API joins.
    pub cert_hash: String,
    /// Ordered intents in this block. `[]` for empty blocks (governance-only).
    pub intents: Vec<IntentView>,
}

/// JSON-safe projection of a single committed transaction (one
/// intent inside its block). The `index` is the position within
/// `BlockView.intents` so a paginated fetch can navigate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransactionView {
    /// 32-byte intent hash (0x-prefixed hex).
    pub tx_hash: String,
    /// DAG round of the committing block.
    pub round: u64,
    /// 32-byte committing cert hash (0x-prefixed hex).
    pub cert_hash: String,
    /// Position within `block.intents`.
    pub index: usize,
    /// The intent payload itself.
    pub intent: IntentView,
}

/// Read-only view over the node state needed by the JSON-RPC methods.
///
/// Implementers must guarantee:
///
/// - Each `*_snapshot` returns an owned, consistent point-in-time copy.
///   It is acceptable for two calls to observe different epochs.
/// - `Send + Sync + 'static` so the router can clone the handle across
///   request tasks.
pub trait StateView: Send + Sync + 'static {
    fn epoch_snapshot(&self) -> impl std::future::Future<Output = EpochView> + Send;

    fn authority_snapshot(
        &self,
    ) -> impl std::future::Future<Output = Vec<AuthorityMemberView>> + Send;

    fn validator_snapshot(
        &self,
    ) -> impl std::future::Future<Output = Vec<ValidatorMemberView>> + Send;

    fn stake_for(
        &self,
        authority_id: u32,
    ) -> impl std::future::Future<Output = Option<u128>> + Send;

    /// Look up the substrate balance for `address` (20-byte EVM-style
    /// account id). Returns `0` for any address the substrate has
    /// never seen — the substrate does not distinguish "absent" from
    /// "explicitly zero." Adapters MUST NOT treat a zero return as
    /// NotFound.
    fn balance_for(&self, address: [u8; 20]) -> impl std::future::Future<Output = u128> + Send;

    /// Look up a committed block by round. Returns `None` if no block
    /// has been committed at that round yet (either because the round
    /// is in the future or because the leader was skipped under the
    /// indirect commit rule).
    fn block_at_round(
        &self,
        round: u64,
    ) -> impl std::future::Future<Output = Option<BlockView>> + Send;

    /// Look up a committed transaction by its intent hash. Returns
    /// `None` if the hash has never been observed in a committed block.
    fn transaction_by_hash(
        &self,
        tx_hash: [u8; 32],
    ) -> impl std::future::Future<Output = Option<TransactionView>> + Send;

    /// Submit a signed intent for inclusion in the next block. The
    /// adapter is responsible for: (1) bincode-decoding `intent_bincode`
    /// into a typed `Intent`, (2) verifying the ML-DSA-65 signature
    /// against the network_id + Authority Ring, (3) computing the
    /// intent hash, (4) enqueueing the intent into the daemon's mpsc.
    ///
    /// Errors are typed so the RPC layer can map them to the correct
    /// JSON-RPC error codes (see `SubmitIntentError`). `intent_bincode`
    /// is the SAME bincode-serialized form used on the TCP wire — SDK
    /// clients build it once, then can choose either ingress wire
    /// (TCP/bincode or JSON-RPC) without re-signing.
    fn submit_intent(
        &self,
        intent_bincode: Vec<u8>,
        signature: Vec<u8>,
        signer_pubkey_hash: [u8; 32],
    ) -> impl std::future::Future<Output = Result<[u8; 32], SubmitIntentError>> + Send;
}

/// Error taxonomy returned by `StateView::submit_intent`. Maps directly
/// to JSON-RPC error codes in the dispatcher.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SubmitIntentError {
    /// Intent bytes failed bincode decoding into `gsx_execution::Intent`.
    /// Maps to `-32602 InvalidParams`.
    BadIntentEncoding(String),
    /// `signer_pubkey_hash` is not in the active Authority Ring.
    /// Maps to `-32001 UnknownSigner`.
    UnknownSigner,
    /// Signature failed ML-DSA-65 verification against the resolved
    /// pubkey. Maps to `-32002 BadSignature`.
    BadSignature,
    /// The daemon's intent channel is full / closed; the caller should
    /// retry. Maps to `-32003 EnqueueFull`.
    EnqueueFull,
}

/// Concrete context handle passed into the router. Wraps an
/// `Arc<S>` so the axum state is cheap to clone per request.
pub struct RpcContext<S: StateView> {
    pub state: std::sync::Arc<S>,
}

impl<S: StateView> Clone for RpcContext<S> {
    fn clone(&self) -> Self {
        Self {
            state: self.state.clone(),
        }
    }
}

impl<S: StateView> RpcContext<S> {
    pub fn new(state: std::sync::Arc<S>) -> Self {
        Self { state }
    }
}
