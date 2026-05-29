//! Prover daemon configuration. TOML-loaded; the deploy pipeline
//! writes the file via cloud-init (matching the gsx-l2-sequencer-daemon
//! + gsx-validator-program shape).

use std::{fs, path::Path};

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Errors returned by [`ProverConfig::load_from_path`].
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

    /// File contents are not valid TOML, or the schema doesn't match
    /// [`ProverConfig`].
    #[error("parsing config file {path}: {source}")]
    Parse {
        /// Path attempted.
        path: String,
        /// Underlying TOML error.
        #[source]
        source: toml::de::Error,
    },

    /// A field that must be non-empty was provided as the empty
    /// string. Caught at load time so the daemon doesn't silently use
    /// a "" RPC URL.
    #[error("config field `{field}` cannot be empty")]
    EmptyRequiredField {
        /// Name of the empty field.
        field: &'static str,
    },

    /// A numeric field was set to a value that would crash the daemon
    /// at startup (e.g. `poll_interval_ms = 0` panics
    /// `tokio::time::interval`). Caught at load time.
    #[error("config field `{field}` must be {constraint}, got {value}")]
    InvalidNumericField {
        /// Name of the invalid field.
        field: &'static str,
        /// Constraint description (e.g. `"non-zero"`).
        constraint: &'static str,
        /// The offending value.
        value: u64,
    },
}

/// Prover daemon configuration. Loaded once at startup; the file path
/// is supplied via `--config` on the CLI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProverConfig {
    /// Sequencer JSON-RPC URL. The daemon polls this for unproven
    /// batches and submits proofs (via `Intent::CommitL2StateRoot`)
    /// back through it. The real client wiring is a follow-up; the
    /// scaffold validates + carries the URL.
    pub sequencer_rpc_url: String,

    /// Address (host:port) the Prometheus `/metrics` endpoint binds
    /// to. Default `127.0.0.1:9600` so the security group never has to
    /// open the port (scraped by a local cloudwatch-agent), matching
    /// gsx-node's metrics posture.
    #[serde(default = "default_metrics_bind_addr")]
    pub metrics_bind_addr: String,

    /// How often (in milliseconds) the poll loop pulls the next
    /// unproven batch. Default 1000 ms.
    #[serde(default = "default_poll_interval_ms")]
    pub poll_interval_ms: u64,

    /// Maximum proving attempts per batch before recording a permanent
    /// failure. Default 5. `1` disables retry.
    #[serde(default = "default_max_attempts")]
    pub max_attempts: u32,

    /// Base backoff between proving retries, in milliseconds. Attempt
    /// `n` waits `base * 2^(n-2)`, capped at `backoff_max_ms`.
    #[serde(default = "default_backoff_base_ms")]
    pub backoff_base_ms: u64,

    /// Cap on the exponential backoff delay, in milliseconds.
    #[serde(default = "default_backoff_max_ms")]
    pub backoff_max_ms: u64,

    /// Tokio worker-thread count. 0 = use the default (CPUs
    /// available). Set explicitly to bound the daemon's CPU footprint.
    #[serde(default)]
    pub tokio_worker_threads: usize,
}

fn default_metrics_bind_addr() -> String {
    "127.0.0.1:9600".to_string()
}

fn default_poll_interval_ms() -> u64 {
    1000
}

fn default_max_attempts() -> u32 {
    5
}

fn default_backoff_base_ms() -> u64 {
    500
}

fn default_backoff_max_ms() -> u64 {
    30_000
}

impl ProverConfig {
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

    /// Field-level validation. Catches empty strings on fields where
    /// empty is never meaningful and numeric values that would crash
    /// the daemon at startup (`poll_interval_ms = 0` /
    /// `max_attempts = 0`).
    pub fn validate(&self) -> Result<(), ConfigError> {
        for (name, value) in [
            ("sequencer_rpc_url", &self.sequencer_rpc_url),
            ("metrics_bind_addr", &self.metrics_bind_addr),
        ] {
            if value.is_empty() {
                return Err(ConfigError::EmptyRequiredField { field: name });
            }
        }
        if self.poll_interval_ms == 0 {
            return Err(ConfigError::InvalidNumericField {
                field: "poll_interval_ms",
                constraint: "non-zero (tokio::time::interval panics on Duration::ZERO)",
                value: self.poll_interval_ms,
            });
        }
        if self.max_attempts == 0 {
            return Err(ConfigError::InvalidNumericField {
                field: "max_attempts",
                constraint: "at least 1",
                value: self.max_attempts as u64,
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn minimal_toml() -> &'static str {
        r#"
sequencer_rpc_url = "https://rpc.testnet.gsx.globalsettlement.com"
"#
    }

    #[test]
    fn loads_minimal_config_with_defaults() {
        let cfg: ProverConfig = toml::from_str(minimal_toml()).unwrap();
        cfg.validate().unwrap();
        assert_eq!(
            cfg.sequencer_rpc_url,
            "https://rpc.testnet.gsx.globalsettlement.com"
        );
        assert_eq!(cfg.metrics_bind_addr, "127.0.0.1:9600");
        assert_eq!(cfg.poll_interval_ms, 1000);
        assert_eq!(cfg.max_attempts, 5);
        assert_eq!(cfg.backoff_base_ms, 500);
        assert_eq!(cfg.backoff_max_ms, 30_000);
        assert_eq!(cfg.tokio_worker_threads, 0);
    }

    #[test]
    fn round_trips_through_toml() {
        let cfg: ProverConfig = toml::from_str(minimal_toml()).unwrap();
        let reser = toml::to_string(&cfg).unwrap();
        let reparsed: ProverConfig = toml::from_str(&reser).unwrap();
        assert_eq!(cfg, reparsed);
    }

    #[test]
    fn rejects_unknown_fields() {
        let bad = r#"
sequencer_rpc_url = "x"
totally_made_up = 42
"#;
        let err = toml::from_str::<ProverConfig>(bad).unwrap_err();
        assert!(err.to_string().contains("totally_made_up"));
    }

    #[test]
    fn rejects_empty_required_fields() {
        let bad = r#"
sequencer_rpc_url = ""
"#;
        let cfg: ProverConfig = toml::from_str(bad).unwrap();
        assert!(matches!(
            cfg.validate(),
            Err(ConfigError::EmptyRequiredField {
                field: "sequencer_rpc_url"
            })
        ));
    }

    #[test]
    fn load_from_missing_path_surfaces_io_error() {
        let err = ProverConfig::load_from_path("/tmp/gsx-l2-prover-config-does-not-exist.toml")
            .unwrap_err();
        assert!(matches!(err, ConfigError::Io { .. }));
    }

    #[test]
    fn rejects_zero_poll_interval_ms() {
        let bad = r#"
sequencer_rpc_url = "x"
poll_interval_ms = 0
"#;
        let cfg: ProverConfig = toml::from_str(bad).unwrap();
        assert!(matches!(
            cfg.validate(),
            Err(ConfigError::InvalidNumericField {
                field: "poll_interval_ms",
                value: 0,
                ..
            })
        ));
    }

    #[test]
    fn rejects_zero_max_attempts() {
        let bad = r#"
sequencer_rpc_url = "x"
max_attempts = 0
"#;
        let cfg: ProverConfig = toml::from_str(bad).unwrap();
        assert!(matches!(
            cfg.validate(),
            Err(ConfigError::InvalidNumericField {
                field: "max_attempts",
                value: 0,
                ..
            })
        ));
    }
}
