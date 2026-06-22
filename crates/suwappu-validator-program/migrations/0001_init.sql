-- suwappu-validator-program initial schema.
--
-- Authority IDs come from the testnet's `suwappu_getAuthorityRegistry`
-- — seed validators have ids 0..6, faucet is 7, external operators
-- are admitted with ids ≥ 8.

CREATE TABLE IF NOT EXISTS operators (
    authority_id  BIGINT PRIMARY KEY,
    label         TEXT NOT NULL,
    joined_at     TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    is_seed       BOOLEAN NOT NULL DEFAULT FALSE
);

-- Per-probe uptime sample. The accumulator probes every 60s; rows
-- here are the raw probe results. The points-rollup query
-- aggregates per epoch (defined as ceil(sample_at / epoch_length)).
CREATE TABLE IF NOT EXISTS uptime_samples (
    authority_id  BIGINT NOT NULL REFERENCES operators(authority_id) ON DELETE CASCADE,
    sample_at     TIMESTAMPTZ NOT NULL,
    ok            BOOLEAN NOT NULL,
    -- Truncated latency in ms; NULL if probe failed. Used for the
    -- v2 "responsiveness" tier (not in v1 formula).
    latency_ms    INTEGER,
    PRIMARY KEY (authority_id, sample_at)
);

CREATE INDEX IF NOT EXISTS uptime_samples_by_sample_at
    ON uptime_samples (sample_at DESC);

-- Per-epoch certs observed. Populated by the (stubbed) S3 ingest
-- task. Stays empty in v1 — included so the rollup query has
-- something to JOIN against without special-casing the empty
-- table.
CREATE TABLE IF NOT EXISTS certs_observed (
    authority_id  BIGINT NOT NULL REFERENCES operators(authority_id) ON DELETE CASCADE,
    epoch         BIGINT NOT NULL,
    count         BIGINT NOT NULL,
    updated_at    TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (authority_id, epoch)
);

-- Manually-entered bug-bounty + hackathon awards. Foundation
-- admin POSTs these via the program's admin HTTP API.
CREATE TABLE IF NOT EXISTS manual_awards (
    id            BIGSERIAL PRIMARY KEY,
    authority_id  BIGINT NOT NULL REFERENCES operators(authority_id) ON DELETE CASCADE,
    kind          TEXT NOT NULL CHECK (kind IN ('bug_bounty', 'hackathon')),
    -- Points awarded. Per POINTS.md: bug_bounty ∈ {5000, 15000, 50000};
    -- hackathon ∈ [1000, 10000]. Check constraint is loose to allow
    -- foundation discretion within those bands.
    points        BIGINT NOT NULL CHECK (points > 0),
    -- Optional human-readable justification (severity, hackathon name).
    reason        TEXT,
    awarded_at    TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    awarded_by    TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS manual_awards_by_authority
    ON manual_awards (authority_id, awarded_at DESC);

-- Per-epoch rolled-up points. The accumulator's scoring task
-- writes here every epoch. Read by the /leaderboard HTTP API.
-- A separate row per epoch (rather than a running total) preserves
-- the audit trail — POINTS.md mandates per-epoch publication +
-- a 7-day adjustment window.
CREATE TABLE IF NOT EXISTS epoch_points (
    authority_id        BIGINT NOT NULL REFERENCES operators(authority_id) ON DELETE CASCADE,
    epoch               BIGINT NOT NULL,
    uptime_points       BIGINT NOT NULL DEFAULT 0,
    cert_points         BIGINT NOT NULL DEFAULT 0,
    bug_bounty_points   BIGINT NOT NULL DEFAULT 0,
    hackathon_points    BIGINT NOT NULL DEFAULT 0,
    computed_at         TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (authority_id, epoch)
);

CREATE INDEX IF NOT EXISTS epoch_points_by_epoch
    ON epoch_points (epoch DESC);
