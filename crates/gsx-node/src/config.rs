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

    /// Optional local TCP socket to bind for the JSON-RPC query API
    /// (`gsx-rpc`). When unset, the RPC server is not started — this is
    /// the perf-testnet default so the cluster's peer-to-peer cost
    /// measurements aren't perturbed by an external read API.
    ///
    /// **Security:** bind to `127.0.0.1:<port>` when running behind an
    /// ALB or reverse proxy — never `0.0.0.0` on a public-facing host.
    /// The devnet security group opens `rpc_port` (9092) to 0.0.0.0/0
    /// for convenience; production deployments should restrict the SG
    /// ingress to the ALB's security group and bind here to loopback.
    #[serde(default)]
    pub rpc_listen: Option<SocketAddr>,

    /// G6: optional local socket for the Prometheus text-format
    /// metrics endpoint. Defaults to UNSET (perf testnet's posture —
    /// nothing extra runs unless asked). Devnet sets this to
    /// `127.0.0.1:9093` so the local amazon-cloudwatch-agent can
    /// scrape it; the security group does NOT open 9093 to the
    /// outside.
    #[serde(default)]
    pub metrics_listen: Option<SocketAddr>,

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

    /// B1 hardening: cap concurrent open connections on the
    /// `client_listen` socket. The 257th simultaneous connection
    /// is rejected (the listener immediately closes the accepted
    /// socket). Defaults to 256, which is well above the perf-
    /// testnet's typical concurrent-loadgen footprint. Set lower
    /// for resource-constrained deployments or higher for
    /// public-facing validators expecting many client wallets.
    #[serde(default = "default_max_client_connections")]
    pub max_client_connections: u32,

    /// B1 hardening: close an inbound client connection if it sits
    /// idle (no frame received) for this many milliseconds.
    /// Defaults to 30,000 (30 s). Set to 0 to disable the idle
    /// timeout. A patient slow-loris-style attacker can otherwise
    /// hold many connections open by trickling bytes; combined
    /// with `max_client_connections` this caps the resource cost.
    #[serde(default = "default_client_idle_timeout_ms")]
    pub client_idle_timeout_ms: u64,

    /// B1 hardening: cap concurrent open connections from any
    /// single source IP. The N+1th connection from the same IP is
    /// rejected at accept time. Defaults to 8 — enough headroom for
    /// a wallet + a re-tried loadgen + a couple of explorers from
    /// the same NAT, but tight enough that a single source can't
    /// monopolize the listener.
    #[serde(default = "default_client_per_ip_limit")]
    pub client_per_ip_limit: u32,

    /// B2.1 hardening: per-IP token-bucket burst allowance on the
    /// JSON-RPC ingress. The N+1th request from a single source IP
    /// (after exhausting `rpc_per_ip_capacity` tokens) is rejected
    /// with JSON-RPC error code `-32099` (`RateLimited`). Tokens
    /// refill at `rpc_per_ip_refill_per_sec`/sec. Defaults to 60 —
    /// covers a typical wallet's startup-time state-query burst.
    #[serde(default = "default_rpc_per_ip_capacity")]
    pub rpc_per_ip_capacity: u64,

    /// B2.1 hardening: per-IP steady-state ceiling for JSON-RPC
    /// requests, in requests/sec. Defaults to 10 — a 10 Hz polling
    /// loop is the steady-state limit; 100 Hz scripted loops are
    /// throttled toward 10 Hz, abuse floods are bounded.
    #[serde(default = "default_rpc_per_ip_refill_per_sec")]
    pub rpc_per_ip_refill_per_sec: u64,

    /// B2.2 hardening: per-request wall-clock timeout for JSON-RPC
    /// requests, in milliseconds. Requests exceeding this window are
    /// cancelled; the caller receives HTTP 408. Defaults to 30 000
    /// (30 s) — generous for any read method; `gsx_submitIntent` is
    /// bounded by ML-DSA verify (~2 ms) + mempool enqueue (~us).
    #[serde(default = "default_rpc_request_timeout_ms")]
    pub rpc_request_timeout_ms: u64,

    /// B2 hardening: cap on a single JSON-RPC request body in bytes.
    /// Defaults to 1 MiB (1,048,576). The largest expected payload
    /// is `gsx_submitIntent` with an ML-DSA signature (3,309 B) +
    /// a bincoded intent (~1 KB).
    #[serde(default = "default_rpc_max_request_body_bytes")]
    pub rpc_max_request_body_bytes: usize,

    /// B2 hardening: cap on simultaneous in-flight JSON-RPC requests
    /// across all source IPs. Defaults to 64.
    #[serde(default = "default_rpc_max_concurrent_requests")]
    pub rpc_max_concurrent_requests: usize,
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

    /// LTP corridor registry (DAG-S24). Each entry pins the 9 super-nodes
    /// authorized to attest the (source_chain → target_chain) corridor.
    /// Daemons use this to verify inbound `CorridorAttestation` BLS
    /// aggregates against pinned super-node keys before accepting them.
    ///
    /// Optional for backward compatibility — manifests without a
    /// `[[corridors]]` section accept attestations unverified (matches
    /// pre-S24 MVP behavior).
    #[serde(default)]
    pub corridors: Vec<CorridorConfig>,

    /// Number of rounds per epoch (DAG-S25 Phase G). Governance actions
    /// (authority admission, exit, ejection) queue during the epoch and
    /// apply atomically when the round driver crosses a boundary. Default
    /// 1024 — short enough for testnet iteration, long enough that
    /// boundary work doesn't dominate the round budget.
    #[serde(default = "default_rounds_per_epoch")]
    pub rounds_per_epoch: u64,

    /// Pre-genesis balance allocations. Applied as a single
    /// `Intent::GenesisAllocation` to the substrate before round 0.
    /// Generated by `scripts/devnet/gen-genesis.py` into a
    /// `[[prebalances]]` section of `genesis.toml`.
    ///
    /// Optional for backward compatibility — manifests without a
    /// `[[prebalances]]` section start with an empty balance map.
    #[serde(default)]
    pub prebalances: Vec<GenesisBalance>,
}

/// One LTP corridor — exactly 9 super-nodes attesting for a (source, target)
/// chain pair. Paper §10.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct CorridorConfig {
    /// Corridor identifier. Unique within the manifest.
    pub id: u32,
    /// Source chain id observed by the super-nodes.
    pub source_chain: u64,
    /// Target chain id receiving attestations.
    pub target_chain: u64,
    /// Exactly `LTP_ATTESTATION_QUORUM_SIZE` (9) super-node entries.
    pub members: Vec<SuperNodeConfig>,
}

/// One super-node entry inside a corridor.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SuperNodeConfig {
    /// Authority ring id this super-node operates under.
    pub authority: u32,
    /// BLS12-381 public key, hex-encoded.
    pub bls_public_key_hex: String,
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

/// One pre-genesis balance allocation entry.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct GenesisBalance {
    /// 0x-prefixed hex address (20 bytes).
    pub address: String,
    /// Initial balance in GSX. `u64` because the `toml` crate doesn't
    /// deserialize `u128`; widened to `u128` at the substrate boundary.
    pub balance_gsx: u64,
    /// Informational label (e.g. `"faucet"`). Not used by the daemon.
    #[serde(default)]
    pub role: Option<String>,
}

fn default_round_ms() -> u64 {
    250
}

fn default_checkpoint_cadence() -> u32 {
    1
}

fn default_rounds_per_epoch() -> u64 {
    1024
}

fn default_max_client_connections() -> u32 {
    256
}

fn default_client_idle_timeout_ms() -> u64 {
    30_000
}

fn default_client_per_ip_limit() -> u32 {
    8
}

fn default_rpc_per_ip_capacity() -> u64 {
    60
}

fn default_rpc_per_ip_refill_per_sec() -> u64 {
    10
}

fn default_rpc_request_timeout_ms() -> u64 {
    30_000
}

fn default_rpc_max_request_body_bytes() -> usize {
    1024 * 1024
}

fn default_rpc_max_concurrent_requests() -> usize {
    64
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
            corridors: Vec::new(),
            rounds_per_epoch: 1024,
            prebalances: vec![],
        };
        let cfg = NodeConfig {
            self_id: "us-east-1".into(),
            authority_id: 0,
            listen: "0.0.0.0:9090".parse().unwrap(),
            client_listen: "0.0.0.0:9091".parse().unwrap(),
            rpc_listen: None,
            peers: vec![],
            round_ms: 250,
            checkpoint_cadence_rounds: 1,
            mldsa_secret_key_path: "/x".into(),
            bls_secret_key_path: "/x".into(),
            genesis_manifest_path: "/x".into(),
            event_log_path: "/x".into(),

            max_client_connections: 256,
            client_idle_timeout_ms: 30_000,
            client_per_ip_limit: 8,
            rpc_per_ip_capacity: 60,
            rpc_per_ip_refill_per_sec: 10,
            rpc_request_timeout_ms: 30_000,
            rpc_max_request_body_bytes: 1024 * 1024,
            rpc_max_concurrent_requests: 64,
            metrics_listen: None,
        };
        let err = manifest.validate_against(&cfg).unwrap_err();
        assert!(matches!(err, ConfigError::LabelMismatch { id: 0, .. }));
    }
}
