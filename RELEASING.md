# Releasing suwappu-dag

Public releases publish binaries to GitHub Releases for external
developers. The cadence is roughly:

| Version step | Cadence | When |
|---|---|---|
| Patch (`0.1.0` → `0.1.1`) | Whenever a fix or small additive change merges to `main` and CI is green for >24h. | Continuous. |
| Minor (`0.1.x` → `0.2.0`) | Every ~2–4 weeks while pre-1.0. Bundles new SDK methods / wire-format additive changes / new public types. | Planned. |
| Major (`0.x` → `1.0`, `1.x` → `2.0`) | When the API stability promise in `clients/rust-sdk/src/lib.rs` graduates. | Coordinated. |

Hard fork (incompatible wire-format change, new chain id, etc.) is a
SEPARATE concern from versioning — see `docs/iq/` for the IQ that
ratified each on-wire change. A hard fork can land in any minor
version; the devnet wipes-and-regenesis on that boundary.

## Checklist

1. **Confirm `main` is green.** All workflows on the latest commit must
   be ✅ in `gh run list --branch main --limit 6 --json conclusion`. If
   `docs` is the only failure and it's the Pages-not-enabled flavor,
   that's OK — it's an org-admin toggle, not a release blocker.

2. **Bump the workspace version.** Edit `Cargo.toml`'s
   `[workspace.package].version` (every workspace crate inherits via
   `version.workspace = true`).

   ```sh
   # patch:  0.1.0 -> 0.1.1
   # minor:  0.1.0 -> 0.2.0
   # major:  0.1.0 -> 1.0.0
   sed -i.bak -e "s/^version = \"0\.1\.0\"/version = \"0.1.1\"/" Cargo.toml
   rm Cargo.toml.bak
   cargo update --workspace  # refresh Cargo.lock with the new version
   ```

3. **Update `CHANGELOG.md`.** Move the contents of `## Unreleased` into
   a new `## <new-version>` section (with today's date). The release
   workflow's `gh release create` step reads this section verbatim for
   the GitHub Release notes.

   ```markdown
   ## Unreleased

   ## 0.1.1 — 2026-05-16

   ### Added
   - …

   ### Changed
   - …

   ### Fixed
   - …
   ```

4. **Commit + push the bump.**

   ```sh
   git add Cargo.toml Cargo.lock CHANGELOG.md
   HUSKY=0 git commit -m "release: 0.1.1"
   git push origin main
   ```

5. **Tag and push.** Annotated tag — the release workflow uses
   `--verify-tag` so a lightweight tag is rejected.

   ```sh
   VERSION=0.1.1
   git tag -a "suwappu-dag-v${VERSION}" -m "suwappu-dag ${VERSION}"
   git push origin "suwappu-dag-v${VERSION}"
   ```

6. **Watch the release workflow.** The push triggers
   `.github/workflows/release.yml` which builds three platforms:
   - `x86_64-unknown-linux-musl` on `ubuntu-latest`
   - `x86_64-apple-darwin` on `macos-13`
   - `aarch64-apple-darwin` on `macos-14`

   ```sh
   gh run watch
   ```

7. **Verify the Release page.** `gh release view suwappu-dag-v${VERSION}`
   should list three `*.tar.gz` archives + a `SHA256SUMS` file.

8. **Smoke test on at least one platform.**

   ```sh
   gh release download suwappu-dag-v${VERSION} --pattern '*linux-musl*'
   tar -xzf suwappu-dag-${VERSION}-x86_64-unknown-linux-musl.tar.gz
   ./suwappu-dag-${VERSION}-x86_64-unknown-linux-musl/suwappu-node --help
   ```

9. **Roll the devnet (optional, per the wipe policy).** If this is a
   patch release, keep state. If minor, follow OPERATIONS.md §
   "Update validator binary" to push the new binary to each region.
   If major + hard fork, OPERATIONS.md § "Devnet wipe + regenesis".

10. **Announce.** Post the Release URL to the SUWAPPU Discord + the
    `#announcements` channel in the team Slack.

## Recovery

If the release workflow fails partway through:

- **`build` job fails on one platform:** binaries for the other
  platforms are uploaded as artifacts but no Release is created.
  Fix the failing platform, then re-run via
  `gh workflow run release.yml -f tag=suwappu-dag-v${VERSION}`.
- **`release` job fails (rare — only `gh release create`):** the
  tag exists but no Release page. Delete the tag's draft Release
  if present, then re-run the workflow.
- **Released a bad binary:** publish a new patch immediately
  (`0.1.1` → `0.1.2`). Do NOT delete the bad Release; external
  devs may already have it locally — they need a fresh version
  to migrate to. Add a `## Yanked: 0.1.1` note in the CHANGELOG.

## Pre-flight gotchas

- **The `SUWAPPU_DB_DEPLOY_KEY` secret must be present** on the repo (or
  inherited from the org). Without it, the workflow's `cargo build`
  fails to fetch `gsxdb-bridge`. Check via Repo Settings →
  Secrets → Actions.
- **CI billing.** Each release run consumes ~15 minutes of macOS
  runner time (the most expensive class). If billing is exhausted,
  the workflow fails before any build job runs.
- **`suwappu-faucet` crate may not exist yet.** Pre-G3, the release
  workflow gates the suwappu-faucet build behind a directory existence
  check and skips silently. Post-G3, the binary appears automatically.

## See also

- `.github/workflows/release.yml` — the actual workflow.
- `CHANGELOG.md` — version history.
- `DEVNET.md` — how external devs use the published binaries.
- `OPERATIONS.md` — how the team rolls a release out to the devnet.
