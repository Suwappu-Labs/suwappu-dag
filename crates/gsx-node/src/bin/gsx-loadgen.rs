//! gsx-loadgen — submit transfer intents at a configured rate.
//!
//! Connects to one validator's client port and fires `Intent::Transfer` at
//! `--rate` per second for `--duration` seconds. Each submitted intent emits
//! a `lane=client event=submitted` event on the validator side (with the
//! intent hash); the load generator prints the same hash + a wall-clock
//! millis timestamp to stdout, so `gsx-metrics` can join the two streams.
//!
//! Usage:
//!
//! ```text
//! gsx-loadgen --target 127.0.0.1:19100 --rate 100 --duration 30
//! ```

use std::net::SocketAddr;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::Context;
use clap::Parser;
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};

use gsx_execution::Intent;
use gsx_node::client::LoadGenClient;

#[derive(Parser, Debug)]
#[command(name = "gsx-loadgen", version, about = "Submit transfer intents to a GSX validator")]
struct Args {
    /// Validator client_listen address.
    #[arg(long)]
    target: SocketAddr,

    /// Intents per second.
    #[arg(long, default_value_t = 100)]
    rate: u32,

    /// Total seconds to run.
    #[arg(long, default_value_t = 30)]
    duration: u64,

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
    let mut client = LoadGenClient::connect(args.target)
        .await
        .with_context(|| format!("connect to validator at {}", args.target))?;
    eprintln!(
        "gsx-loadgen: connected to {} — {} intents/sec for {}s",
        args.target, args.rate, args.duration
    );

    let total = args.rate as u64 * args.duration;
    let interval = Duration::from_micros(1_000_000 / args.rate as u64);
    let mut rng = StdRng::seed_from_u64(args.seed);
    let mut next_send = Instant::now();

    // CSV header on stdout. gsx-metrics joins client_submitted_ms by tx_hash.
    println!("client_submitted_ms,tx_hash");

    for _ in 0..total {
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
        let hash = client
            .submit(intent)
            .await
            .with_context(|| "submit intent")?;
        println!("{},{}", send_ms, hex::encode(hash));

        next_send += interval;
        let now = Instant::now();
        if next_send > now {
            tokio::time::sleep(next_send - now).await;
        }
    }
    Ok(())
}
