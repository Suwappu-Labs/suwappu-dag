//! Postgres-backed `Store` implementation (T7).
//!
//! Connects to the URL passed via `--database-url` /
//! `GSX_INDEXER_DATABASE_URL` and applies the `migrations/` directory
//! on startup. Schema lives in
//! [`crates/gsx-indexer/migrations/0001_create_indexed_blocks.sql`].
//!
//! Compiled only when the `postgres` feature is enabled. The in-memory
//! `InMemoryStore` remains available unconditionally so unit tests +
//! offline use don't need a live Postgres.
//!
//! ## Concurrency
//!
//! `PostgresStore` is `Clone` (the `PgPool` is internally
//! `Arc<PoolInner>`), so multiple ingester / API tasks can share one
//! instance without locking on the Rust side. Postgres handles MVCC.
//!
//! ## Idempotency
//!
//! `ingest_committed_block` uses `INSERT … ON CONFLICT (round) DO
//! NOTHING`. The in-memory store has the same semantics
//! (`if contains_key { return }`); both pass the
//! `ingest_is_idempotent` test in `store.rs`.
//!
//! ## Crash recovery
//!
//! Restarts resume from "live" — the WebSocket subscriber re-subscribes
//! and the next commit observed gets inserted. Commits that landed on
//! the chain between the last persisted row and the restart are MISSED
//! by the live tail; backfill via `gsx_getBlock` is a follow-up (T7
//! Phase 2). On startup we log the persisted `latest_round` so the
//! operator can compare against the chain's current head.

use std::time::Duration;

use sqlx::{
    postgres::{PgPool, PgPoolOptions},
    Row,
};

use crate::store::{IndexedBlock, Store};

/// Postgres-backed `Store`.
#[derive(Clone)]
pub struct PostgresStore {
    pool: PgPool,
}

impl PostgresStore {
    /// Connect to `database_url`, apply migrations, and return a
    /// ready-to-use store. Run-once on indexer startup.
    pub async fn connect(database_url: &str) -> anyhow::Result<Self> {
        let pool = PgPoolOptions::new()
            .max_connections(8)
            .acquire_timeout(Duration::from_secs(10))
            .connect(database_url)
            .await?;

        // Apply migrations from the embedded directory. `migrate!()`
        // bakes the migration files into the binary at compile time
        // so the deployed binary doesn't need access to the source
        // tree.
        sqlx::migrate!("./migrations").run(&pool).await?;

        Ok(Self { pool })
    }
}

impl Store for PostgresStore {
    async fn ingest_committed_block(&self, block: IndexedBlock) {
        // Idempotent insert. Mirror the in-memory store's "first
        // ingest wins" semantics — `ON CONFLICT DO NOTHING` skips
        // duplicate rounds without surfacing an error.
        let result = sqlx::query(
            "INSERT INTO blocks (round, cert_hash, indexed_at_ms, tx_hashes) \
             VALUES ($1, $2, $3, $4) \
             ON CONFLICT (round) DO NOTHING",
        )
        .bind(block.round as i64)
        .bind(&block.cert_hash)
        .bind(block.indexed_at_ms as i64)
        .bind(&block.tx_hashes)
        .execute(&self.pool)
        .await;
        if let Err(e) = result {
            // Don't crash the subscriber on a transient Postgres error;
            // log + drop. The next commit's INSERT will reconnect.
            tracing::warn!(
                round = block.round,
                error = %e,
                "postgres: ingest_committed_block failed",
            );
        }
    }

    async fn latest_round(&self) -> Option<u64> {
        let row = sqlx::query("SELECT MAX(round) AS max_round FROM blocks")
            .fetch_one(&self.pool)
            .await
            .ok()?;
        let max: Option<i64> = row.try_get("max_round").ok()?;
        max.map(|v| v as u64)
    }

    async fn get_block(&self, round: u64) -> Option<IndexedBlock> {
        let row = sqlx::query(
            "SELECT round, cert_hash, indexed_at_ms, tx_hashes \
             FROM blocks WHERE round = $1",
        )
        .bind(round as i64)
        .fetch_optional(&self.pool)
        .await
        .ok()
        .flatten()?;
        Some(row_to_block(&row))
    }

    async fn get_blocks(&self, from: u64, to: u64) -> Vec<IndexedBlock> {
        let rows = match sqlx::query(
            "SELECT round, cert_hash, indexed_at_ms, tx_hashes \
             FROM blocks WHERE round BETWEEN $1 AND $2 \
             ORDER BY round ASC LIMIT 1024",
        )
        .bind(from as i64)
        .bind(to as i64)
        .fetch_all(&self.pool)
        .await
        {
            Ok(rs) => rs,
            Err(e) => {
                tracing::warn!(from, to, error = %e, "postgres: get_blocks failed");
                return Vec::new();
            }
        };
        rows.iter().map(row_to_block).collect()
    }
}

fn row_to_block(row: &sqlx::postgres::PgRow) -> IndexedBlock {
    let round: i64 = row.try_get("round").unwrap_or(0);
    let cert_hash: String = row.try_get("cert_hash").unwrap_or_default();
    let indexed_at_ms: i64 = row.try_get("indexed_at_ms").unwrap_or(0);
    let tx_hashes: Vec<String> = row.try_get("tx_hashes").unwrap_or_default();
    IndexedBlock {
        round: round as u64,
        cert_hash,
        indexed_at_ms: indexed_at_ms as u64,
        tx_hashes,
    }
}
