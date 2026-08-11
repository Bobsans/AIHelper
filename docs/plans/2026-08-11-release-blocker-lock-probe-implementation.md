# Release blocker lock probe implementation plan

## Objective

Restore correct Windows managed MCP lease observation for release `v1.2.1`
without reintroducing lock files or changing public contracts, and remove stale
release-version literals from tests. The complete release validation gate must
pass before any commit, push, tag, workflow dispatch, or publication.

## Workstreams

### Stream 1 – Named mutex lease observation

- Remove filesystem directory creation from Windows named mutex acquisition in
  `src/mcp_service/lock.rs`.
  - Definition of Done: `FileLease::try_acquire` creates only a named kernel
    object and does not write to the lease path or its parent.
  - Validation: lock unit tests and absent-service read-only lifecycle tests.
- Remove legacy `Path::exists` short-circuits from lifecycle and instance lease
  probes in `src/mcp_service/lifecycle.rs`.
  - Definition of Done: an open lifecycle lease reports `Busy`; an open instance
    lease participates in controlled shutdown; an absent lease reports idle
    without filesystem mutation.
  - Validation: the three previously failing lifecycle tests.

### Stream 2 – Version-aware release fixtures

- Replace the updater `User-Agent` test's hard-coded `1.2.0` expectation with
  `env!("CARGO_PKG_VERSION")` in `src/updater/github.rs`.
  - Definition of Done: the request header remains exact for any workspace
    release version.
  - Validation: the previously failing pagination/header test.
- Keep release-set fixtures derived from the current package version.
  - Definition of Done: signing tests validate `v1.2.1` without another
    release-specific literal.
  - Validation: `ah-release-tool` release-set integration tests.

### Stream 3 – Release gate

- Run the four failed tests directly through local AIHelper `1.2.1`.
- Run `cargo fmt --all -- --check`.
- Run `cargo test --workspace --all-targets --locked`.
- Run `cargo build --locked` and `cargo build --release --locked`.
- Inspect the complete release diff and proceed to the release commit only when
  every command reports success without timeout.

## Risks & Mitigations

- Named mutex probing can accidentally mutate disk if directory creation remains
  in the adapter. Remove it and retain read-only durable-byte assertions.
- Removing the existence guard can expose mutex API errors. Preserve the current
  conservative status reduction: lifecycle probe errors report `Busy`, instance
  probe errors do not assert liveness.
- Version literals can recur in intentional compatibility fixtures. Change only
  expectations that mean "current workspace version"; keep malformed, older,
  and newer-version test cases explicit.
- Rollback is limited to the two lease-probe changes and dynamic test
  expectations; no persistent data migration is introduced.
