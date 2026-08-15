# Support

## Getting help

- **Bug reports and feature requests**: open a GitHub issue on this repository
  using the issue templates. Include the crate or client involved, the commit
  or release version, and a minimal reproduction where possible.
- **Security vulnerabilities**: do **not** open a public issue — follow
  [SECURITY.md](SECURITY.md) (GitHub Private Vulnerability Reporting).
- **Running a node / devnet questions**: start with [DEVNET.md](DEVNET.md) for
  quickstart, then [OPERATIONS.md](OPERATIONS.md) for the full runbooks.

## Self-service references

- Architecture: `docs/architecture/` (start at `docs/architecture/overview.md`)
- Contributor rules and CI expectations: [CONTRIBUTING.md](CONTRIBUTING.md)
- Release process and versioning: [RELEASING.md](RELEASING.md)
- Design decision records: `docs/iq/`

## Verifying your environment

```bash
./scripts/check.sh                  # fmt + clippy + tests + crypto boundary + cargo-deny
./scripts/check-crypto-boundary.sh  # lane-separation check only (fast, no toolchain)
```

On constrained machines, skip workspace-wide cargo commands and let CI
validate instead (see CLAUDE.md §Local development).
