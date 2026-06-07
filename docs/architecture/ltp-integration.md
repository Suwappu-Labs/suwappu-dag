# LTP integration — Commit / Lattice / Materialize

**Paper §**: 10 — Lattice Transfer Protocol ([`suwappu-papers/papers/dag-l1`](https://github.com/suwappu/suwappu-papers))
**Code**: `crates/suwappu-ltp/src/` (attestation, anchor)
**IQs**: —
**Visuals**: [`docs/visuals/mermaid/ltp.md`](../visuals/mermaid/ltp.md) · [`docs/visuals/ltp.html`](../visuals/ltp.html)
**Sprint**: DAG-S15 (LTP 7-of-9 attestation) ✅ Closed · DAG-S16 (DA SLA) ✅ Closed · DAG-S17 (DID STARK) ✅ Closed

## What it does

LTP carries a payload from a sender to a receiver in three phases:

1. **Commit** — sender encodes the payload + policy into an erasure-coded
   shard set; shards are distributed to the commitment network.
2. **Lattice** — sender constructs a constant-size attestation envelope
   (~1.3 kB) describing the payload, security stack, and policy. Envelope
   is the on-chain LTP commitment; payload bytes never reach the chain.
3. **Materialize** — receiver pulls ≥`k` shards from the commitment
   network, decrypts under the envelope's CEK, reconstructs the payload.

The envelope's constant size (≈1,600 B at chain layer: ML-KEM-768
ciphertext ~1,568 B + BLS aggregate sig ~96 B + SHA3-256 payload root
32 B) is paper Invariant 3.

## Key invariants

- **Constant-size LTP commitment (Invariant 3 in SUWAPPUHELPER.md):** every
  attestation, regardless of payload bytes, commits to the same on-chain
  envelope size.
- **7-of-9 super-node aggregate (S15 exit gate):** see [super-node.md](super-node.md).
- **DA SLA (S16 exit gate):** see `proptest_da.rs`.
- **Cross-chain DID via STARK (S17 exit gate):** see `proptest_did_stark.rs`.

## Cross-references

- **Engineering:** `crates/suwappu-ltp/src/attestation.rs` (envelope construction),
  `crates/suwappu-ltp/src/anchor.rs` (state-anchor handoff to suwappu-db's
  `AnchorDispatcher`).
- **Spec:** Paper §10 — the v8 paper has the full LTP construction.
- **Sister-repo integration:** the LTP runtime + corridor wire-mirror lives
  in [`suwappu-lattice-protocol`](https://github.com/suwappu/suwappu-lattice-protocol);
  bit-for-bit corridor parity (BLS DST + length-prefixed SHA3) is enforced
  by tests in both repos.
- **Visual:** [`docs/visuals/mermaid/ltp.md`](../visuals/mermaid/ltp.md) and
  the presentation [`ltp.html`](../visuals/ltp.html).
