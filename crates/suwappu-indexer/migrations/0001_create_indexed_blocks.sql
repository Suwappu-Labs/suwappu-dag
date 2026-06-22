-- suwappu-indexer: initial schema.
--
-- One row per committed block as observed by the indexer. The
-- in-memory `InMemoryStore`'s data model maps 1:1:
--
--   blocks.round         <-> IndexedBlock.round
--   blocks.cert_hash     <-> IndexedBlock.cert_hash (0x-prefixed hex)
--   blocks.indexed_at_ms <-> IndexedBlock.indexed_at_ms
--   blocks.tx_hashes     <-> IndexedBlock.tx_hashes  (TEXT[])
--
-- `tx_index` is materialized as a GIN index on `tx_hashes` so
-- `WHERE tx_hashes @> ARRAY[$1]::TEXT[]` is O(log n).
--
-- Idempotency: `ingest_committed_block` uses INSERT … ON CONFLICT DO
-- NOTHING keyed on `round`, mirroring the in-memory store's
-- "skip if already present" rule.

CREATE TABLE IF NOT EXISTS blocks (
    round            BIGINT      PRIMARY KEY,
    cert_hash        TEXT        NOT NULL,
    indexed_at_ms    BIGINT      NOT NULL,
    tx_hashes        TEXT[]      NOT NULL
);

CREATE INDEX IF NOT EXISTS blocks_tx_hashes_gin
    ON blocks USING GIN (tx_hashes);

CREATE INDEX IF NOT EXISTS blocks_indexed_at_ms_idx
    ON blocks (indexed_at_ms DESC);
