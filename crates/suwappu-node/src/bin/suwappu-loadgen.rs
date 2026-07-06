//! suwappu-loadgen — submit transfer intents at a configured rate.
//!
//! Connects to one or more validator client ports and fires
//! `Intent::Transfer` at `--rate` per second. Each submitted intent
//! emits a `lane=client event=submitted` event on the validator side
//! (with the intent hash); the load generator prints the same hash +
//! a wall-clock millis timestamp + target index to stdout, so
//! `suwappu-metrics` can join the two streams.
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
//! Fast-path mode (PERF-2): `--fastpath` submits signed `FastPathTx`s
//! (one fresh single-owner object per submission, nonce 0) instead of
//! transfer intents. Lineage is grounded via `GetLineage` at start and
//! refreshed once per second per target. The stdout CSV keeps the same
//! `client_submitted_ms,tx_hash,target_idx` shape — the hash column
//! carries the tx's `payload_digest`, which is what the validator
//! writes as `cert_hash` on its `lane=fastpath` event lines, so
//! `suwappu-metrics --mode fastpath` joins the two streams directly.
//!
//! Usage:
//!
//! ```text
//! # Single-target, one-shot (legacy):
//! suwappu-loadgen --target 127.0.0.1:19100 --rate 100 --duration 30
//!
//! # Multi-target, continuous (compliance load):
//! suwappu-loadgen --targets 10.0.1.1:9091,10.0.2.1:9091,10.0.3.1:9091,10.0.4.1:9091 \
//!             --rate 400 --continuous
//!
//! # Fast-path latency campaign (PERF-2):
//! suwappu-loadgen --target 127.0.0.1:9091 --fastpath --rate 50 --duration 30
//! ```

use std::{
    net::SocketAddr,
    path::PathBuf,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::{bail, Context};
use clap::Parser;
use rand::{rngs::StdRng, Rng, SeedableRng};
use suwappu_consensus::CertHash;
use suwappu_crypto::mldsa;
use suwappu_execution::Intent;
use suwappu_fastpath::{FastPathTx, OwnedObjectId, OwnerAddress};
use suwappu_node::client::LoadGenClient;

#[derive(Parser, Debug)]
#[command(
    name = "suwappu-loadgen",
    version,
    about = "Submit transfer intents to one or more SUWAPPU validators"
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

    /// Run until SIGINT/SIGTERM. Pairs with `suwappu-loadgen.service`
    /// systemd unit (DAG-S26.3) for sustained-load compliance runs.
    #[arg(long, default_value_t = false)]
    continuous: bool,

    /// PERF-2: submit fast-path transactions instead of transfer
    /// intents. Each submission uses a fresh single-owner object
    /// (nonce 0) so no two txs contend, isolating pure lane latency.
    /// `--batch-size` is ignored (the fast-path wire acks per tx).
    #[arg(long, default_value_t = false)]
    fastpath: bool,

    /// Per-intent transfer amount (constant for the run).
    #[arg(long, default_value_t = 1)]
    amount: u128,

    /// Deterministic RNG seed for repeatable runs. Default `0` is
    /// treated specially: each campaign picks a fresh runtime entropy
    /// seed (current unix nanos), so intent hashes don't collide
    /// across consecutive campaigns sharing a long-lived daemon's
    /// event log (DAG-S29.1 e2e join hazard). Pass any non-zero value
    /// to opt in to a reproducible run — useful for debugging.
    #[arg(long, default_value_t = 0)]
    seed: u64,

    /// Number of intents per batched `ClientMessage::SubmitBatch`
    /// (DAG-S29.2). Pre-S29 the wire protocol acked every intent
    /// individually, capping the loadgen at `n_targets / RTT` ≈ 88
    /// TPS for 4 cross-region targets at 45 ms median ack RTT. With
    /// N intents per ack, the ceiling becomes `N × n_targets / RTT`.
    /// Default 100 lifts the ceiling to ~8,800 TPS; 500 lifts it to
    /// ~44k TPS, beyond what a 4-cert/sec consensus cadence at 12k
    /// intents/cert can commit (~48k TPS hard ceiling). Set to 1 for
    /// the pre-S29 one-intent-one-ack behaviour.
    #[arg(long, default_value_t = 100)]
    batch_size: u32,

    /// Path to a raw ML-DSA-65 secret key on disk. Required (with
    /// `--mldsa-public-key`) for the new Phase 2.6 signed-intent wire
    /// (Issue #28). Mutually exclusive with `--mldsa-secret-key-hex`.
    /// The keypair MUST correspond to a seated Authority Ring member
    /// in the target validators' genesis manifest — otherwise every
    /// submission is rejected with `auth: unknown signer`.
    #[arg(long)]
    mldsa_secret_key: Option<PathBuf>,

    /// Path to the matching ML-DSA-65 public key on disk.
    #[arg(long)]
    mldsa_public_key: Option<PathBuf>,

    /// Hex-encoded ML-DSA-65 secret key. Convenient for CI / inline
    /// configuration; do not use in production (key material ends up
    /// in shell history and process listings). Mutually exclusive
    /// with `--mldsa-secret-key`.
    #[arg(long)]
    mldsa_secret_key_hex: Option<String>,

    /// Hex-encoded ML-DSA-65 public key matching `--mldsa-secret-key-hex`.
    #[arg(long)]
    mldsa_public_key_hex: Option<String>,

    /// Genesis `network_id` mixed into every intent's signing digest
    /// (Issue #28). Must exactly match the validators' manifest
    /// `network_id`, otherwise verification fails.
    #[arg(long)]
    network_id: String,
}

/// Resolve a ML-DSA-65 keypair from the CLI flags. The two accepted
/// shapes are file-pair (`--mldsa-secret-key path --mldsa-public-key path`)
/// or hex-pair (`--mldsa-secret-key-hex hex --mldsa-public-key-hex hex`).
/// Mixing the two is rejected.
fn resolve_keypair(args: &Args) -> anyhow::Result<(mldsa::SecretKey, mldsa::PublicKey)> {
    let from_files = args.mldsa_secret_key.is_some() && args.mldsa_public_key.is_some();
    let from_hex = args.mldsa_secret_key_hex.is_some() && args.mldsa_public_key_hex.is_some();
    match (from_files, from_hex) {
        (true, true) => bail!("specify either --mldsa-*-key OR --mldsa-*-key-hex, not both"),
        (false, false) => bail!(
            "Issue #28: must supply ML-DSA-65 key material. \
             Use --mldsa-secret-key <path> --mldsa-public-key <path>, \
             or --mldsa-secret-key-hex <hex> --mldsa-public-key-hex <hex>."
        ),
        (true, false) => {
            let sk_path = args.mldsa_secret_key.as_ref().unwrap();
            let pk_path = args.mldsa_public_key.as_ref().unwrap();
            let sk_bytes = std::fs::read(sk_path)
                .with_context(|| format!("read --mldsa-secret-key {:?}", sk_path))?;
            let pk_bytes = std::fs::read(pk_path)
                .with_context(|| format!("read --mldsa-public-key {:?}", pk_path))?;
            let sk = mldsa::SecretKey::from_bytes(&sk_bytes)
                .map_err(|e| anyhow::anyhow!("decode ML-DSA secret key: {:?}", e))?;
            let pk = mldsa::PublicKey::from_bytes(&pk_bytes)
                .map_err(|e| anyhow::anyhow!("decode ML-DSA public key: {:?}", e))?;
            Ok((sk, pk))
        }
        (false, true) => {
            let sk_bytes = hex::decode(args.mldsa_secret_key_hex.as_ref().unwrap())
                .context("decode --mldsa-secret-key-hex")?;
            let pk_bytes = hex::decode(args.mldsa_public_key_hex.as_ref().unwrap())
                .context("decode --mldsa-public-key-hex")?;
            let sk = mldsa::SecretKey::from_bytes(&sk_bytes)
                .map_err(|e| anyhow::anyhow!("decode ML-DSA secret key hex: {:?}", e))?;
            let pk = mldsa::PublicKey::from_bytes(&pk_bytes)
                .map_err(|e| anyhow::anyhow!("decode ML-DSA public key hex: {:?}", e))?;
            Ok((sk, pk))
        }
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    // Issue #28: resolve the ML-DSA-65 keypair before any TCP work
    // happens. A missing or malformed key is a config error and
    // should fail fast rather than after the test rig has spun up
    // every per-target task.
    let (signer_sk, signer_pk) = resolve_keypair(&args)?;
    let signer_pkh = blake3::hash(signer_pk.as_bytes());
    eprintln!(
        "suwappu-loadgen: signing with ML-DSA-65, pubkey_hash={}",
        hex::encode(signer_pkh.as_bytes())
    );

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
        "suwappu-loadgen: connecting to {} target(s): {:?}",
        targets.len(),
        targets
    );

    let mode = if args.continuous {
        format!("continuous @ {} TPS aggregate", args.rate)
    } else {
        format!("{} TPS for {}s", args.rate, args.duration)
    };
    eprintln!(
        "suwappu-loadgen: {} targets — {} (DAG-S28.2 per-target parallel)",
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
    // DAG-S29.2: in batched mode, each task fires `batch_size` intents
    // per roundtrip. The sleep interval scales by batch_size so the
    // overall TPS still matches `--rate`. With batch_size=100 and
    // per_target_rate=200, each task does 2 batches/sec (=200 intents/
    // sec/target × 4 targets = 800 TPS aggregate, RTT-amortised by 100).
    // PERF-2 fast-path mode has no batch wire (one ack per tx), so the
    // interval is simply 1/rate per target.
    let batch_size = args.batch_size.max(1) as u64;
    let batches_per_sec = (per_target_rate / batch_size).max(1);
    let per_target_interval = if args.fastpath {
        Duration::from_micros(1_000_000 / per_target_rate)
    } else {
        Duration::from_micros(1_000_000 / batches_per_sec)
    };
    let per_target_planned = if args.continuous {
        u64::MAX
    } else {
        per_target_rate * args.duration
    };
    if args.fastpath {
        eprintln!(
            "suwappu-loadgen: fast-path mode — per-target {} txs/s (one ack per tx)",
            per_target_rate
        );
    } else {
        eprintln!(
            "suwappu-loadgen: per-target {} intents/s in batches of {} ({} batches/s/target)",
            per_target_rate, batch_size, batches_per_sec
        );
    }

    // PERF-2 fast-path mode: owner address = blake3 of the signer's
    // ML-DSA public key. One owner for the whole campaign; per-tx
    // uniqueness comes from the derived object ids.
    let fastpath_owner = OwnerAddress(*blake3::hash(signer_pk.as_bytes()).as_bytes());

    // DAG-S29.1: when --seed is at its default (0), pick a runtime
    // entropy seed so intent hashes don't collide with prior runs'
    // committed events in the (long-lived) validator event log. Each
    // task gets seed_base + target_idx so the 4 targets still emit
    // distinct intent sequences within this campaign.
    let seed_base = if args.seed == 0 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0)
    } else {
        args.seed
    };
    eprintln!(
        "suwappu-loadgen: seed_base = {} (from --seed {})",
        seed_base, args.seed
    );

    let mut task_set = tokio::task::JoinSet::new();
    for (idx, addr) in targets.iter().enumerate() {
        let addr = *addr;
        let csv_tx = csv_tx.clone();
        let aggregate_sent = aggregate_sent.clone();
        let amount = args.amount;
        let seed = seed_base.wrapping_add(idx as u64);
        let interval = per_target_interval;
        let planned = per_target_planned;
        let batch = batch_size;
        let signer_sk = signer_sk.clone();
        let signer_pk = signer_pk.clone();
        let network_id = args.network_id.clone();
        let fastpath = args.fastpath;
        let owner = fastpath_owner;
        task_set.spawn(async move {
            let mut client =
                match LoadGenClient::connect(addr, signer_sk, signer_pk, network_id).await {
                    Ok(c) => c,
                    Err(e) => {
                        eprintln!("suwappu-loadgen: target {} connect failed: {:#}", idx, e);
                        return;
                    }
                };
            if fastpath {
                // PERF-2: ground lineage once at start, refresh at most
                // once per second — a lineage a few rounds stale is fine
                // (the K=4 binding window is measured FROM lineage_round,
                // so fresher is safer, but per-tx roundtrips would halve
                // throughput for no measurement benefit).
                let (mut lineage_round, mut lineage_hash) = match client.get_lineage().await {
                    Ok(l) => l,
                    Err(e) => {
                        eprintln!("suwappu-loadgen: target {} GetLineage failed: {:#}", idx, e);
                        return;
                    }
                };
                let mut last_lineage_refresh = Instant::now();
                let seed_bytes = seed.to_le_bytes();
                let mut next_send = Instant::now();
                let mut sent_local: u64 = 0;
                while sent_local < planned {
                    if let Some(d) = deadline {
                        if Instant::now() >= d {
                            break;
                        }
                    }
                    if last_lineage_refresh.elapsed() >= Duration::from_secs(1) {
                        if let Ok((r, h)) = client.get_lineage().await {
                            lineage_round = r;
                            lineage_hash = h;
                        }
                        last_lineage_refresh = Instant::now();
                    }
                    // Fresh single-owner object per tx (nonce stays 0)
                    // so submissions never contend on an object key.
                    // Derivations are deterministic from (seed, i) —
                    // and seed differs per target task — so objects and
                    // payload digests are campaign-unique.
                    let i_bytes = sent_local.to_le_bytes();
                    let mut h = blake3::Hasher::new();
                    h.update(&seed_bytes);
                    h.update(&i_bytes);
                    let object = *h.finalize().as_bytes();
                    let mut h = blake3::Hasher::new();
                    h.update(&seed_bytes);
                    h.update(&i_bytes);
                    h.update(b"payload");
                    let payload_digest = *h.finalize().as_bytes();
                    let tx = FastPathTx {
                        object: OwnedObjectId(object),
                        owner,
                        nonce: 0,
                        lineage: CertHash(lineage_hash),
                        lineage_round,
                        payload_digest,
                    };
                    let send_ms = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .map(|d| d.as_millis() as u64)
                        .unwrap_or(0);
                    match client.submit_fastpath(tx).await {
                        Ok(digest) => {
                            // Hash column carries the payload_digest —
                            // the join key against `lane=fastpath`
                            // `cert_hash` in the validator event log.
                            let _ = csv_tx.send((send_ms, digest, idx));
                            aggregate_sent.fetch_add(1, Ordering::Relaxed);
                            sent_local += 1;
                        }
                        Err(e) => {
                            eprintln!(
                                "suwappu-loadgen: submit_fastpath failed on target {} ({}): {:#}",
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
                return;
            }
            let mut rng = StdRng::seed_from_u64(seed);
            let mut next_send = Instant::now();
            let mut sent_local: u64 = 0;
            while sent_local < planned {
                if let Some(d) = deadline {
                    if Instant::now() >= d {
                        break;
                    }
                }
                // Build one batch of `batch_size` intents.
                let mut intents: Vec<Intent> = Vec::with_capacity(batch as usize);
                for _ in 0..batch {
                    if sent_local + intents.len() as u64 >= planned {
                        break;
                    }
                    let from: [u8; 20] = rng.gen();
                    let to: [u8; 20] = rng.gen();
                    intents.push(Intent::Transfer { from, to, amount });
                }
                if intents.is_empty() {
                    break;
                }
                let n_in_batch = intents.len() as u64;
                let send_ms = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|d| d.as_millis() as u64)
                    .unwrap_or(0);
                match client.submit_batch(intents).await {
                    Ok(hashes) => {
                        for hash in &hashes {
                            let _ = csv_tx.send((send_ms, *hash, idx));
                        }
                        aggregate_sent.fetch_add(hashes.len() as u64, Ordering::Relaxed);
                        sent_local += n_in_batch;
                    }
                    Err(e) => {
                        eprintln!(
                            "suwappu-loadgen: submit_batch failed on target {} ({}): {:#}",
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
                "suwappu-loadgen: {} TPS aggregate (window of {:.2}s, total {} sent)",
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
            eprintln!("suwappu-loadgen: shutdown signal — aborting targets");
            task_set.shutdown().await;
        }
        _ = drain => {}
    }

    reporter.abort();
    let _ = writer.await;

    eprintln!(
        "suwappu-loadgen: done — {} intents acked across {} targets",
        aggregate_sent.load(Ordering::Relaxed),
        targets.len()
    );
    Ok(())
}
