---
description: Tag and ship a release (RELEASING.md procedure)
argument-hint: <version, e.g. 0.4.0>
---

Ship release $ARGUMENTS following `RELEASING.md` exactly:

1. Confirm the working tree is clean, on `main`, and CI is green.
2. Confirm `CHANGELOG.md` has a section for this version —
   `release.yml` extracts it verbatim as the release notes.
3. Run `/check`; abort on any red gate.
4. Tag `suwappu-dag-v$ARGUMENTS` and push the tag (ask tier — the human
   confirms the push). The `release.yml` workflow builds the 3-target
   matrix and publishes the GitHub Release.
5. Watch the release workflow to completion; verify the artifacts and
   SHA256SUMS are attached before declaring the release shipped.

Never re-tag a published version; a botched release gets a new patch
version.
