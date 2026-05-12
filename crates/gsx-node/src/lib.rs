//! gsx-node — full validator composition library surface.
//!
//! The binary lives at `src/main.rs`. The integration logic exercised
//! by DAG-S20's E2E proptest is exposed as a library here so the
//! property tests can drive multi-validator genesis scenarios.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod validator;

pub use validator::{run_genesis_flow_with_keys, seed_registry, NodeError, Validator};
