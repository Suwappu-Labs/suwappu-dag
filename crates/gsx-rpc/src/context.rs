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
