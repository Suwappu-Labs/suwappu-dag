# Security policy

## Reporting a vulnerability

Please **do not** open a public GitHub issue for security
vulnerabilities. We need a window to triage and patch before
disclosure.

### Preferred channel

Email **`security@suwappu.bot`** with:

- A description of the issue.
- The affected component(s) — crate path, RPC method, wire variant,
  or operator workflow.
- Reproduction steps (the more concrete, the faster we can move).
- Your assessment of impact (denial-of-service vs. safety violation
  vs. theft of value).
- Whether you'd like to be credited in the post-fix disclosure
  notice.

If you need PGP, our key fingerprint will be published at this URL
once the security team finalizes the key ceremony:
`https://suwappu.bot/.well-known/security.txt` (TODO —
tracked alongside C3). Until then, plain email is acceptable for
initial contact; we'll set up an encrypted channel before exchanging
any reproduction material.

### Response targets

| Severity | First response | Patched mainline |
|---|---|---|
| **Critical** (safety violation, theft of value, key compromise) | 24 hours | 7 days |
| **High** (DoS that halts commits, slashing-bypass) | 72 hours | 14 days |
| **Medium** (DoS that throttles but doesn't halt) | 1 week | 30 days |
| **Low** (info disclosure that doesn't help an attacker) | 2 weeks | next minor release |

These are targets, not commitments — see the
[mainnet-readiness audit](docs/audit/mainnet-readiness-2026-05-15.md)
for the team's current operational posture. Pre-mainnet, the security
surface area is intentionally small; expect a thoughtful response
even if a same-day fix isn't possible.

## What's in scope

Anything in this repo's `crates/`, `clients/`, `scripts/`, or
`fuzz/` is in scope. Specifically:

- The Mysticeti-C consensus implementation
  (`crates/suwappu-consensus/`, `crates/suwappu-node/src/daemon.rs`).
- The fast-path equivocation slashing surface
  (`crates/suwappu-fastpath/`, especially `binding.rs` + `equivocation.rs`).
- The ML-DSA-65 signature gate on client + JSON-RPC ingress
  (`crates/suwappu-node/src/client.rs::verify_signed_intent`,
  `crates/suwappu-rpc/src/methods.rs::submit_intent`).
- The wire decode path (`crates/suwappu-node/src/wire.rs` + the
  cargo-fuzz targets in `fuzz/`).
- The cross-repo cryptographic primitives in `suwappu-crypto`
  (ML-DSA-65, ML-KEM-768, BLS12-381, SHA3-256 — see
  [`docs/architecture/cryptographic-posture.md`](docs/architecture/cryptographic-posture.md)).
- The LTP attestation surface in `suwappu-ltp`.

The state-substrate code (`suwappu-db`) lives in
[its own repo](https://github.com/Suwappu-Labs/suwappu-db);
report substrate issues there.

## What's out of scope (for this repo)

- **Operational issues** unrelated to the codebase: lost validator
  keys, mis-configured genesis manifests, peering / NAT problems.
  Open a regular GitHub issue or contact the operator team.
- **Third-party deps:** RUSTSEC advisories already tracked in
  [`deny.toml`](deny.toml) (and re-checked quarterly). If you find a
  new advisory affecting our dep tree, please report it via the
  same `security@` email so we can prioritize the bump.
- **Production cluster configurations:** the public perf testnet's
  AWS infrastructure is managed by the operator team. Report cluster
  issues via the channels in
  [`docs/audit/mainnet-readiness-2026-05-15.md`](docs/audit/mainnet-readiness-2026-05-15.md).

## Disclosure policy

We follow a coordinated disclosure model:

1. You report → we acknowledge within the response window above.
2. We triage + patch on a private branch.
3. We coordinate a release window with you (typically 30 days max
   pre-disclosure for the patch to bake on testnets).
4. We publish a fix + an advisory crediting you (unless you opt out).

If a critical issue is being actively exploited in the wild, we may
publish the fix sooner without coordinated disclosure — we'll tell
you and credit you regardless.

## Bug bounty

Pre-mainnet: not currently funded. Post-public-testnet: the team
intends to scope a bounty program; details will be published here
when defined.

## See also

- [`CONTRIBUTING.md`](CONTRIBUTING.md) — non-security contribution
  workflow.
- [`docs/architecture/security.md`](docs/architecture/security.md) —
  ingress + fuzz target inventory.
- [`docs/audit/mainnet-readiness-2026-05-15.md`](docs/audit/mainnet-readiness-2026-05-15.md) —
  current security + operational posture.
