# Visuals — source-of-truth policy

A small subset of `docs/visuals/` is intentionally bit-identically mirrored
between [`gsx-dag`](https://github.com/GlobalSettlementNetwork/gsx-dag)
and
[`gsx-lattice-protocol`](https://github.com/GlobalSettlementNetwork/gsx-lattice-protocol).
This lets the LTP repo render the core stack diagrams offline without a
cross-repo include or relative path that breaks on a shallow checkout.

## Canonical home

**`gsx-dag/docs/visuals/`** owns these files. Edit here first. The matching
copies in `gsx-lattice-protocol/docs/visuals/` are mirrors — propagate
your changes there in the same session (or a follow-up PR) and let the
drift-check CI job catch any oversight.

## Shared file set

Bit-identical across both repos:

- `mermaid/gsx-dag.md` — chain stack flow
- `mermaid/gsx-db.md` — substrate lattice + mutation pipeline
- `mermaid/ltp.md` — LTP lifecycle + security stack
- `gsx-dag.html` — DAG layer presentation
- `gsx-db.html` — DB layer presentation
- `ltp.html` — LTP layer presentation
- `gsx-ecosystem-atlas.html` — single-page ecosystem atlas
- `index.html` — landing page (mirrored verbatim including the LTP-specific
  link section; both repos point readers at the same cards)

The drift-check script's allow-list is the authoritative copy of this list.

## What's NOT shared

Each repo also carries its own diagrams that are repo-specific:

- **gsx-dag-only:** consensus deep-dives (`commit-rule.md` /
  `fast-path-and-slashing.md` / `governance-flow.md` / `dual-vm.md` /
  `scion-transport.md`), the `auth-dispatch.md` draft, the inline-Mermaid
  `README.md` (gsx-dag's entrypoint), and `excalidraw-archive/`.
- **LTP-only:** corridor/handshake/anchor-registry/dkg-ceremony Mermaid
  sources, its own `README.md` and `SUMMARY.md`.

The drift-check script's allow-list deliberately excludes these. Adding a
new diagram to the shared set is a 1-line PR to `SHARED_FILES=()` in
[`scripts/check-visuals-parity.sh`](../../scripts/check-visuals-parity.sh)
in **both** repos.

## Drift detection

`scripts/check-visuals-parity.sh` (run locally or in CI) compares the
shared set bit-for-bit. The same script lives in both repos with the same
allow-list; either repo's CI can flag drift introduced by the other.

CI workflow: [`.github/workflows/visuals-parity.yml`](../../.github/workflows/visuals-parity.yml).
The job is non-blocking — drift surfaces as a workflow failure on the
PR's checks panel but does not gate merge, because operator workflows
sometimes need to ship a quick fix to one repo and mirror in a follow-up.

## When to add a file to the shared set

Add when a diagram is referenced from both repos' docs *and* you want
LTP-side readers to be able to render it without resolving back to
gsx-dag. Examples of when NOT to share:

- A diagram that's gsx-dag-internal (e.g., a daemon-state-machine detail
  that LTP doesn't need to know about).
- A diagram that's LTP-internal (e.g., DKG ceremony, which is LTP runtime,
  not the on-chain LTP attestation surface).

Default to "not shared" unless the cross-repo reading order specifically
needs it.

## Cross-references

- [`scripts/check-visuals-parity.sh`](../../scripts/check-visuals-parity.sh) — drift detection
- [`.github/workflows/visuals-parity.yml`](../../.github/workflows/visuals-parity.yml) — CI wiring
- Linear: [GLO-762](https://linear.app/globalsettlement/issue/GLO-762/) (LTP-side visuals refresh) + the gsx-dag-side mirror issue tracked separately
