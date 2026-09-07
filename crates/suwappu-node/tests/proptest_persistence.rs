//! DAG-S34.3 exit gate (IQ-008 D4, invariant candidate I-P1): replay
//! equivalence at the storage layer.
//!
//! For any committed sequence of blocks, applying them live and replaying
//! them from the durable commit log — from genesis, or from a snapshot
//! taken at an arbitrary point plus the records after it — yields the
//! same substrate state root. A torn tail (crash mid-write) yields the
//! root of the longest intact prefix, never a root the live node did not
//! pass through.
//!
//! Run at default 256 cases under CI; sprint close runs
//! `PROPTEST_CASES=10000 cargo test -p suwappu-node --release --test proptest_persistence`.

use std::{collections::BTreeMap, fs, path::PathBuf};

use proptest::prelude::*;
use suwappu_consensus::Certificate;
use suwappu_execution::{execute_block, Block, InMemorySubstrate, Intent, Substrate};
use suwappu_node::{
    store::{
        load_latest_snapshot, write_snapshot, CommitLog, LogEvent, StateSnapshot, STORE_VERSION,
    },
    BlockPayload,
};

fn tmp_dir(tag: &str, seed: u64) -> PathBuf {
    let d = std::env::temp_dir().join(format!(
        "suwappu-persist-{tag}-{}-{seed}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&d);
    d
}

fn addr(i: u8) -> [u8; 20] {
    [i; 20]
}

/// A block of `k` transfers among 4 funded addresses, values derived from
/// `seed` so the sequence is reproducible under shrinking.
fn block_at(round: u64, seed: u64, k: usize) -> (Certificate, BlockPayload) {
    let mut intents = Vec::with_capacity(k);
    for i in 0..k {
        let x = seed
            .wrapping_mul(0x9E37_79B9_7F4A_7C15)
            .wrapping_add(round * 131 + i as u64 * 17);
        let from = addr(1 + (x % 4) as u8);
        let to = addr(1 + ((x >> 8) % 4) as u8);
        let amount = (x >> 16) % 50;
        intents.push(Intent::Transfer {
            from,
            to,
            amount: amount as u128,
        });
    }
    let cert = Certificate {
        author: (round % 4) as u32,
        round,
        parents: Vec::new(),
        payload_digest: {
            let bytes = suwappu_node::codec::encode(&intents).unwrap();
            *blake3::hash(&bytes).as_bytes()
        },
        signature: Vec::new(),
    };
    let block = BlockPayload {
        payload_digest: cert.payload_digest,
        author: cert.author,
        round,
        cert_hash: cert.hash(),
        intents,
        governance_auth: Vec::new(),
    };
    (cert, block)
}

fn genesis_substrate() -> InMemorySubstrate {
    InMemorySubstrate::from_balances((1..=4u8).map(|i| (addr(i), 10_000u128)))
}

fn apply(sub: &mut InMemorySubstrate, round: u64, intents: &[Intent]) {
    let _ = execute_block(
        sub,
        &Block {
            round,
            intents: intents.to_vec(),
        },
    );
}

fn snapshot_of(sub: &InMemorySubstrate, leader_round: u64, log_sequence: u64) -> StateSnapshot {
    StateSnapshot {
        version: STORE_VERSION,
        network_id: "persist".into(),
        leader_round,
        has_committed: true,
        gc_round: None,
        log_sequence,
        last_authored_round: Some(leader_round),
        state_root: sub.state_root(),
        substrate: sub.clone(),
        authority_registry: Default::default(),
        validator_registry: Default::default(),
        stake_table: Default::default(),
        epoch: (0, 1024, 0),
        pending_governance: Vec::new(),
        pending_stake: BTreeMap::new(),
        n_authorities: 4,
        dag_certs: Vec::new(),
        tombstones: Vec::new(),
        committed: Vec::new(),
        blocks: Vec::new(),
        checkpoint: None,
        checkpoint_chain: Vec::new(),
        checkpoint_cursor: (0, 0, [0u8; 32]),
    }
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: std::env::var("PROPTEST_CASES").ok().and_then(|v| v.parse().ok()).unwrap_or(256),
        max_shrink_iters: 16,
        .. ProptestConfig::default()
    })]

    /// EXIT GATE (I-P1): live == replay-from-genesis == snapshot + replay.
    #[test]
    fn replay_equivalence(
        seed in any::<u64>(),
        n_blocks in 1usize..=24,
        per_block in 0usize..=6,
        cut_frac in 0u64..=100,
    ) {
        let dir = tmp_dir("eq", seed);
        let mut live = genesis_substrate();
        let mut roots_after: Vec<[u8; 32]> = Vec::new();
        {
            let (mut log, existing) = CommitLog::open(&dir, false).unwrap();
            prop_assert!(existing.is_empty());
            for r in 1..=n_blocks as u64 {
                let (cert, block) = block_at(r, seed, per_block);
                // Write-ahead, then apply — the daemon's order.
                log.append_committed(r, cert, block.clone()).unwrap();
                apply(&mut live, r, &block.intents);
                roots_after.push(live.state_root());
            }
        }

        // Replay from genesis.
        let (log, records) = CommitLog::open(&dir, false).unwrap();
        prop_assert_eq!(log.next_sequence() as usize, n_blocks);
        let mut replayed = genesis_substrate();
        for rec in &records {
            if let LogEvent::Committed { block, .. } = &rec.event {
                apply(&mut replayed, block.round, &block.intents);
            }
        }
        prop_assert_eq!(replayed.state_root(), live.state_root(), "replay from genesis diverged");
        prop_assert_eq!(&replayed, &live);

        // Snapshot at an arbitrary cut, then replay the tail.
        let cut = (n_blocks as u64 * cut_frac / 100) as usize; // 0..=n_blocks
        let mut at_cut = genesis_substrate();
        for rec in records.iter().take(cut) {
            if let LogEvent::Committed { block, .. } = &rec.event {
                apply(&mut at_cut, block.round, &block.intents);
            }
        }
        if cut > 0 {
            prop_assert_eq!(at_cut.state_root(), roots_after[cut - 1]);
        }
        write_snapshot(&dir, &snapshot_of(&at_cut, cut as u64, cut as u64)).unwrap();
        let snap = load_latest_snapshot(&dir, "persist").unwrap().unwrap();
        prop_assert_eq!(snap.log_sequence as usize, cut);
        let mut resumed = snap.substrate;
        for rec in &records {
            if let LogEvent::Committed { sequence, block, .. } = &rec.event {
                if (*sequence as usize) < cut {
                    continue;
                }
                apply(&mut resumed, block.round, &block.intents);
            }
        }
        prop_assert_eq!(resumed.state_root(), live.state_root(), "snapshot + tail replay diverged");
        let _ = fs::remove_dir_all(&dir);
    }

    /// A torn tail recovers to the longest intact prefix, and that prefix's
    /// replay root is one the live node actually passed through.
    #[test]
    fn torn_tail_recovers_a_live_prefix(
        seed in any::<u64>(),
        n_blocks in 2usize..=16,
        per_block in 1usize..=4,
        chop_frac in 1u64..=99,
    ) {
        let dir = tmp_dir("torn", seed);
        let mut roots_after: Vec<[u8; 32]> = vec![genesis_substrate().state_root()];
        let mut live = genesis_substrate();
        {
            let (mut log, _) = CommitLog::open(&dir, false).unwrap();
            for r in 1..=n_blocks as u64 {
                let (cert, block) = block_at(r, seed, per_block);
                log.append_committed(r, cert, block.clone()).unwrap();
                apply(&mut live, r, &block.intents);
                roots_after.push(live.state_root());
            }
        }
        let path = dir.join("commit.log");
        let full = fs::metadata(&path).unwrap().len();
        let keep = full * chop_frac / 100;
        fs::OpenOptions::new().write(true).open(&path).unwrap().set_len(keep).unwrap();

        let (log, records) = CommitLog::open(&dir, false).unwrap();
        let intact = log.next_sequence() as usize;
        prop_assert_eq!(records.len(), intact);
        prop_assert!(intact < n_blocks, "chopping inside the file must lose at least the last record");
        // Reopening after truncation is stable: no further loss.
        let (log2, records2) = CommitLog::open(&dir, false).unwrap();
        prop_assert_eq!(log2.next_sequence() as usize, intact);
        prop_assert_eq!(records2.len(), intact);

        let mut replayed = genesis_substrate();
        for rec in &records {
            if let LogEvent::Committed { block, .. } = &rec.event {
                apply(&mut replayed, block.round, &block.intents);
            }
        }
        prop_assert_eq!(replayed.state_root(), roots_after[intact]);
        let _ = fs::remove_dir_all(&dir);
    }
}
