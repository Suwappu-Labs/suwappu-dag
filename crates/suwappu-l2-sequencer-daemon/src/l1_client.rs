//! L1 substrate client trait.
//!
//! The daemon's read/write surface against the suwappu-dag L1 is
//! narrowed to this trait so the wiring layer (Phase 2.2-b
//! batch builder, 2.2-c force-include watcher) is testable in
//! isolation. The real JSON-RPC implementation lands in
//! 2.2-c alongside `suwappu-client` integration.
//!
//! ## Method surface
//!
//! The trait covers exactly what the sequencer daemon needs:
//!
//! - `current_l1_height` — drives the force-include watcher
//!   + populates each batch header's `l1_anchor_height`.
//! - `current_l2_state_root` — populates `prev_l2_state_root`
//!   for the next batch's header.
//! - `current_l1_state_root` — populates `prev_l1_state_root`
//!   so the public-input blob binds the batch to the exact L1
//!   height it reads from.
//! - `read_force_include_registry` — returns the raw bytes the
//!   force-include watcher decodes (via
//!   `suwappu_execution::force_include::decode_map` at the wiring
//!   boundary; this crate stays light by accepting raw bytes).
//! - `submit_intent` — generic submission path covering
//!   `PostL2DA`, `CommitL2StateRoot`, `MarkForceIncludeHonored`,
//!   `SlashSequencer`, `EjectSequencer`. The serialized intent
//!   bytes are the caller's responsibility; the L1 returns the
//!   resulting intent_hash for ack/log.

use async_trait::async_trait;
use thiserror::Error;

/// Errors returned by [`L1Client`] methods.
#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum L1ClientError {
    /// Underlying transport (HTTP, WebSocket) error.
    #[error("L1 transport error: {0}")]
    Transport(String),

    /// L1 returned a JSON-RPC error response.
    #[error("L1 rpc error: {0}")]
    Rpc(String),

    /// Response could not be parsed.
    #[error("L1 response parse error: {0}")]
    Parse(String),
}

/// Narrow facade over the suwappu-dag L1 JSON-RPC surface.
///
/// `Send + Sync + 'static` bounds are intentional: the daemon
/// passes the client into multiple Tokio tasks. Production
/// implementations wrap a connection pool; the
/// [`mock::MockL1Client`] used in tests is `Arc`-cloneable.
#[async_trait]
pub trait L1Client: Send + Sync + 'static {
    /// Current L1 block height. Polled by the force-include
    /// watcher every `force_include_interval_l1_blocks`.
    async fn current_l1_height(&self) -> Result<u64, L1ClientError>;

    /// Current L2 state root for this sequencer's
    /// `l2_chain_id_hash`. Used as `prev_l2_state_root` in the
    /// next batch's header. Returns `[0u8; 32]` for a chain
    /// that hasn't yet committed any L2 state.
    async fn current_l2_state_root(
        &self,
        l2_chain_id_hash: &[u8; 32],
    ) -> Result<[u8; 32], L1ClientError>;

    /// Current L1 state root at `current_l1_height`. Pinned
    /// into each batch header's `prev_l1_state_root` so the
    /// public-input blob binds proof <-> L1 height.
    async fn current_l1_state_root(&self) -> Result<[u8; 32], L1ClientError>;

    /// Raw bytes of the force-include obligation registry.
    /// Caller decodes via
    /// `suwappu_execution::force_include::decode_map`.
    async fn read_force_include_registry(&self) -> Result<Vec<u8>, L1ClientError>;

    /// Submit a serialized `Intent` to the L1. The wire format
    /// is whatever the suwappu-client SDK accepts; the daemon
    /// constructs the bytes via the SDK's intent builders. The
    /// L1 returns the intent_hash on success for log/ack.
    async fn submit_intent(&self, intent_bytes: Vec<u8>) -> Result<[u8; 32], L1ClientError>;
}

/// In-memory mock for unit tests. The daemon's tasks call the
/// same trait; this mock returns whatever the test sets up.
pub mod mock {
    use std::sync::Mutex;

    use async_trait::async_trait;

    use super::{L1Client, L1ClientError};

    /// Mock L1 client backed by a Mutex-protected snapshot.
    /// Tests set the state, run the task, assert on
    /// [`submitted_intents`].
    #[derive(Debug, Default)]
    pub struct MockL1Client {
        inner: Mutex<MockState>,
    }

    #[derive(Debug, Default)]
    struct MockState {
        l1_height: u64,
        l2_state_root: [u8; 32],
        l1_state_root: [u8; 32],
        force_include_registry: Vec<u8>,
        submitted: Vec<Vec<u8>>,
        rpc_should_fail: bool,
    }

    impl MockL1Client {
        /// Construct an empty mock (L1 height 0, all roots zero,
        /// registry empty, no failures).
        pub fn new() -> Self {
            Self::default()
        }

        /// Set the L1 height returned by `current_l1_height`.
        pub fn set_l1_height(&self, h: u64) {
            self.inner.lock().unwrap().l1_height = h;
        }

        /// Set the L2 state root returned by
        /// `current_l2_state_root`.
        pub fn set_l2_state_root(&self, r: [u8; 32]) {
            self.inner.lock().unwrap().l2_state_root = r;
        }

        /// Set the L1 state root returned by
        /// `current_l1_state_root`.
        pub fn set_l1_state_root(&self, r: [u8; 32]) {
            self.inner.lock().unwrap().l1_state_root = r;
        }

        /// Set the raw force-include registry bytes.
        pub fn set_force_include_registry(&self, bytes: Vec<u8>) {
            self.inner.lock().unwrap().force_include_registry = bytes;
        }

        /// Make subsequent calls return `L1ClientError::Rpc`.
        /// Use to test the daemon's retry / log-and-skip
        /// behavior under transient RPC failures.
        pub fn set_should_fail(&self, fail: bool) {
            self.inner.lock().unwrap().rpc_should_fail = fail;
        }

        /// Read all intents the daemon has submitted so far.
        pub fn submitted_intents(&self) -> Vec<Vec<u8>> {
            self.inner.lock().unwrap().submitted.clone()
        }

        /// Reset the submitted-intents log.
        pub fn clear_submitted(&self) {
            self.inner.lock().unwrap().submitted.clear();
        }

        fn check_fail(&self) -> Result<(), L1ClientError> {
            if self.inner.lock().unwrap().rpc_should_fail {
                Err(L1ClientError::Rpc("mock failure".into()))
            } else {
                Ok(())
            }
        }
    }

    #[async_trait]
    impl L1Client for MockL1Client {
        async fn current_l1_height(&self) -> Result<u64, L1ClientError> {
            self.check_fail()?;
            Ok(self.inner.lock().unwrap().l1_height)
        }

        async fn current_l2_state_root(
            &self,
            _l2_chain_id_hash: &[u8; 32],
        ) -> Result<[u8; 32], L1ClientError> {
            self.check_fail()?;
            Ok(self.inner.lock().unwrap().l2_state_root)
        }

        async fn current_l1_state_root(&self) -> Result<[u8; 32], L1ClientError> {
            self.check_fail()?;
            Ok(self.inner.lock().unwrap().l1_state_root)
        }

        async fn read_force_include_registry(&self) -> Result<Vec<u8>, L1ClientError> {
            self.check_fail()?;
            Ok(self.inner.lock().unwrap().force_include_registry.clone())
        }

        async fn submit_intent(&self, intent_bytes: Vec<u8>) -> Result<[u8; 32], L1ClientError> {
            self.check_fail()?;
            // Mock intent_hash: blake3 over the bytes. Tests can
            // recompute or just assert .len() / contents.
            let hash = *blake3::hash(&intent_bytes).as_bytes();
            self.inner.lock().unwrap().submitted.push(intent_bytes);
            Ok(hash)
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[tokio::test]
        async fn mock_round_trips_height_and_roots() {
            let c = MockL1Client::new();
            c.set_l1_height(42);
            c.set_l2_state_root([0xaa; 32]);
            c.set_l1_state_root([0xbb; 32]);

            assert_eq!(c.current_l1_height().await.unwrap(), 42);
            assert_eq!(
                c.current_l2_state_root(&[0u8; 32]).await.unwrap(),
                [0xaa; 32]
            );
            assert_eq!(c.current_l1_state_root().await.unwrap(), [0xbb; 32]);
        }

        #[tokio::test]
        async fn mock_records_submitted_intents() {
            let c = MockL1Client::new();
            let h1 = c.submit_intent(vec![1, 2, 3]).await.unwrap();
            let h2 = c.submit_intent(vec![4, 5, 6]).await.unwrap();
            assert_ne!(h1, h2);
            assert_eq!(c.submitted_intents(), vec![vec![1, 2, 3], vec![4, 5, 6]]);
        }

        #[tokio::test]
        async fn mock_fail_flag_surfaces_rpc_error() {
            let c = MockL1Client::new();
            c.set_should_fail(true);
            assert!(matches!(
                c.current_l1_height().await,
                Err(L1ClientError::Rpc(_))
            ));
            assert!(matches!(
                c.submit_intent(vec![1]).await,
                Err(L1ClientError::Rpc(_))
            ));
        }
    }
}

/// Real JSON-RPC client for the suwappu-dag L1 node. Uses `reqwest`
/// to call the RPC endpoint exposed by `suwappu-rpc`. The mock module
/// above is for unit tests; this module is for integration and
/// production use.
pub mod rpc {
    use async_trait::async_trait;

    use super::{L1Client, L1ClientError};

    /// JSON-RPC client backed by `reqwest`. Thread-safe (the inner
    /// `reqwest::Client` is `Arc`-backed and clones are cheap).
    pub struct RpcL1Client {
        client: reqwest::Client,
        url: String,
    }

    impl RpcL1Client {
        /// Construct a client pointing at the given L1 RPC URL
        /// (e.g. `"http://127.0.0.1:9095"`).
        pub fn new(url: String) -> Self {
            Self {
                client: reqwest::Client::new(),
                url,
            }
        }

        /// Send a JSON-RPC 2.0 request and return the `result`
        /// field. Translates HTTP/transport failures to
        /// `L1ClientError::Transport` and JSON-RPC `error`
        /// responses to `L1ClientError::Rpc`.
        async fn rpc_call(
            &self,
            method: &str,
            params: serde_json::Value,
        ) -> Result<serde_json::Value, L1ClientError> {
            let body = serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": method,
                "params": params,
            });
            let resp = self
                .client
                .post(&self.url)
                .json(&body)
                .send()
                .await
                .map_err(|e| L1ClientError::Transport(e.to_string()))?;
            let json: serde_json::Value = resp
                .json()
                .await
                .map_err(|e| L1ClientError::Parse(e.to_string()))?;
            if let Some(error) = json.get("error") {
                return Err(L1ClientError::Rpc(error.to_string()));
            }
            json.get("result")
                .cloned()
                .ok_or_else(|| L1ClientError::Parse("missing 'result' field".into()))
        }
    }

    /// Parse a 32-byte hex hash from a JSON field (e.g.
    /// `"state_root": "0xabcd..."`).
    fn parse_hash_field(value: &serde_json::Value, field: &str) -> Result<[u8; 32], L1ClientError> {
        let hex_str = value
            .get(field)
            .and_then(|v| v.as_str())
            .ok_or_else(|| L1ClientError::Parse(format!("missing '{field}' field")))?;
        let trimmed = hex_str
            .strip_prefix("0x")
            .or_else(|| hex_str.strip_prefix("0X"))
            .unwrap_or(hex_str);
        let bytes =
            hex::decode(trimmed).map_err(|e| L1ClientError::Parse(format!("{field} hex: {e}")))?;
        bytes
            .as_slice()
            .try_into()
            .map_err(|_| L1ClientError::Parse(format!("{field} must be 32 bytes")))
    }

    #[async_trait]
    impl L1Client for RpcL1Client {
        async fn current_l1_height(&self) -> Result<u64, L1ClientError> {
            let result = self
                .rpc_call("suwappu_getEpoch", serde_json::Value::Null)
                .await?;
            result
                .get("latest_committed_round")
                .and_then(|v| v.as_u64())
                .ok_or_else(|| L1ClientError::Parse("missing latest_committed_round".into()))
        }

        async fn current_l2_state_root(
            &self,
            l2_chain_id_hash: &[u8; 32],
        ) -> Result<[u8; 32], L1ClientError> {
            let result = self
                .rpc_call(
                    "suwappu_getL2StateRoot",
                    serde_json::json!({
                        "l2_chain_id_hash": format!("0x{}", hex::encode(l2_chain_id_hash)),
                    }),
                )
                .await?;
            parse_hash_field(&result, "state_root")
        }

        async fn current_l1_state_root(&self) -> Result<[u8; 32], L1ClientError> {
            let result = self
                .rpc_call("suwappu_getL1StateRoot", serde_json::Value::Null)
                .await?;
            parse_hash_field(&result, "state_root")
        }

        async fn read_force_include_registry(&self) -> Result<Vec<u8>, L1ClientError> {
            let result = self
                .rpc_call("suwappu_getForceIncludeRegistry", serde_json::Value::Null)
                .await?;
            let hex_str = result
                .get("data")
                .and_then(|v| v.as_str())
                .ok_or_else(|| L1ClientError::Parse("missing 'data' field".into()))?;
            let trimmed = hex_str
                .strip_prefix("0x")
                .or_else(|| hex_str.strip_prefix("0X"))
                .unwrap_or(hex_str);
            hex::decode(trimmed).map_err(|e| L1ClientError::Parse(format!("data hex: {e}")))
        }

        async fn submit_intent(&self, _intent_bytes: Vec<u8>) -> Result<[u8; 32], L1ClientError> {
            // Phase 2.2-c: the daemon constructs the signed envelope
            // (intent + ML-DSA signature + signer_pubkey_hash) via the
            // SDK before calling this method. For now the read surface
            // is the critical path — batch submission requires the SP1
            // prover (Phase 2.1, #104).
            //
            // Return an explicit error instead of `unimplemented!` so
            // the daemon doesn't panic/abort if this path is reached
            // before SP1 integration lands.
            Err(L1ClientError::Transport(
                "RpcL1Client::submit_intent not yet wired — requires SP1 prover integration (Phase 2.1, #104)".into(),
            ))
        }
    }
}
