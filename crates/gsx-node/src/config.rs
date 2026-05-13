//! Daemon configuration — TOML-loaded.
//!
//! One config file per validator process; the perf testnet ships seven of
//! these (one per AWS region) plus a shared `genesis.toml` that lists every
//! validator's public key + stake. See `terraform/perf/templates/` for the
//! per-region rendering.

use std::{
    net::SocketAddr,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

/// Per-validator runtime config.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct NodeConfig {
    /// Human label for this validator. Becomes the `PeerId` on the wire and
    /// the `region` field on event-log lines. Convention: AWS region name,
    /// e.g. `"us-east-1"`.
    pub self_id: String,

    /// 0-indexed Authority Ring id. Must match this node's entry in the
    /// genesis manifest. Maps to `gsx_consensus::AuthorityId`.
    pub authority_id: u32,

    /// Local TCP socket to bind for peer connections.
    pub listen: SocketAddr,

    /// Local TCP socket to bind for client intent submission (load generator
    /// connects here). Distinct from `listen` to keep operator surfaces apart.
    pub client_listen: SocketAddr,

    /// Peers this node should dial. List excludes self.
    pub peers: Vec<Peer>,

    /// Round cadence in milliseconds. Mysticeti-C round = one DAG layer.
    /// Paper uses ~250 ms for the testnet; tune down for low-latency regions.
    #[serde(default = "default_round_ms")]
    pub round_ms: u64,

    /// Checkpoint cadence in rounds. Default 1 = checkpoint every round
    /// (matches `gsx_execution::DEFAULT_CHECKPOINT_CADENCE_ROUNDS`).
    #[serde(default = "default_checkpoint_cadence")]
    pub checkpoint_cadence_rounds: u32,

    /// Path to the ML-DSA-65 secret key file (raw bytes, no envelope).
    pub mldsa_secret_key_path: PathBuf,

    /// Path to the BLS12-381 secret key file (used for LTP corridor
    /// attestations and the fast-path co-signature). 32 raw bytes.
    pub bls_secret_key_path: PathBuf,

    /// Path to the shared genesis manifest. Same file on every validator.
    pub genesis_manifest_path: PathBuf,

    /// Where to write the structured event log (NDJSON, one line per event).
    /// `gsx-metrics` tails this file.
    pub event_log_path: PathBuf,
}

/// One peer entry inside [`NodeConfig::peers`].
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Peer {
    /// Human label / `PeerId` value sent over the wire.
    pub id: String,
    /// Network address to dial.
    pub addr: SocketAddr,
}

/// Shared genesis manifest. Identical across every validator's filesystem.
/// Generated once via `scripts/perf/gen-genesis.sh` and shipped via S3.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct GenesisManifest {
    /// Network identifier — included in every signed payload so cross-network
    /// replay attacks are impossible. Free-form ASCII, e.g. `"gsx-perf-7r"`.
    pub network_id: String,

    /// Ordered list of validators. Index = `authority_id`.
    pub validators: Vec<GenesisValidator>,
}

/// One validator's public-key bundle in the genesis manifest.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct GenesisValidator {
    /// 0-indexed Authority Ring id.
    pub authority_id: u32,
    /// Human label (matches the validator's `NodeConfig::self_id`).
    pub label: String,
    /// ML-DSA-65 public key, hex-encoded.
    pub mldsa_public_key_hex: String,
    /// BLS12-381 public key, hex-encoded.
    pub bls_public_key_hex: String,
    /// Validator-Ring stake in GSX (paper Definition 1). `u64` rather than
    /// `u128` because the `toml` crate doesn't deserialize `u128`; widened
    /// to `u128` at the `StakeTable` boundary which matches the consensus
    /// crate's `Stake` type.
    pub validator_stake_gsx: u64,
    /// Authority-Ring stake in GSX (paper Definition 1). Same `u64` rationale.
    pub authority_stake_gsx: u64,
}

fn default_round_ms() -> u64 {
    250
}

fn default_checkpoint_cadence() -> u32 {
    1
}

/// Errors from loading config / genesis off disk.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// File read failed.
    #[error("read {0}: {1}")]
    Read(PathBuf, std::io::Error),
    /// TOML parse failed.
    #[error("parse {0}: {1}")]
    Parse(PathBuf, toml::de::Error),
    /// Genesis manifest is missing the entry for the configured authority id.
    #[error("authority_id {0} not found in genesis manifest")]
    MissingAuthority(u32),
    /// authority_id present in genesis but label doesn't match self_id —
    /// indicates a config/manifest desync.
    #[error("self_id '{self_id}' does not match genesis label '{manifest}' at authority_id {id}")]
    LabelMismatch {
        /// Authority ring id.
        id: u32,
        /// `self_id` from local config.
        self_id: String,
        /// `label` from manifest.
        manifest: String,
    },
}

impl NodeConfig {
    /// Load from a TOML file.
    pub fn from_path(path: impl AsRef<Path>) -> Result<Self, ConfigError> {
        let path = path.as_ref();
        let text =
            std::fs::read_to_string(path).map_err(|e| ConfigError::Read(path.to_path_buf(), e))?;
        toml::from_str(&text).map_err(|e| ConfigError::Parse(path.to_path_buf(), e))
    }
}

impl GenesisManifest {
    /// Load from a TOML file.
    pub fn from_path(path: impl AsRef<Path>) -> Result<Self, ConfigError> {
        let path = path.as_ref();
        let text =
            std::fs::read_to_string(path).map_err(|e| ConfigError::Read(path.to_path_buf(), e))?;
        toml::from_str(&text).map_err(|e| ConfigError::Parse(path.to_path_buf(), e))
    }

    /// Cross-check the manifest against a [`NodeConfig`]: the configured
    /// `authority_id` must exist in `validators`, and its `label` must match
    /// `self_id`.
    pub fn validate_against(&self, cfg: &NodeConfig) -> Result<&GenesisValidator, ConfigError> {
        let entry = self
            .validators
            .iter()
            .find(|v| v.authority_id == cfg.authority_id)
            .ok_or(ConfigError::MissingAuthority(cfg.authority_id))?;
        if entry.label != cfg.self_id {
            return Err(ConfigError::LabelMismatch {
                id: cfg.authority_id,
                self_id: cfg.self_id.clone(),
                manifest: entry.label.clone(),
            });
        }
        Ok(entry)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_minimal_config() {
        let toml_src = r#"
            self_id = "us-east-1"
            authority_id = 0
            listen = "0.0.0.0:9090"
            client_listen = "0.0.0.0:9091"
            mldsa_secret_key_path = "/var/lib/gsx/mldsa.sk"
            bls_secret_key_path = "/var/lib/gsx/bls.sk"
            genesis_manifest_path = "/var/lib/gsx/genesis.toml"
            event_log_path = "/var/log/gsx/events.ndjson"

            [[peers]]
            id = "eu-west-1"
            addr = "10.0.1.1:9090"

            [[peers]]
            id = "ap-northeast-1"
            addr = "10.0.2.1:9090"
        "#;
        let cfg: NodeConfig = toml::from_str(toml_src).unwrap();
        assert_eq!(cfg.self_id, "us-east-1");
        assert_eq!(cfg.authority_id, 0);
        assert_eq!(cfg.peers.len(), 2);
        // Defaults applied.
        assert_eq!(cfg.round_ms, 250);
        assert_eq!(cfg.checkpoint_cadence_rounds, 1);
    }

    #[test]
    fn genesis_validate_catches_label_mismatch() {
        let manifest = GenesisManifest {
            network_id: "perf".into(),
            validators: vec![GenesisValidator {
                authority_id: 0,
                label: "us-west-2".into(),
                mldsa_public_key_hex: "00".into(),
                bls_public_key_hex: "00".into(),
                validator_stake_gsx: 1,
                authority_stake_gsx: 1,
            }],
        };
        let cfg = NodeConfig {
            self_id: "us-east-1".into(),
            authority_id: 0,
            listen: "0.0.0.0:9090".parse().unwrap(),
            client_listen: "0.0.0.0:9091".parse().unwrap(),
            peers: vec![],
            round_ms: 250,
            checkpoint_cadence_rounds: 1,
            mldsa_secret_key_path: "/x".into(),
            bls_secret_key_path: "/x".into(),
            genesis_manifest_path: "/x".into(),
            event_log_path: "/x".into(),
        };
        let err = manifest.validate_against(&cfg).unwrap_err();
        assert!(matches!(err, ConfigError::LabelMismatch { id: 0, .. }));
    }
}
