//! Sequencer daemon configuration. TOML-loaded; the deploy
//! pipeline writes the file via cloud-init (matching the
//! gsx-validator-program shape).

use std::{fs, path::Path};

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Errors returned by [`SequencerConfig::load_from_path`].
#[derive(Debug, Error)]
pub enum ConfigError {
    /// Could not read the file (missing, permissions, etc.).
    #[error("reading config file {path}: {source}")]
    Io {
        /// Path attempted.
        path: String,
        /// Underlying OS error.
        #[source]
        source: std::io::Error,
    },

    /// File contents are not valid TOML, or the schema doesn't
    /// match [`SequencerConfig`].
    #[error("parsing config file {path}: {source}")]
    Parse {
        /// Path attempted.
        path: String,
        /// Underlying TOML error.
        #[source]
        source: toml::de::Error,
    },

    /// A field that must be non-empty was provided as the
    /// empty string. Caught at load time so the daemon doesn't
    /// silently use a "" RPC URL or empty chain id.
    #[error("config field `{field}` cannot be empty")]
    EmptyRequiredField {
        /// Name of the empty field.
        field: &'static str,
    },
}

/// Sequencer daemon configuration. Loaded once at startup;
/// the file path is supplied via `--config` on the CLI.
///
/// ## Why TOML
///
/// Matches the gsx-validator-program + gsx-faucet + gsx-node
/// posture across the repo. Cloud-init writes the file, the
/// daemon loads + validates, fail-fast on bad config.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SequencerConfig {
    /// L1 substrate JSON-RPC URL. The daemon submits L1
    /// Intents here (PostL2DA, CommitL2StateRoot, etc.) and
    /// polls for state changes (force-include obligations,
    /// L1 block height).
    pub l1_rpc_url: String,

    /// L2 chain identifier. Used as input to the
    /// `l2_chain_id_hash` field of the public-input layout,
    /// per `gsx_l2_stm::to_public_inputs`.
    pub l2_chain_id: String,

    /// Address (host:port) the sequencer's JSON-RPC server
    /// binds to. The L2 RPC fronting (ALB / CloudFront) reaches
    /// the daemon here.
    pub rpc_bind_addr: String,

    /// Path to the file containing the sequencer's signing
    /// key (PEM or hex per the gsx-faucet convention). The
    /// daemon loads + holds the key in memory for the
    /// lifetime of the process.
    pub signer_key_path: String,

    /// How often (in milliseconds) the batch-builder runs.
    /// Default 250-500 ms per Phase 2.2 plan; smaller values
    /// increase L1 traffic, larger values increase L2 latency.
    #[serde(default = "default_batch_interval_ms")]
    pub batch_interval_ms: u64,

    /// How often (in L1 blocks) the force-include watcher
    /// re-evaluates obligations. Default 1 (every L1 block);
    /// raise if RPC budget is tight.
    #[serde(default = "default_force_include_interval_l1_blocks")]
    pub force_include_interval_l1_blocks: u64,

    /// Tokio worker-thread count. 0 = use the default
    /// (CPUs available). Set explicitly to bound the daemon's
    /// CPU footprint on shared instances.
    #[serde(default)]
    pub tokio_worker_threads: usize,
}

fn default_batch_interval_ms() -> u64 {
    // 250 ms matches the lower end of the Phase 2.2 plan
    // (250-500 ms cadence). The substrate's per-Intent
    // throughput accepts this rate trivially; the bottleneck
    // is the prover's batch-rate.
    250
}

fn default_force_include_interval_l1_blocks() -> u64 {
    1
}

impl SequencerConfig {
    /// Load + validate from a TOML file.
    pub fn load_from_path(path: impl AsRef<Path>) -> Result<Self, ConfigError> {
        let path_ref = path.as_ref();
        let path_str = path_ref.display().to_string();

        let raw = fs::read_to_string(path_ref).map_err(|source| ConfigError::Io {
            path: path_str.clone(),
            source,
        })?;
        let cfg: Self = toml::from_str(&raw).map_err(|source| ConfigError::Parse {
            path: path_str,
            source,
        })?;
        cfg.validate()?;
        Ok(cfg)
    }

    /// Field-level validation. Catches empty strings on fields
    /// where empty is never meaningful — RPC URLs, chain ids,
    /// bind addresses, key paths.
    fn validate(&self) -> Result<(), ConfigError> {
        for (name, value) in [
            ("l1_rpc_url", &self.l1_rpc_url),
            ("l2_chain_id", &self.l2_chain_id),
            ("rpc_bind_addr", &self.rpc_bind_addr),
            ("signer_key_path", &self.signer_key_path),
        ] {
            if value.is_empty() {
                return Err(ConfigError::EmptyRequiredField { field: name });
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn minimal_toml() -> &'static str {
        r#"
l1_rpc_url = "https://rpc.testnet.gsx.globalsettlement.com"
l2_chain_id = "gsx-l2-testnet-1"
rpc_bind_addr = "0.0.0.0:8546"
signer_key_path = "/etc/gsx/sequencer.key"
"#
    }

    #[test]
    fn loads_minimal_config_with_defaults() {
        let cfg: SequencerConfig = toml::from_str(minimal_toml()).unwrap();
        cfg.validate().unwrap();
        assert_eq!(
            cfg.l1_rpc_url,
            "https://rpc.testnet.gsx.globalsettlement.com"
        );
        assert_eq!(cfg.batch_interval_ms, 250);
        assert_eq!(cfg.force_include_interval_l1_blocks, 1);
        assert_eq!(cfg.tokio_worker_threads, 0);
    }

    #[test]
    fn round_trips_through_toml() {
        let cfg: SequencerConfig = toml::from_str(minimal_toml()).unwrap();
        let reser = toml::to_string(&cfg).unwrap();
        let reparsed: SequencerConfig = toml::from_str(&reser).unwrap();
        assert_eq!(cfg, reparsed);
    }

    #[test]
    fn rejects_unknown_fields() {
        let bad = r#"
l1_rpc_url = "x"
l2_chain_id = "y"
rpc_bind_addr = "0.0.0.0:1"
signer_key_path = "/k"
totally_made_up = 42
"#;
        let err = toml::from_str::<SequencerConfig>(bad).unwrap_err();
        assert!(err.to_string().contains("totally_made_up"));
    }

    #[test]
    fn rejects_empty_required_fields() {
        let bad = r#"
l1_rpc_url = ""
l2_chain_id = "y"
rpc_bind_addr = "0.0.0.0:1"
signer_key_path = "/k"
"#;
        let cfg: SequencerConfig = toml::from_str(bad).unwrap();
        assert!(matches!(
            cfg.validate(),
            Err(ConfigError::EmptyRequiredField {
                field: "l1_rpc_url"
            })
        ));
    }

    #[test]
    fn load_from_missing_path_surfaces_io_error() {
        let err = SequencerConfig::load_from_path("/tmp/gsx-l2-seq-config-does-not-exist.toml")
            .unwrap_err();
        assert!(matches!(err, ConfigError::Io { .. }));
    }
}
