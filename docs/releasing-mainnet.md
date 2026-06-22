# Mainnet binary release pipeline

**Status:** Spec — extends `RELEASING.md` (current testnet-
grade procedure) to the mainnet-grade procedure. Closes F.2
(#149). Implementation (the new GitHub Actions workflow) lands
at M+3 to M+6 once a Sigstore + Fulcio account is provisioned
under the foundation's GitHub organization.

**Audience:** release engineers, validator-operators verifying
release artifacts pre-bootstrap, security auditors (Track A),
exchange-listing reviewers.

**Authoritative inputs:**
- `suwappu-strategy/docs/mainnet-plan.md` Track F.2
- Existing `RELEASING.md` (testnet-grade)
- SLSA Build L3 + in-toto attestation specs
  ([slsa.dev/spec/v1.0/levels](https://slsa.dev/spec/v1.0/levels))

---

## 1. Why mainnet differs from testnet

Testnet releases optimize for **operator UX** — get a binary
out fast, accept downloaders pulling unverified tarballs from
GitHub Releases. Mainnet releases optimize for **supply-chain
integrity** — every binary published to mainnet validators
must be:

| Property | Mechanism | Verifier check |
|---|---|---|
| Authenticated | Sigstore cosign signature | `cosign verify-blob` against the foundation's pinned OIDC identity |
| Provenance-attested | in-toto SLSA L3 build provenance | `slsa-verifier verify-artifact` against the source repo + commit SHA |
| Reproducible | Pinned rust toolchain + deterministic Cargo.lock | Re-run the build → hash matches |
| SBOM-traceable | cargo-sbom (Rust) + npm-sbom (TS SDK) | Audit firms grep SBOM for known-vulnerable transitive deps |
| Multi-target | aarch64/x86_64-linux-musl statically linked | Operators verify the target triple matches their hardware |

The testnet RELEASING.md procedure is preserved verbatim for
testnet releases. This doc is the **additional** mainnet
procedure that runs alongside it.

---

## 2. Release artifact targets

Mainnet releases publish 4 binaries × 2 target triples =
**8 artifacts per release**:

| Binary | Source crate | Purpose |
|---|---|---|
| `suwappu-node` | `crates/suwappu-node` | Consensus validator daemon |
| `suwappu-l2-sequencer` | `crates/suwappu-l2-sequencer` (Track G G4.2) | L2 batch sequencer (post-G4) |
| `suwappu-l2-prover` | `crates/suwappu-l2-prover` (Track G G4.1) | SP1 prover daemon (post-G4) |
| `suwappu-bridge-relayer` | `crates/suwappu-bridge-relayer` (Track I I.3) | Foundation relayer for the LTP↔Ethereum/Solana bridges (post-I.3) |

Target triples (matches existing testnet workflow):

- `aarch64-unknown-linux-musl` (ARM64 server — preferred for Tier A buyers on AWS Graviton)
- `x86_64-unknown-linux-musl` (Intel/AMD — universal fallback)

Each binary is statically linked against musl + `openssl-sys`
vendored (per the existing workspace dep configuration). No
glibc requirements; runs on any modern Linux kernel.

---

## 3. Per-artifact signing flow

Every mainnet artifact gets four companion files:

```
suwappu-node-v1.0.0-aarch64-linux-musl
suwappu-node-v1.0.0-aarch64-linux-musl.sig         ← cosign signature
suwappu-node-v1.0.0-aarch64-linux-musl.intoto.jsonl ← SLSA L3 provenance
suwappu-node-v1.0.0-aarch64-linux-musl.sbom.json   ← CycloneDX SBOM
suwappu-node-v1.0.0-aarch64-linux-musl.sha256       ← legacy hash digest
```

### 3.1 Sigstore cosign

The release workflow signs with the foundation's GitHub OIDC
identity. No long-lived signing key; identity-bound:

```yaml
- uses: sigstore/cosign-installer@v3
- run: cosign sign-blob --yes "$ARTIFACT" \
    --output-signature "$ARTIFACT.sig" \
    --bundle "$ARTIFACT.bundle"
  env:
    COSIGN_EXPERIMENTAL: "true"
```

Verifier-side:

```sh
cosign verify-blob \
    --signature suwappu-node-v1.0.0-aarch64-linux-musl.sig \
    --certificate-identity-regexp "^https://github.com/Suwappu-Labs/suwappu-dag/" \
    --certificate-oidc-issuer "https://token.actions.githubusercontent.com" \
    suwappu-node-v1.0.0-aarch64-linux-musl
```

### 3.2 SLSA L3 in-toto attestation

The release workflow uses
[`slsa-framework/slsa-github-generator`](https://github.com/slsa-framework/slsa-github-generator)
which is officially audited for SLSA L3:

```yaml
build-provenance:
  permissions:
    id-token: write
    contents: write
    actions: read
  uses: slsa-framework/slsa-github-generator/.github/workflows/generator_generic_slsa3.yml@v2.0.0
  with:
    base64-subjects: ${{ needs.build.outputs.hashes }}
```

The `.intoto.jsonl` carries:
- Source repo URL + commit SHA
- Builder identity (GitHub Actions runner)
- Build steps (literal workflow file content)
- Artifact hashes
- Builder + materials timestamps

Verifier-side:

```sh
slsa-verifier verify-artifact suwappu-node-v1.0.0-aarch64-linux-musl \
    --provenance-path suwappu-node-v1.0.0-aarch64-linux-musl.intoto.jsonl \
    --source-uri github.com/Suwappu-Labs/suwappu-dag \
    --source-tag v1.0.0
```

### 3.3 CycloneDX SBOM

Rust: `cargo cyclonedx --format json --output-format json`.
TS SDK (per-release): `npm sbom --sbom-format cyclonedx`.

The SBOM file lists every transitive dep + version + license.
Auditors grep this for known-vulnerable advisories (matches
`cargo-deny --deny advisories` but in a portable format).

---

## 4. Reproducible builds

Mainnet builds MUST be deterministic enough that two
independent re-runs of the same git tag produce byte-identical
binaries.

### 4.1 Required pinning

- **Rust toolchain**: `rust-toolchain.toml` at the workspace
  root pins to a specific stable version (`channel =
  "1.78.0"`, matching the existing `rust-version.workspace`
  declaration in Cargo.toml). MUST be updated only via PR
  with corresponding IQ.
- **Cargo.lock**: committed (already is). Mainnet workflow
  runs `cargo build --locked` to refuse if Cargo.lock would
  change.
- **Native deps**: `openssl-sys = { features = ["vendored"] }`
  pins the openssl source vendored in-tree (already in
  workspace deps); no system OpenSSL leaks.
- **Build flags**: `RUSTFLAGS=-D warnings -C link-arg=-s`
  (strip + deny warnings).
- **Workspace profile**: `[profile.release] lto = "thin",
  codegen-units = 1, panic = "abort"` (already pinned in
  Cargo.toml).

### 4.2 Verification

A reproducibility verifier re-runs the build and compares
hashes:

```sh
git clone https://github.com/Suwappu-Labs/suwappu-dag
cd suwappu-dag
git checkout v1.0.0
cargo build --release --locked -p suwappu-node \
    --target aarch64-unknown-linux-musl
sha256sum target/aarch64-unknown-linux-musl/release/suwappu-node
# Compare to the published .sha256 file
```

The foundation MUST publish at least one independent
reproduction of every release within 7 days (anchored to a
Track A audit firm or one of the larger Tier A buyer's
operations teams).

---

## 5. The new release workflow shape

Replace `.github/workflows/release.yml` (current testnet
shape) with `.github/workflows/release-mainnet.yml` that
runs on tags matching `v*.*.*`:

```yaml
name: release-mainnet
on:
  push:
    tags:
      - 'v*.*.*'

permissions:
  contents: write
  id-token: write
  actions: read

jobs:
  build:
    strategy:
      matrix:
        binary: [suwappu-node, suwappu-l2-sequencer, suwappu-l2-prover, suwappu-bridge-relayer]
        target: [aarch64-unknown-linux-musl, x86_64-unknown-linux-musl]
    runs-on: ubuntu-latest
    outputs:
      hashes: ${{ steps.hash.outputs.hashes }}
    steps:
      - uses: actions/checkout@v4
        with:
          fetch-depth: 0
      - uses: dtolnay/rust-toolchain@1.78.0
        with:
          targets: ${{ matrix.target }}
      - run: |
          cargo build --release --locked \
            -p ${{ matrix.binary }} \
            --target ${{ matrix.target }}
      - run: |
          cargo install cargo-cyclonedx
          cargo cyclonedx --format json \
            --output-format json \
            --output-path "${{ matrix.binary }}.sbom.json"
      - id: hash
        run: |
          cd target/${{ matrix.target }}/release
          BASE64_HASH=$(sha256sum "${{ matrix.binary }}" | base64 -w0)
          echo "hashes=$BASE64_HASH" >> "$GITHUB_OUTPUT"
      - uses: sigstore/cosign-installer@v3
      - run: |
          cd target/${{ matrix.target }}/release
          cosign sign-blob --yes "${{ matrix.binary }}" \
            --output-signature "${{ matrix.binary }}.sig"
      - uses: actions/upload-artifact@v4
        with:
          name: ${{ matrix.binary }}-${{ matrix.target }}
          path: |
            target/${{ matrix.target }}/release/${{ matrix.binary }}
            target/${{ matrix.target }}/release/${{ matrix.binary }}.sig
            ${{ matrix.binary }}.sbom.json

  provenance:
    needs: build
    permissions:
      id-token: write
      contents: write
      actions: read
    uses: slsa-framework/slsa-github-generator/.github/workflows/generator_generic_slsa3.yml@v2.0.0
    with:
      base64-subjects: ${{ needs.build.outputs.hashes }}

  release:
    needs: [build, provenance]
    runs-on: ubuntu-latest
    steps:
      - uses: actions/download-artifact@v4
      - uses: softprops/action-gh-release@v2
        with:
          generate_release_notes: true
          fail_on_unmatched_files: true
          files: |
            **/suwappu-*-*
            **/*.sig
            **/*.sbom.json
            **/*.intoto.jsonl
```

### 5.1 What's different from `.github/workflows/release.yml`

Compared to the existing testnet workflow:

- Pinned rust toolchain via `dtolnay/rust-toolchain@1.78.0`
  (testnet uses `stable`)
- `--locked` flag on `cargo build`
- Sigstore cosign signing step
- SLSA L3 provenance via the official generator
- SBOM generation via cargo-cyclonedx
- Multi-binary matrix (testnet only ships suwappu-node)
- No mac-os runner (the source-of-cost in testnet); musl-only
  Linux targets

### 5.2 What's preserved

- `softprops/action-gh-release@v2` for the actual upload
- `generate_release_notes: true` reads CHANGELOG.md
- Tag pattern `v*.*.*` triggers
- Workspace version bump procedure (per the existing
  RELEASING.md §2)

---

## 6. Operator-side verification runbook

Before bootstrapping a mainnet validator off a downloaded
binary, the operator MUST run:

```sh
# 1. Download artifact + companions.
RELEASE=v1.0.0
TARGET=aarch64-unknown-linux-musl
BIN=suwappu-node-${RELEASE}-${TARGET}
gh release download $RELEASE --repo Suwappu-Labs/suwappu-dag \
    --pattern "$BIN*"

# 2. Verify the Sigstore signature.
cosign verify-blob \
    --signature "${BIN}.sig" \
    --certificate-identity-regexp "^https://github.com/Suwappu-Labs/suwappu-dag/" \
    --certificate-oidc-issuer "https://token.actions.githubusercontent.com" \
    "$BIN"

# 3. Verify the SLSA provenance.
slsa-verifier verify-artifact "$BIN" \
    --provenance-path "${BIN}.intoto.jsonl" \
    --source-uri github.com/Suwappu-Labs/suwappu-dag \
    --source-tag "$RELEASE"

# 4. Verify the legacy SHA256 (defensive).
sha256sum -c "${BIN}.sha256"

# 5. Optionally re-build to verify reproducibility.
git clone --depth 1 --branch $RELEASE \
    https://github.com/Suwappu-Labs/suwappu-dag
cd suwappu-dag
cargo build --release --locked -p suwappu-node --target $TARGET
diff <(sha256sum target/$TARGET/release/suwappu-node | awk '{print $1}') \
     <(awk '{print $1}' ../${BIN}.sha256)
```

Any of steps 2-4 failing means **do not bootstrap with this
binary**. Operators report the failure to security@suwappu.bot
within 1 hour; foundation triggers emergency response (this is
a supply-chain attack candidate).

---

## 7. Foundation pre-publish checklist

Each mainnet release publication:

- [ ] All Track A audit findings for the version are closed
      (no Critical / High open)
- [ ] Workspace version bumped via PR (per existing
      RELEASING.md §2)
- [ ] CHANGELOG.md `## Unreleased` moved to `## v1.x.x`
- [ ] Tag signed: `git tag -s v1.0.0 -m "..."` (foundation
      signing key, NOT the cosign OIDC identity; the GPG
      signature is for git provenance, cosign is for binary
      provenance)
- [ ] `release-mainnet.yml` workflow ran green; all 8
      artifacts published with all 4 companion files each
- [ ] Independent reproducer (audit firm or Tier A buyer)
      confirmed byte-identical re-build within 7 days
- [ ] Validator-operator notice on Discord + status page +
      foundation board ratification of the release

---

## 8. Mainnet release cadence

Per Track F.2 in the strategic plan:

- **Patch (`v1.0.x → v1.0.(x+1)`):** as needed; passes Track
  A audit-firm review of the diff
- **Minor (`v1.x.0 → v1.(x+1).0`):** every ~2 months
  post-mainnet; bundles new features behind feature flags
- **Major (`v1.x → v2.x`):** governance-ratified hard fork
  ONLY; the testnet hard-fork dry run (Track B.4) is the
  rehearsal target for every major

There is no "fast-track" mainnet release path. Even critical-
severity zero-days go through the full pipeline (audit-firm
fast review + accelerated workflow); the foundation accepts
the 24–48hr disclosure → release window in exchange for the
supply-chain integrity guarantee.

---

## 9. Cross-references

- **Testnet release procedure**: `RELEASING.md` (preserved
  for testnet releases; mainnet uses the workflow above)
- **F.4 24/7 on-call**: `#151` — release emergencies route
  through the on-call pager
- **F.3 bug bounty**: `#150` — Immunefi tier-1 bounty becomes
  live with the mainnet launch; in-scope artifacts are
  exactly the binaries published by this pipeline
- **Track A audit-firm engagements** (#114 #115 #116 #117) —
  audit firms verify the release-workflow shape as part of
  pre-mainnet review
- **B.4 hard-fork dry run** (#122) — exercises the
  release-mainnet.yml workflow at M15 with a test tag

---

## 10. Change log

| Date | Change | Source |
|---|---|---|
| 2026-05-17 | Initial draft | F.2 (issue #149); spec only, workflow lands M+3 to M+6 once Sigstore + Fulcio account provisioned under the foundation's GitHub org |
