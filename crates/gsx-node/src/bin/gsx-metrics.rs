//! gsx-metrics — join validator event logs across regions and emit a CSV
//! suitable for matplotlib.
//!
//! Reads one NDJSON file per region (the daemon's `event_log_path`), groups
//! events by `cert_hash`, and emits one CSV row per `(cert_hash, region)`
//! showing when each region observed each lifecycle stage:
//!
//! ```text
//! cert_hash,region,proposed_ms,received_ms,voted_ms,committed_ms
//! 0xabc...,us-east-1,1715553210123,,,1715553210251
//! 0xabc...,eu-west-1,,1715553210178,1715553210179,1715553210289
//! ```
//!
//! Empty cells mean the region didn't emit that event (e.g. only the
//! authoring region has a `proposed` row; others have `received`).
//!
//! Usage:
//!
//! ```text
//! gsx-metrics --logs us-east-1=/path/to/us.ndjson --logs eu-west-1=/path/to/eu.ndjson > main_lane.csv
//! ```

use std::collections::BTreeMap;
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Parser;
use serde::Deserialize;

#[derive(Parser, Debug)]
#[command(name = "gsx-metrics", version, about = "Join GSX validator event logs into CSV")]
struct Args {
    /// One per region: `region=path/to/file.ndjson`. May be passed multiple
    /// times.
    #[arg(long, value_parser = parse_log_arg)]
    logs: Vec<(String, PathBuf)>,

    /// Optional: restrict output to one lane (main, fastpath, ltp, client).
    /// When omitted, only the main lane is emitted (the campaign's default
    /// publishable metric).
    #[arg(long, default_value = "main")]
    lane: String,
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
}

#[derive(Default, Debug)]
struct Row {
    proposed_ms: Option<u64>,
    received_ms: Option<u64>,
    voted_ms: Option<u64>,
    committed_ms: Option<u64>,
}

fn main() -> Result<()> {
    let args = Args::parse();
    // grouped[(cert_hash, region)] = Row
    let mut grouped: BTreeMap<(String, String), Row> = BTreeMap::new();

    for (region, path) in &args.logs {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("read {}", path.display()))?;
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
            if ev.lane != args.lane {
                continue;
            }
            // Sanity: the region we were told in --logs should match the
            // region recorded in the line. Mismatch usually means a log file
            // got mislabeled at collection time.
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
            let key_hash = ev.cert_hash.or(ev.tx_hash);
            let Some(hash) = key_hash else { continue };
            let row = grouped.entry((hash, region.clone())).or_default();
            match ev.event.as_str() {
                "proposed" => row.proposed_ms = Some(ev.t_ms),
                "received" => row.received_ms = Some(ev.t_ms),
                "voted" => row.voted_ms = Some(ev.t_ms),
                "committed" => row.committed_ms = Some(ev.t_ms),
                "submitted" | "certified" | "attested" => { /* other lanes */ }
                _ => {}
            }
        }
    }

    println!("cert_hash,region,proposed_ms,received_ms,voted_ms,committed_ms");
    for ((hash, region), row) in grouped {
        println!(
            "{},{},{},{},{},{}",
            hash,
            region,
            row.proposed_ms.map(|t| t.to_string()).unwrap_or_default(),
            row.received_ms.map(|t| t.to_string()).unwrap_or_default(),
            row.voted_ms.map(|t| t.to_string()).unwrap_or_default(),
            row.committed_ms.map(|t| t.to_string()).unwrap_or_default(),
        );
    }
    Ok(())
}
