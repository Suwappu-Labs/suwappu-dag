# IQ-005 — bincode 2.x migration + wire-frame version byte

**Status:** Ratified 2026-05-16 — shipped via PR #70 (F4 in the
code-only follow-up program).
**Owner:** transport / node
**Date:** 2026-05-16
**Tracking:** F4 line item in the post-C4 code-only follow-up plan.
**Note (2026-05-16 revision):** the F4 commit message originally
claimed this closed `RUSTSEC-2025-0141`. That was wrong — the
advisory covers the entire bincode crate (1.x AND 2.x; the project
was discontinued, not just the 1.x branch), so the ignore stays in
`deny.toml` until we migrate to one of the suggested alternatives
(postcard, rkyv, bitcode). See `deny.toml` for the updated gate.

## Question

`bincode 1.x` is unmaintained upstream. `bincode 2.x` ships a new
serde-feature-gated API (`bincode::serde::encode_to_vec` /
`decode_from_slice`) and a configurable encoding pipeline; the
default `config::standard()` in 2.x uses varint encoding, which is
byte-incompatible with 1.x's default. Switching the workspace dep
without preserving byte layout would (a) change every existing
`blake3(bincode(intent))` content hash and (b) silently fail to
decode peer frames sent by pre-flip builds.

Should we migrate now, and if so how do we preserve the
byte-stability the codebase depends on?

## Decision

Migrate workspace-wide in one PR, pinning
`bincode::config::legacy()` at every call site to keep byte layout
identical to 1.x's default. Additionally, prepend a 1-byte version
marker (`FRAME_VERSION_V1 = 0x01`) to every wire-going frame so
future codec flips are detectable instead of silently corrupting
peers.

Per-PR scope:

1. **`Cargo.toml` + `fuzz/Cargo.toml`**: bump to `bincode = "2"`
   with `features = ["serde", "std"]`,
   `default-features = false`.
2. **New `crates/gsx-node/src/codec.rs`**: thin shim with
   `encode` / `decode` / `encode_frame` / `decode_frame` helpers
   over `bincode::config::legacy()`. Centralizes the codec choice
   so a future cutover is a single change, not a 40-site grep.
3. **All bincode call sites** (~40 sites across 9 modules)
   migrated to the codec helpers. Wire-going paths (`wire::*`,
   `client::write_response` / `LoadGenClient::submit{,_batch}`,
   the daemon test's `round_trip`) use `encode_frame` /
   `decode_frame`. Inner uses (intent → blake3 hash, internal
   serialization for testing) use the plain `encode` / `decode`
   so the canonical hash recipe stays stable independent of
   future frame-version bumps.
4. **`WireError`**: split `Codec(bincode::Error)` into
   `Encode(codec::EncodeError)` and `Decode(codec::FrameDecodeError)`;
   the latter wraps `FrameDecodeError::UnknownVersion(u8)` so a
   pre-flip peer (or a malicious one) is surfaced cleanly.
5. **`deny.toml`**: ~~remove the `RUSTSEC-2025-0141` ignore~~ —
   reverted on 2026-05-16; the advisory covers all bincode versions
   including 2.x. The ignore's rationale is updated to reflect the
   actual project-wide unmaintained status.

## Constraints honored

- **Hash stability.** Every site that computes
  `blake3(bincode(intent))` (mempool dedup, intent signing digest,
  block tx-hash recomputation in `rpc_adapter::block_at_round`,
  consensus tx-hash list) uses the same `legacy()` config. Pre- and
  post-flip hashes are byte-identical. No on-chain content shifted.
- **Wire byte-identity.** `legacy()` matches 1.x's default
  (fixint, little-endian, no limit, allow trailing bytes). The
  on-wire bincode payload bytes are unchanged; the only new byte
  is the leading `0x01` version marker prepended by `encode_frame`.
  Pre-F4 peers cannot interop with F4 peers (their first frame byte
  is the start of the bincode payload, which F4 reads as a version
  marker → `FrameDecodeError::UnknownVersion`). This is a **hard
  fork** relative to anything running pre-flip, but no public
  network is live today, so the cutover is free.
- **No `--workspace` cargo commands on this Mac** per `CLAUDE.md`.
  Per-crate `cargo check` + `cargo clippy --all-targets -- -D
  warnings` validated F4-touched crates locally; CI matrix
  validates the rest once billing is restored.

## Future cutovers

A v2 frame is a single match-arm addition in `codec::decode_frame`:

```rust
match version {
    FRAME_VERSION_V1 => decode::<T>(body),
    FRAME_VERSION_V2 => decode_v2::<T>(body),
    other => Err(FrameDecodeError::UnknownVersion(other)),
}
```

Operators get a transition window where both versions decode; once
all peers have upgraded, the v1 arm is removed.

## What this PR does NOT change

- The 4-byte big-endian length prefix wrapping the frame (lives in
  `wire::write_frame` / `read_frame`; the version byte sits
  INSIDE this envelope).
- `MAX_FRAME_BYTES` (1 MiB) and `MAX_COMPACT_MESSAGE_BYTES`
  (64 KiB) caps. The compact-cap check uses the framed byte count;
  a 1-byte version marker is within rounding error of either cap.
- Postgres / SQL schemas. Indexer rows are unchanged.

## See also

- [`crates/gsx-node/src/codec.rs`](../../crates/gsx-node/src/codec.rs)
- [`crates/gsx-node/tests/proptest_wire_decode.rs`](../../crates/gsx-node/tests/proptest_wire_decode.rs) —
  panic-freedom contract extended to the framed surface.
- [bincode 2.x migration guide](https://github.com/bincode-org/bincode/blob/trunk/docs/migration_guide.md)
