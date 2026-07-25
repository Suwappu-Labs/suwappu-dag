# PQ-primitive hedge: what if ML-DSA-65 itself needs replacing?

Companion to [Cryptographic posture](cryptographic-posture.md). That doc
covers migrating *away from* classical primitives (ECDSA, BLS12-381,
Groth16) toward ML-DSA-65/ML-KEM-768. This doc covers the complementary,
harder question it doesn't answer: **what happens if ML-DSA-65 itself
— our only signature scheme — needs replacing?**

This is a design doc, not a commitment or a shipped feature. Nothing
here is implemented. The goal is to have a real answer ready before a
hedge is needed, following the multi-algorithm precedent set by
Cellframe (which ships several PQ signature options rather than
standardizing on one), rather than improvising during an incident.

## Why this is a real question, not paranoia

ML-DSA-65 (FIPS 204) is a lattice-based scheme (Module-LWE / Module-SIS).
NIST's own PQC program did not standardize a single signature family —
it standardized ML-DSA (lattice-based) *and* SLH-DSA (FIPS 205,
hash-based) side by side, specifically because lattice hardness
assumptions are younger and less battle-tested than, say, RSA or ECC
were at standardization, and a structurally *different* fallback
assumption is valuable insurance. Falcon (also NIST-selected) is
**also** lattice-based (NTRU lattices) — so despite being a second
algorithm, it does **not** hedge against a lattice-cryptanalysis
break; a real hedge needs a scheme whose security does not reduce to
the same class of lattice problem.

We are currently **100% concentrated on one lattice assumption**,
chain-wide, with zero fallback. That is a reasonable bet — ML-DSA-65 is
the most-vetted PQ signature standard that exists — but it is a bet,
and this doc exists so we know what unwinding it would look like if
the bet ever needs to change.

## Current state (verified against source, 2026-07-25)

- `suwappu-crypto::mldsa` (`crates/suwappu-crypto/src/mldsa.rs`) wraps
  `pqcrypto_mldsa::mldsa65` directly. `PublicKey`/`SecretKey`/`Signature`
  are bare `Vec<u8>` newtypes — no scheme tag, no length-prefix, no
  discriminant byte anywhere in this module.
- The consensus `Certificate` (`crates/suwappu-consensus/src/cert.rs`)
  signs/verifies with `suwappu_crypto::mldsa::{sign,verify}` directly;
  its doc comment states outright "every certificate carries a
  detached ML-DSA-65 signature." The signature field is raw bytes.
- The genesis manifest (`GenesisValidator.mldsa_public_key_hex` in
  `crates/suwappu-node/src/config.rs`) encodes the algorithm in the
  *field name*, not a data tag. Genesis has no concept of "this
  validator's key is scheme X."
- RPC-facing types (`suwappu-rpc/src/context.rs`,
  `rpc_adapter.rs`) carry `mldsa_public_key_hex` the same way.
- **No algorithm-agility mechanism exists anywhere in this surface.**
  `grep -rn "SchemeId\|scheme_id\|AlgorithmId\|enum AuthScheme"` across
  `suwappu-dag` returns zero hits on the consensus/genesis/RPC path.

There **is** a real, working precedent for exactly this kind of
agility elsewhere in this workspace — it's just scoped to a different
surface. `crates/suwappudb-bridge/src/anchor/types.rs`'s `AuthScheme`
(`#[repr(u8)]` discriminant: `Blake3Mac=0, Sp1ZkProof=1,
EcdsaSecp256k1=2, MlDsa65Hybrid=3`, documented in
`docs/visuals/mermaid/auth-dispatch.md`) already does scheme-tagged,
hybrid-composed (AND-gate) credential verification for anchor
authentication. That pattern — a discriminant byte in front of the
credential, per-variant verification, an explicit `Hybrid` composition
that requires *both* halves to pass — is the template a Certificate/
genesis-level hedge should follow. It does not exist on the
consensus-signing path today; it would need to be built there
separately.

## What other PQ signature schemes are already "in the building"

- `pqcrypto-mlkem` (ML-KEM-768) is already a dependency
  (`crates/suwappu-crypto/src/mlkem.rs`, used for LTP sealed session
  keys) — but it's a **KEM**, not a signature scheme. It cannot hedge
  ML-DSA's signing role no matter how deeply integrated it is.
- No SLH-DSA/SPHINCS+/Falcon crate exists anywhere in `Cargo.lock`
  today. Falcon and SPHINCS+ appear only as prose mentions in
  competitor-landscape audit notes (`docs/audit/mainnet-readiness-*.md`,
  discussing Algorand's Falcon-1024 choice), not as candidates
  evaluated for this codebase.
- BLS12-381 (LTP aggregate signatures) is already flagged as a
  **classical**-crypto exception in `cryptographic-posture.md`, with
  its own migration target — orthogonal to this doc, since BLS isn't
  a PQ hedge candidate, it's the thing being migrated *away from*.

## Candidate hedge algorithm: SLH-DSA (FIPS 205), not Falcon

If a hedge is ever needed, **SLH-DSA (the standardized form of
SPHINCS+)** is the structurally-sound choice, not Falcon:

| | ML-DSA-65 | Falcon-1024 | SLH-DSA (SPHINCS+) |
|---|---|---|---|
| Hardness basis | Module-LWE/SIS (lattice) | NTRU (lattice) | Hash function security only |
| Hedges a lattice break? | — | **No** — same assumption family | **Yes** — no algebraic structure to attack |
| Signature size | ~3.3 KB | ~1.3 KB | ~8–50 KB (parameter-dependent) |
| Sign/verify cost | Fast | Fast (verify), slower (sign, floating-point) | Slow sign, fast verify |
| Standard | FIPS 204 | Not yet a NIST FIPS (round 4 alt) | FIPS 205 |

SLH-DSA's cost (large signatures, slow signing) is exactly the price
of a genuinely independent security assumption — that trade-off is the
point of a hedge, not a flaw to optimize away. A `pqcrypto-sphincsplus`
or equivalent crate would need to be added; none exists in this
workspace's dependency tree today.

## What a real hedge would require (honest scope, not a flag-flip)

A prior audit of this codebase found **42 files across 13 crates**
reference ML-DSA directly (`suwappu-crypto`, `suwappu-consensus`,
`suwappu-node`, `suwappu-execution`, `suwappu-authority`, `suwappu-rpc`,
`suwappu-transport`, `suwappu-ltp`, `suwappu-l2-confidential`,
`suwappu-mldsa-precompile`, `suwappu-precompiles`, `suwappu-faucet`,
`clients/rust-sdk`), including a dedicated `suwappu-mldsa-precompile`
crate and DID-resolver precompiles that hardcode the scheme name. This
is not a configuration change; it's simultaneous work across consensus,
genesis, RPC, precompiles, and the SDK. Concretely, in rough dependency
order:

1. **Wire-format versioning first.** Add a scheme-id byte in front of
   every signature/public-key blob that doesn't have one today
   (`Certificate.signature`, genesis manifest keys, RPC key fields,
   client intent signatures) — mirroring `AuthScheme`'s discriminant
   pattern. This alone is a breaking wire-format change requiring a
   coordinated network upgrade (comparable in kind, if not in size, to
   the `Certificate` signing work landed for DAG-S6).
2. **Dual-key genesis.** Genesis needs to carry *two* public keys per
   validator (ML-DSA-65 + hedge scheme) during any transition window,
   not a single algorithm-typed key.
3. **Hybrid verification, not a hard cutover.** Following the
   `AuthScheme::MlDsa65Hybrid` AND-gate precedent: a transition period
   should require *both* signatures to verify, not switch atomically —
   an atomic cutover reintroduces exactly the single-point-of-failure
   risk a hedge exists to remove, just on the new algorithm instead of
   the old one.
4. **Precompile + SDK updates.** `suwappu-mldsa-precompile` and DID
   resolution logic hardcode the scheme; both need scheme-aware
   dispatch. Client SDKs need to support signing/verifying under
   either scheme during the transition.
5. **Eventual retirement.** Once confidence in the hedge scheme (or a
   fix to the original) is established, drop the AND-gate back to a
   single scheme — this is the exit condition, mirroring
   `cryptographic-posture.md`'s "Migration sequencing" pattern.

Rough estimate, by analogy to the DAG-S6 certificate-signing effort
(one crate, ~10 files, ~1 week) scaled to this doc's 13-crate,
42-file blast radius plus a genesis/network-upgrade coordination cost
on top: **not a "1-2 day" or "1 sprint" change** — closer to a
multi-week, multi-crate initiative requiring a coordinated validator-set
upgrade, if and when it's triggered. This doc's own effort (research +
writing) is the 1-2 day deliverable; the migration it describes is not.

## Trigger conditions (when would this actually get built)

Not every incremental cryptanalysis paper warrants activating this
plan. Reasonable triggers, in ascending order of urgency:

1. **Advisory tier** — a credible cryptanalytic advance against
   Module-LWE/SIS that meaningfully reduces ML-DSA's claimed security
   margin, without yet breaking it. Response: begin adding the
   scheme-id wire format (step 1 above) as low-priority background
   work, so the option exists.
2. **Deprecation tier** — NIST or another standards body formally
   flags ML-DSA for deprecation, or a major PQC implementer
   (e.g. another L1, a major TLS stack) begins visibly hedging.
   Response: execute steps 1-3, begin dual-signing on new validator
   onboarding.
3. **Break tier** — a practical forgery attack against ML-DSA-65 is
   published. Response: emergency hybrid cutover (all steps), treated
   as an incident, not a planned migration — the AND-gate design in
   step 3 exists precisely so this tier doesn't require an atomic,
   network-halting switch.

## What this doc deliberately does not do

- It does not pick a hedge scheme's exact parameter set, add a
  dependency, or write any code.
- It does not commit to a timeline — there is no evidence today that
  any trigger condition above has occurred.
- It does not cover BLS12-381 or ECDSA — those are covered by
  [Cryptographic posture](cryptographic-posture.md)'s existing
  migration-away-from-classical-crypto plan.

If a trigger condition above is met, treat this doc as the starting
point for a real design, not the design itself — it will need
revisiting against whatever the actual state of the art is at that
time.
