# `gsx-validator-program`

Testnet points-accumulator daemon. Implements the public contract
in [`docs/testnet/POINTS.md`](../../docs/testnet/POINTS.md).

## What it does

- **Probe** (every 60 s): hits `gsx_getEpoch` against the public
  testnet RPC and records uptime samples per registered
  operator.
- **Score** (every 5 min): rolls up the previous hour's uptime
  samples into `epoch_points`. Bucket size is 1 hour to match
  the operator-side `events.ndjson` rotation cadence.
- **Serve**: HTTP API
  - `GET  /leaderboard` — public; sorted by total points.
  - `GET  /health` — public; always 200.
  - `POST /admin/operators` — bearer-token auth; register or
    update an operator (called after a governance admit).
  - `GET  /admin/operators` — bearer-token auth; list operators.
  - `POST /admin/award` — bearer-token auth; credit a
    bug-bounty or hackathon award.
  - `GET  /admin/awards/:authority_id` — bearer-token auth;
    list manual awards for an operator (audit trail).

## v1 scope limits

- **Cert observation is STUBBED**: the `certs_observed` table is
  declared but unused — no external operators uploading
  `events.ndjson` yet. The scoring task JOINs against it
  (always producing zero cert points) so adding S3 NDJSON
  ingest in v2 is a single workstream addition.
- **Single foundation instance**: the daemon is one host. Per
  POINTS.md, decentralized scoring (multi-party MPC across
  seed validators) is a v2 consideration.
- **All operators share one uptime signal**: v1 probes the
  ALB-fronted RPC and credits every registered operator the
  same sample. Per-operator probing requires DNS + IAM-scoped
  endpoints — out of scope until external operators are
  actually online.

## Local development

```sh
# 1. Spin up a local Postgres (any 16+ release).
docker run -d --name gsx-program-db -p 5432:5432 \
    -e POSTGRES_PASSWORD=dev -e POSTGRES_DB=validator_program postgres:16

# 2. Build + run.
cargo run --release -p gsx-validator-program -- \
    --database-url postgres://postgres:dev@127.0.0.1:5432/validator_program \
    --rpc-url       https://rpc.testnet.gsx.globalsettlement.com \
    --bind          127.0.0.1:8090 \
    --admin-token   localdev-bearer-token

# 3. Register an operator (foundation-admin only).
curl -X POST -H 'Authorization: Bearer localdev-bearer-token' \
     -H 'Content-Type: application/json' \
     -d '{"authority_id":0,"label":"us-east-1","is_seed":true}' \
     http://127.0.0.1:8090/admin/operators

# 4. Read the leaderboard (public).
curl http://127.0.0.1:8090/leaderboard | jq .
```

## Production deployment

The daemon runs on the EC2 instance provisioned by
[`terraform/testnet/validator-program.tf`](../../terraform/testnet/validator-program.tf).
See [`OPERATIONS.md` § 10.3](../../OPERATIONS.md) for the SSM
deployment procedure.

Required environment (set in the systemd unit):

| Variable | Purpose |
|---|---|
| `GSX_PROGRAM_DATABASE_URL` | Postgres connection string. Read from AWS Secrets Manager at boot. |
| `GSX_PROGRAM_RPC_URL` | Public testnet RPC (default: `https://rpc.testnet.gsx.globalsettlement.com`). |
| `GSX_PROGRAM_BIND` | TCP bind addr (default `0.0.0.0:8090`). |
| `GSX_PROGRAM_ADMIN_TOKEN` | Bearer token gating `/admin/*`. Rotate via Secrets Manager + service restart. |

## Database schema

See [`migrations/0001_init.sql`](migrations/0001_init.sql).
`init_db()` runs the migrations idempotently on every startup
via `sqlx::migrate!`.

## Tests

```sh
cargo test -p gsx-validator-program --lib
```

4 unit tests cover the uptime-tier formula. Integration tests
(against a real Postgres) live in `tests/` and are gated on the
`integration` feature so the default test target stays hermetic.

## See also

- [`docs/testnet/POINTS.md`](../../docs/testnet/POINTS.md) —
  authoritative formula contract.
- [`docs/testnet/VALIDATOR-OPERATORS.md`](../../docs/testnet/VALIDATOR-OPERATORS.md)
  — operator-side onboarding.
- [`terraform/testnet/validator-program.tf`](../../terraform/testnet/validator-program.tf)
  — EC2 + RDS where this binary deploys.
- [`OPERATIONS.md`](../../OPERATIONS.md) § 10.3 — deployment
  procedure.
