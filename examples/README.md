# gsx-dag examples

Minimal end-to-end examples against a running local devnet (see
[`../DEVNET.md`](../DEVNET.md) to bring one up).

| Audience | Read order |
|---|---|
| "How do I read state?" | [Rust query_epoch](rust/query_epoch.rs) → [Rust query_balance](rust/query_balance.rs) → [TS query_epoch](typescript/query_epoch.ts) |
| "How do I watch for commits?" | [Rust subscribe_events](rust/subscribe_events.rs) → [TS subscribe_events](typescript/subscribe_events.ts) |
| "How do I submit a tx?" | [Rust submit_transfer](rust/submit_transfer.rs) (the signing surface is Rust-only today; see the file's header for the ML-DSA-65 caveat) |

## Running the Rust examples

The `examples/rust/` directory is a standalone Cargo project that
depends on `gsx-client`, `gsx-execution`, and `gsx-crypto` via path
references. Bring up the devnet first, then:

```sh
# From the repo root.
cd examples/rust
cargo run --bin query_epoch
cargo run --bin query_balance
cargo run --bin subscribe_events
cargo run --bin submit_transfer       # see file for signing caveat
```

Each example prints its own help when run with `--help`. The
expected stdout for the read-only examples (against a fresh
devnet) is captured in `expected_output.txt` files alongside each
source.

## Running the TypeScript examples

The `examples/typescript/` directory is a standalone npm project
that depends on `@gsx/client` from the local workspace.

```sh
cd examples/typescript
npm install
npm run example:query-epoch
npm run example:query-balance
npm run example:subscribe-events
```

## Why no TS `submit_transfer`?

The intent-submission path requires an ML-DSA-65 signature. ML-DSA-65
doesn't yet have a stable, audited JavaScript implementation —
landing one is a substantial undertaking that belongs in its own
project. Until then, the **Rust** `submit_transfer` example is the
canonical reference; a TS wallet integration will copy its signing
flow once a vetted JS PQC library exists. Tracked in the
roadmap.

## Devnet keys note

The devnet uses placeholder keys derived from
`scripts/gen-devnet-genesis.py`'s seed. Real ML-DSA-65 secret keys
are 4,032 bytes of structured material, not arbitrary bytes — so a
client signing with the placeholder bytes will produce signatures
that the validator **cannot** verify against the seated public key.

`submit_transfer.rs` documents this and provides two execution
modes:

1. **Mock-key mode (default):** generates a fresh keypair at runtime
   so the signing path runs end-to-end. The submission is rejected
   by the devnet's `verify_signed_intent` gate (Unknown signer),
   but you can see the full client → wire → reject path.
2. **Real-key mode:** points the example at a real ML-DSA-65
   secret-key file. To produce one matching the devnet's seated
   set, run `cargo run -p gsx-node --bin gsx-keygen` and **regenerate
   the devnet** with the produced public key (a one-line edit to
   `target/devnet/genesis.toml`'s `mldsa_public_key_hex`).

The `gsx-keygen` helper is a follow-up — it doesn't ship in C2.
Until then, `submit_transfer.rs` runs in mock-key mode by default.

## See also

- [`../DEVNET.md`](../DEVNET.md) — local devnet bring-up
- [`../clients/rust-sdk/README.md`](../clients/rust-sdk/README.md) — Rust SDK reference
- [`../clients/ts-sdk/README.md`](../clients/ts-sdk/README.md) — TS SDK reference
- [`../docs/visuals/README.md`](../docs/visuals/README.md) — protocol-flow diagrams
