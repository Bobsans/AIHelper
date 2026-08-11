# Release blocker: named lifecycle lease probing

## Context

The Windows managed MCP lifecycle moved from exclusive lock files to named
mutex handles so the thin service launcher can transfer process ownership.
Status and runtime inspection still short-circuit when the old lock-file path
does not exist. Named mutex acquisition intentionally creates no file, so an
active lifecycle or instance lease is incorrectly reported as idle or absent.

The `v1.2.1` version bump also exposed release tests that hard-code the previous
`1.2.0` package version.

## Decision

- Treat the named mutex as the only Windows lease authority.
- Make mutex acquisition independent of filesystem state and remove directory
  creation from the mutex adapter.
- Probe lifecycle and instance leases directly without checking whether the
  legacy lock-file path exists.
- Keep status read-only: probing may create and close a named kernel object but
  must not create directories or files.
- Derive release-test version and updater `User-Agent` expectations from
  `CARGO_PKG_VERSION` instead of duplicating the current release number.

## Rejected alternatives

- Persistent marker files can become stale and would make read-only status
  mutate durable state.
- Reverting to exclusive file handles would broaden the change and risk the new
  launcher ownership handoff.

## Validation

- Run the four previously failing tests directly.
- Re-run `cargo fmt --all -- --check`.
- Re-run `cargo test --workspace --all-targets --locked`.
- Continue with debug and release builds only after the complete suite passes.
