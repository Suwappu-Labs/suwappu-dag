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

use anyhow::bail;
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

    let mode = if args.continuous {
        format!("continuous @ {} TPS aggregate", args.rate)
    } else {
        format!("{} TPS for {}s", args.rate, args.duration)
    };
    eprintln!(
        "gsx-loadgen: {} targets — {} (DAG-S28.2 per-target parallel)",
        targets.len(),
        mode
    );

    // DAG-S28.2: per-target parallelization.
    //
    // Pre-S28 the loop was a single tokio task round-robining a Vec of
    // clients. With each `client.submit` being one synchronous
    // write+flush+read_exact roundtrip, per-target throughput capped at
    // 1/RTT (≈ 6–14 TPS cross-region), giving an aggregate ceiling of
    // ~24 TPS for 4 targets. Now: spawn one task per target. Each task
    // owns its own LoadGenClient, drives at `rate / n_targets`, and
    // emits CSV rows through a shared `tokio::sync::mpsc` to a
    // dedicated writer task. The aggregate TPS counter lives in an
    // `AtomicU64` that a reporter task samples every second.
    //
    // Result: aggregate ≈ N_targets × (1/RTT). For 4 cross-region
    // targets at ~75 ms median RTT, that's ~50 TPS sustained — enough
    // to meet the 100 TPS SLA when paired with round_ms=100 (DAG-S28.3
    // on the validator side).

    use std::sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    };

    let run_start = Instant::now();
    let deadline: Option<Instant> = if args.continuous {
        None
    } else {
        Some(run_start + Duration::from_secs(args.duration))
    };

    // CSV header on stdout once, before tasks start.
    println!("client_submitted_ms,tx_hash,target_idx");

    let aggregate_sent = Arc::new(AtomicU64::new(0));
    let report_every = Duration::from_secs(1);

    // mpsc writer: one consumer drains rows and prints them to stdout
    // serially (no interleaving). Producers (one per target task) push
    // (send_ms, tx_hash, target_idx) tuples as each ack arrives.
    let (csv_tx, mut csv_rx) = tokio::sync::mpsc::unbounded_channel::<(u64, [u8; 32], usize)>();
    let writer = tokio::spawn(async move {
        while let Some((send_ms, hash, idx)) = csv_rx.recv().await {
            println!("{},{},{}", send_ms, hex::encode(hash), idx);
        }
    });

    // Per-task rate: split aggregate evenly across targets.
    let per_target_rate = (args.rate as u64 / targets.len() as u64).max(1);
    let per_target_interval = Duration::from_micros(1_000_000 / per_target_rate);
    let per_target_planned = if args.continuous {
        u64::MAX
    } else {
        per_target_rate * args.duration
    };

    let mut task_set = tokio::task::JoinSet::new();
    for (idx, addr) in targets.iter().enumerate() {
        let addr = *addr;
        let csv_tx = csv_tx.clone();
        let aggregate_sent = aggregate_sent.clone();
        let amount = args.amount;
        let seed = args.seed.wrapping_add(idx as u64);
        let interval = per_target_interval;
        let planned = per_target_planned;
        task_set.spawn(async move {
            let mut client = match LoadGenClient::connect(addr).await {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("gsx-loadgen: target {} connect failed: {:#}", idx, e);
                    return;
                }
            };
            let mut rng = StdRng::seed_from_u64(seed);
            let mut next_send = Instant::now();
            let mut sent_local: u64 = 0;
            while sent_local < planned {
                if let Some(d) = deadline {
                    if Instant::now() >= d {
                        break;
                    }
                }
                let from: [u8; 20] = rng.gen();
                let to: [u8; 20] = rng.gen();
                let intent = Intent::Transfer { from, to, amount };
                let send_ms = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|d| d.as_millis() as u64)
                    .unwrap_or(0);
                match client.submit(intent).await {
                    Ok(hash) => {
                        let _ = csv_tx.send((send_ms, hash, idx));
                        aggregate_sent.fetch_add(1, Ordering::Relaxed);
                        sent_local += 1;
                    }
                    Err(e) => {
                        eprintln!(
                            "gsx-loadgen: submit failed on target {} ({}): {:#}",
                            idx, addr, e
                        );
                    }
                }
                next_send += interval;
                let now = Instant::now();
                if next_send > now {
                    tokio::time::sleep(next_send - now).await;
                }
            }
        });
    }
    drop(csv_tx); // close writer once all task clones are dropped

    // Reporter task: every second, print the aggregate TPS observed in
    // the last window. Exits when the deadline passes (continuous mode
    // exits via Ctrl-C).
    let reporter_aggregate = aggregate_sent.clone();
    let reporter = tokio::spawn(async move {
        let mut window_start = Instant::now();
        let mut last_count: u64 = 0;
        loop {
            tokio::time::sleep(report_every).await;
            let now_count = reporter_aggregate.load(Ordering::Relaxed);
            let delta = now_count.saturating_sub(last_count);
            let elapsed = window_start.elapsed().as_secs_f64();
            eprintln!(
                "gsx-loadgen: {} TPS aggregate (window of {:.2}s, total {} sent)",
                delta as f64 / elapsed.max(0.001),
                elapsed,
                now_count
            );
            last_count = now_count;
            window_start = Instant::now();
            if let Some(d) = deadline {
                if Instant::now() >= d + Duration::from_secs(2) {
                    break;
                }
            }
        }
    });

    // Wait for SIGINT or for every target task to finish. JoinSet
    // lets us drain incrementally and shut down cleanly on signal.
    let drain = async { while task_set.join_next().await.is_some() {} };
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {
            eprintln!("gsx-loadgen: shutdown signal — aborting targets");
            task_set.shutdown().await;
        }
        _ = drain => {}
    }

    reporter.abort();
    let _ = writer.await;

    eprintln!(
        "gsx-loadgen: done — {} intents acked across {} targets",
        aggregate_sent.load(Ordering::Relaxed),
        targets.len()
    );
    Ok(())
}
