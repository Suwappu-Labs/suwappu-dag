//! gsx-loadgen — submit transfer intents at a configured rate.
//!
//! Connects to one or more validator client ports and fires
//! `Intent::Transfer` at `--rate` per second. Each submitted intent
//! emits a `lane=client event=submitted` event on the validator side
//! (with the intent hash); the load generator prints the same hash +
//! a wall-clock millis timestamp + target index to stdout, so
//! `gsx-metrics` can join the two streams.
//!
//! Multi-target mode (DAG-S26.2): supply a comma-separated list via
//! `--targets host1:port,host2:port,...`. The generator opens one TCP
//! connection per target, round-robins each submission across them.
//! Combined with the existing peer-to-peer mesh (each validator dials
//! every other), this exercises every n × (n-1) flow in the cluster
//! for bank-compliance load profiling.
//!
//! Continuous mode (DAG-S26.2): `--continuous` ignores `--duration`
//! and runs until SIGINT / SIGTERM. Useful as a systemd-supervised
//! service driving sustained load for SLA observation.
//!
//! Usage:
//!
//! ```text
//! # Single-target, one-shot (legacy):
//! gsx-loadgen --target 127.0.0.1:19100 --rate 100 --duration 30
//!
//! # Multi-target, continuous (compliance load):
//! gsx-loadgen --targets 10.0.1.1:9091,10.0.2.1:9091,10.0.3.1:9091,10.0.4.1:9091 \
//!             --rate 400 --continuous
//! ```

use std::{
    net::SocketAddr,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::{bail, Context};
use clap::Parser;
use gsx_execution::Intent;
use gsx_node::client::LoadGenClient;
use rand::{rngs::StdRng, Rng, SeedableRng};

#[derive(Parser, Debug)]
#[command(
    name = "gsx-loadgen",
    version,
    about = "Submit transfer intents to one or more GSX validators"
)]
struct Args {
    /// Single validator client_listen address. Mutually exclusive
    /// with `--targets`.
    #[arg(long)]
    target: Option<SocketAddr>,

    /// Comma-separated list of validator client_listen addresses for
    /// multi-target mode (DAG-S26.2). Submissions round-robin across
    /// every connection so every TCP edge in the cluster sees load.
    #[arg(long, value_delimiter = ',')]
    targets: Vec<SocketAddr>,

    /// Total intents per second across ALL targets (round-robin).
    #[arg(long, default_value_t = 100)]
    rate: u32,

    /// Total seconds to run. Ignored when `--continuous` is set.
    #[arg(long, default_value_t = 30)]
    duration: u64,

    /// Run until SIGINT/SIGTERM. Pairs with `gsx-loadgen.service`
    /// systemd unit (DAG-S26.3) for sustained-load compliance runs.
    #[arg(long, default_value_t = false)]
    continuous: bool,

    /// Per-intent transfer amount (constant for the run).
    #[arg(long, default_value_t = 1)]
    amount: u128,

    /// Deterministic RNG seed for repeatable runs.
    #[arg(long, default_value_t = 0)]
    seed: u64,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    // Resolve target list — accept either `--target` (single) or
    // `--targets` (multi). Reject both for clarity.
    let targets: Vec<SocketAddr> = match (args.target, args.targets.is_empty()) {
        (Some(t), true) => vec![t],
        (None, false) => args.targets.clone(),
        (Some(_), false) => {
            bail!("use either --target or --targets, not both");
        }
        (None, true) => {
            bail!("must specify --target <addr> or --targets <addr,addr,...>");
        }
    };

    eprintln!(
        "gsx-loadgen: connecting to {} target(s): {:?}",
        targets.len(),
        targets
    );

    let mut clients: Vec<LoadGenClient> = Vec::with_capacity(targets.len());
    for addr in &targets {
        let c = LoadGenClient::connect(*addr)
            .await
            .with_context(|| format!("connect to validator at {}", addr))?;
        clients.push(c);
    }

    let mode = if args.continuous {
        format!("continuous @ {} TPS", args.rate)
    } else {
        format!("{} TPS for {}s", args.rate, args.duration)
    };
    eprintln!(
        "gsx-loadgen: {} targets connected — {}",
        targets.len(),
        mode
    );

    let interval = Duration::from_micros(1_000_000 / args.rate.max(1) as u64);
    let mut rng = StdRng::seed_from_u64(args.seed);
    let mut next_send = Instant::now();

    // CSV header on stdout. gsx-metrics joins client_submitted_ms by
    // tx_hash; `target_idx` lets per-validator load attribution.
    println!("client_submitted_ms,tx_hash,target_idx");

    // Per-second TPS counter for operator visibility.
    let mut window_start = Instant::now();
    let mut window_count: u64 = 0;
    let report_every = Duration::from_secs(1);

    let total_planned = if args.continuous {
        u64::MAX
    } else {
        args.rate as u64 * args.duration
    };
    let mut sent: u64 = 0;
    let mut target_idx: usize = 0;

    // Graceful shutdown: SIGINT/SIGTERM stops the loop.
    let shutdown = tokio::signal::ctrl_c();
    tokio::pin!(shutdown);

    while sent < total_planned {
        // Bail out on signal between intents.
        if tokio::time::timeout(Duration::from_millis(0), &mut shutdown)
            .await
            .is_ok()
        {
            eprintln!("gsx-loadgen: shutdown signal — flushing and exiting");
            break;
        }

        let from: [u8; 20] = rng.gen();
        let to: [u8; 20] = rng.gen();
        let intent = Intent::Transfer {
            from,
            to,
            amount: args.amount,
        };
        let send_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);

        let client = &mut clients[target_idx];
        match client.submit(intent).await {
            Ok(hash) => {
                println!("{},{},{}", send_ms, hex::encode(hash), target_idx);
                window_count += 1;
                sent += 1;
            }
            Err(e) => {
                eprintln!(
                    "gsx-loadgen: submit failed on target {} ({}): {:#}",
                    target_idx, targets[target_idx], e
                );
                // Don't increment sent — let the loop retry next tick.
                // Could re-connect here for production hardening (S26.7).
            }
        }
        target_idx = (target_idx + 1) % targets.len();

        next_send += interval;
        let now = Instant::now();
        if next_send > now {
            tokio::time::sleep(next_send - now).await;
        }

        // Per-second TPS report to stderr.
        if window_start.elapsed() >= report_every {
            eprintln!(
                "gsx-loadgen: {} TPS submitted (window of {:.2}s)",
                window_count,
                window_start.elapsed().as_secs_f64()
            );
            window_count = 0;
            window_start = Instant::now();
        }
    }

    eprintln!("gsx-loadgen: done — {} intents submitted total", sent);
    Ok(())
}
