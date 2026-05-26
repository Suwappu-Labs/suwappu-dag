//! L1 substrate client trait.
//!
//! The daemon's read/write surface against the gsx-dag L1 is
//! narrowed to this trait so the wiring layer (Phase 2.2-b
//! batch builder, 2.2-c force-include watcher) is testable in
//! isolation. The real JSON-RPC implementation lands in
//! 2.2-c alongside `gsx-client` integration.
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
//!   `gsx_execution::force_include::decode_map` at the wiring
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

/// Narrow facade over the gsx-dag L1 JSON-RPC surface.
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
    /// `gsx_execution::force_include::decode_map`.
    async fn read_force_include_registry(&self) -> Result<Vec<u8>, L1ClientError>;

    /// Submit a serialized `Intent` to the L1. The wire format
    /// is whatever the gsx-client SDK accepts; the daemon
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
