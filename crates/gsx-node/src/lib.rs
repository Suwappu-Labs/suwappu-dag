//! gsx-node — full validator composition library surface.
//!
//! The binary lives at `src/main.rs`. The integration logic exercised
//! by DAG-S20's E2E proptest is exposed as a library here so the
//! property tests can drive multi-validator genesis scenarios.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod client;
pub mod codec;
pub mod config;
pub mod daemon;
pub mod events;
pub mod metrics_http;
pub mod rpc_adapter;
pub mod validator;
pub mod wire;

pub use config::{
    ConfigError, GenesisBalance, GenesisManifest, GenesisSigner, GenesisValidator, NodeConfig, Peer,
};
pub use daemon::Daemon;
pub use events::{Event, EventLog, Lane};
pub use validator::{
    run_genesis_flow_with_keys, seed_registry, sign_cert, sign_vote, verify_cert_signature,
    verify_vote_signature, NodeError, Validator,
};
pub use wire::{
    BlockPayload, PeerId, Wire, WireConfig, WireError, WireEvent, WireMessage, WireSplit,
    MAX_FRAME_BYTES,
};
