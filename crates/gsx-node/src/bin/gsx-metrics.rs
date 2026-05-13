//! gsx-metrics — multi-mode event-log analyzer for bank-compliance
//! load campaigns (DAG-S26.4).
//!
//! Reads one NDJSON file per region (the daemon's `event_log_path`)
//! plus optionally the loadgen CSV (per intent submission timestamps)
//! and emits one of several CSV outputs selected by `--mode`:
//!
//! - `cert` (default) — per-cert lifecycle as proposed → received →
//!   voted → committed per region. The pre-S26 schema; preserved for
//!   plot.py compatibility.
//! - `e2e` — intent-submit → first-finalized latency per intent per
//!   region. Joins loadgen CSV (tx_hash) to validator `committed`
//!   event's `intent_hashes` field (DAG-S26.1).
//! - `pair` — per-validator-pair edge counts derived from `received`
//!   events' `peer` field. The "every n × (n-1) flow exercised"
//!   evidence row in the compliance report.
//! - `tps` — windowed throughput in 60-second buckets: distinct
//!   committed certs and total intents per bucket.
//! - `recovery` — gaps between consecutive `committed` events
//!   longer than `--recovery-threshold-ms` (default 5000), reported
//!   as outage windows.
//!
//! Usage:
//!
//! ```text
//! # Legacy per-cert lifecycle (unchanged):
//! gsx-metrics --logs us-east-1=us.ndjson --logs eu-west-1=eu.ndjson > certs.csv
//!
//! # Bank-compliance e2e join:
//! gsx-metrics --mode e2e --loadgen-csv loadgen.csv \
//!     --logs us-east-1=us.ndjson --logs eu-west-1=eu.ndjson > e2e.csv
//!
//! # Per-validator-pair edges:
//! gsx-metrics --mode pair --logs us-east-1=us.ndjson --logs eu-west-1=eu.ndjson
//! ```

use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    path::PathBuf,
};

use anyhow::{Context, Result};
use clap::{Parser, ValueEnum};
use serde::Deserialize;

#[derive(Copy, Clone, Debug, ValueEnum)]
enum Mode {
    /// Per-cert lifecycle (proposed/received/voted/committed × region).
    Cert,
    /// Intent submit → first-finalized latency per region.
    E2e,
    /// Per-validator-pair received-event counts.
    Pair,
    /// Windowed throughput buckets (60s).
    Tps,
    /// Outage windows: gaps between consecutive `committed`.
    Recovery,
}

#[derive(Parser, Debug)]
#[command(
    name = "gsx-metrics",
    version,
    about = "Join GSX validator event logs into CSV"
)]
struct Args {
    /// One per region: `region=path/to/file.ndjson`. May be passed
    /// multiple times.
    #[arg(long, value_parser = parse_log_arg)]
    logs: Vec<(String, PathBuf)>,

    /// Optional loadgen CSV from gsx-loadgen stdout. Required for
    /// `--mode e2e`. Format: `client_submitted_ms,tx_hash[,target_idx]`.
    #[arg(long)]
    loadgen_csv: Option<PathBuf>,

    /// Output mode (see top-of-file docs).
    #[arg(long, value_enum, default_value_t = Mode::Cert)]
    mode: Mode,

    /// Lane filter (cert mode only). Defaults to `main`.
    #[arg(long, default_value = "main")]
    lane: String,

    /// Bucket width in milliseconds for `--mode tps`. Default 60s.
    #[arg(long, default_value_t = 60_000)]
    bucket_ms: u64,

    /// Recovery threshold in milliseconds for `--mode recovery`. A
    /// gap between consecutive `committed` events longer than this
    /// is reported as an outage window. Default 5s.
    #[arg(long, default_value_t = 5_000)]
    recovery_threshold_ms: u64,
}

fn parse_log_arg(s: &str) -> Result<(String, PathBuf), String> {
    let (region, path) = s
        .split_once('=')
        .ok_or_else(|| format!("expected region=path, got `{}`", s))?;
    Ok((region.to_string(), PathBuf::from(path)))
}

#[derive(Debug, Deserialize)]
struct Event {
    t_ms: u64,
    region: String,
    lane: String,
    event: String,
    #[serde(default)]
    cert_hash: Option<String>,
    #[serde(default)]
    tx_hash: Option<String>,
    #[serde(default)]
    peer: Option<String>,
    #[serde(default)]
    intent_hashes: Option<Vec<String>>,
}

#[derive(Default, Debug)]
struct CertRow {
    proposed_ms: Option<u64>,
    received_ms: Option<u64>,
    voted_ms: Option<u64>,
    committed_ms: Option<u64>,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let mut all_events: Vec<Event> = Vec::new();
    for (region, path) in &args.logs {
        let text =
            std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
        for (lineno, line) in text.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            let ev: Event = match serde_json::from_str(line) {
                Ok(e) => e,
                Err(e) => {
                    eprintln!(
                        "warning: {} line {}: skipping unparseable line ({})",
                        path.display(),
                        lineno + 1,
                        e
                    );
                    continue;
                }
            };
            if ev.region != *region {
                eprintln!(
                    "warning: {} line {}: log says region={} but --logs says {}",
                    path.display(),
                    lineno + 1,
                    ev.region,
                    region
                );
                continue;
            }
            all_events.push(ev);
        }
    }

    match args.mode {
        Mode::Cert => emit_cert_mode(&all_events, &args.lane),
        Mode::E2e => emit_e2e_mode(&all_events, &args.loadgen_csv)?,
        Mode::Pair => emit_pair_mode(&all_events),
        Mode::Tps => emit_tps_mode(&all_events, args.bucket_ms),
        Mode::Recovery => emit_recovery_mode(&all_events, args.recovery_threshold_ms),
    }
    Ok(())
}

/// Per-cert lifecycle (legacy schema).
fn emit_cert_mode(events: &[Event], lane: &str) {
    let mut grouped: BTreeMap<(String, String), CertRow> = BTreeMap::new();
    for ev in events {
        if ev.lane != lane {
            continue;
        }
        let key_hash = ev.cert_hash.clone().or(ev.tx_hash.clone());
        let Some(hash) = key_hash else { continue };
        let row = grouped.entry((hash, ev.region.clone())).or_default();
        match ev.event.as_str() {
            "proposed" => row.proposed_ms = Some(ev.t_ms),
            "received" => row.received_ms = Some(ev.t_ms),
            "voted" => row.voted_ms = Some(ev.t_ms),
            "committed" => row.committed_ms = Some(ev.t_ms),
            _ => {}
        }
    }
    println!("cert_hash,region,proposed_ms,received_ms,voted_ms,committed_ms");
    for ((hash, region), row) in grouped {
        println!(
            "{},{},{},{},{},{}",
            hash,
            region,
            opt(row.proposed_ms),
            opt(row.received_ms),
            opt(row.voted_ms),
            opt(row.committed_ms),
        );
    }
}

/// Intent submit → first finalized commit per region. Bank-compliance
/// e2e latency table. Joins loadgen CSV tx_hash to `committed` events'
/// `intent_hashes`.
fn emit_e2e_mode(events: &[Event], loadgen_csv: &Option<PathBuf>) -> Result<()> {
    let path = loadgen_csv
        .as_ref()
        .context("--mode e2e requires --loadgen-csv")?;
    let csv = std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    // tx_hash → client_submitted_ms (lowest, in case of retries).
    let mut submitted: HashMap<String, u64> = HashMap::new();
    for (i, line) in csv.lines().enumerate() {
        if i == 0 || line.trim().is_empty() {
            continue; // header
        }
        let mut parts = line.split(',');
        let ms = parts.next().and_then(|s| s.parse::<u64>().ok());
        let h = parts.next().map(|s| s.to_string());
        if let (Some(ms), Some(h)) = (ms, h) {
            submitted
                .entry(h)
                .and_modify(|prev| {
                    if ms < *prev {
                        *prev = ms;
                    }
                })
                .or_insert(ms);
        }
    }

    // For each intent: first_committed per region = min(t_ms) of
    // committed events whose intent_hashes contains the tx_hash.
    // Map (tx_hash, region) → first_committed_ms.
    let mut first_committed: HashMap<(String, String), u64> = HashMap::new();
    for ev in events {
        if ev.event != "committed" {
            continue;
        }
        let Some(hashes) = ev.intent_hashes.as_ref() else {
            continue;
        };
        for tx_hash in hashes {
            let key = (tx_hash.clone(), ev.region.clone());
            first_committed
                .entry(key)
                .and_modify(|prev| {
                    if ev.t_ms < *prev {
                        *prev = ev.t_ms;
                    }
                })
                .or_insert(ev.t_ms);
        }
    }

    println!("tx_hash,submitted_ms,region,first_committed_ms,e2e_latency_ms");
    for ((tx_hash, region), committed_ms) in first_committed {
        let submitted_ms = submitted.get(&tx_hash).copied();
        let latency = submitted_ms.map(|s| committed_ms.saturating_sub(s));
        println!(
            "{},{},{},{},{}",
            tx_hash,
            opt(submitted_ms),
            region,
            committed_ms,
            opt(latency),
        );
    }
    Ok(())
}

/// Per-validator-pair edge counts from `received` events. Evidence
/// that every n × (n-1) flow is being exercised.
fn emit_pair_mode(events: &[Event]) {
    // (receiver, sender) → count.
    let mut counts: BTreeMap<(String, String), u64> = BTreeMap::new();
    for ev in events {
        if ev.event != "received" {
            continue;
        }
        let Some(peer) = ev.peer.as_ref() else {
            continue;
        };
        *counts.entry((ev.region.clone(), peer.clone())).or_insert(0) += 1;
    }
    println!("receiver,sender,count");
    for ((rx, tx), c) in counts {
        println!("{},{},{}", rx, tx, c);
    }
}

/// Windowed throughput: distinct committed certs + total intents per
/// `--bucket-ms` window.
fn emit_tps_mode(events: &[Event], bucket_ms: u64) {
    // bucket_start_ms → (distinct certs, intent count).
    let mut buckets: BTreeMap<u64, (BTreeSet<String>, u64)> = BTreeMap::new();
    // DAG-S28.5: track how many committed events lacked an
    // `intent_hashes` field so the operator sees when the TPS column
    // falls back to per-cert counting (one intent per cert).
    let mut commits_total: u64 = 0;
    let mut commits_with_intents: u64 = 0;
    for ev in events {
        if ev.event != "committed" {
            continue;
        }
        commits_total += 1;
        let bucket = (ev.t_ms / bucket_ms) * bucket_ms;
        let entry = buckets.entry(bucket).or_default();
        if let Some(h) = &ev.cert_hash {
            entry.0.insert(h.clone());
        }
        match &ev.intent_hashes {
            Some(hashes) if !hashes.is_empty() => {
                entry.1 += hashes.len() as u64;
                commits_with_intents += 1;
            }
            _ => {
                // DAG-S28.5 fallback: if the committed event has no
                // `intent_hashes` (older binary, lane filter, missing
                // field), count the commit itself as one intent batch
                // so TPS doesn't silently report 0 when commits are
                // landing but intent-trace events aren't.
                entry.1 += 1;
            }
        }
    }
    if commits_total > 0 && commits_with_intents == 0 {
        eprintln!(
            "gsx-metrics tps: warning — {} committed events lacked `intent_hashes`; \
             reporting commit-rate (1 intent per cert) instead of intent-rate. \
             Upgrade validators past DAG-S26.1 for true intent counts.",
            commits_total
        );
    } else if commits_total > 0 && commits_with_intents < commits_total {
        eprintln!(
            "gsx-metrics tps: {} of {} committed events had no `intent_hashes` \
             (fallback applied for those)",
            commits_total - commits_with_intents,
            commits_total
        );
    }
    println!("bucket_start_ms,bucket_end_ms,distinct_certs,intents,tps");
    for (start, (certs, intents)) in buckets {
        let end = start + bucket_ms;
        let tps = intents as f64 / (bucket_ms as f64 / 1000.0);
        println!("{},{},{},{},{:.2}", start, end, certs.len(), intents, tps);
    }
}

/// Outage windows: gaps between consecutive `committed` events longer
/// than `threshold_ms`. One row per outage per region.
fn emit_recovery_mode(events: &[Event], threshold_ms: u64) {
    // region → sorted list of committed timestamps.
    let mut by_region: BTreeMap<String, Vec<u64>> = BTreeMap::new();
    for ev in events {
        if ev.event == "committed" {
            by_region
                .entry(ev.region.clone())
                .or_default()
                .push(ev.t_ms);
        }
    }
    println!("region,gap_start_ms,gap_end_ms,gap_ms");
    for (region, mut times) in by_region {
        times.sort_unstable();
        for w in times.windows(2) {
            let gap = w[1] - w[0];
            if gap > threshold_ms {
                println!("{},{},{},{}", region, w[0], w[1], gap);
            }
        }
    }
}

fn opt<T: std::fmt::Display>(o: Option<T>) -> String {
    o.map(|t| t.to_string()).unwrap_or_default()
}
