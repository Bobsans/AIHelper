# 08 — Testing Strategy

**Severity: High.** The suite is substantial (~7.6k lines of integration tests plus
large in-file unit modules) and genuinely catches regressions. The problem is its
*shape*: because the code lacks seams, most behavior can only be tested by launching
a process, and that cost is now shaping the code itself.

## Findings

### 8.1 The pyramid is inverted

| Layer | Size | Mechanism |
|---|---|---|
| process-level integration | 7 590 lines (`tests/integration/*`) | `assert_cmd` spawning `ah` |
| in-file unit tests | large, e.g. `crates/ah-mcp/src/server.rs` 1 340 lines of tests, `crates/ah-runtime/src/lib.rs` ~1 040 | `#[cfg(test)] mod tests` |
| focused unit tests in `tests/` per crate | few | — |

`tests/integration/mcp.rs` alone is 2 280 lines. Every output assertion, every
`--quiet` check, every error message is verified by starting a subprocess, because
`println!` writes to the process (group 04) and configuration comes from a
process-global env var (`IsolatedAhCommand` sets `AH_CONFIG_DIR`,
`tests/integration/common.rs:31`).

### 8.2 Test hooks leak into production

Because there are no injectable ports for process spawning, the clock, or PATH
resolution, tests reach in through environment variables that ship in release
binaries:

- `AH_GITHUB_TEST_CREDENTIAL_SLEEP` (`plugins/ah-plugin-github/src/lib.rs:2798`)
- `AH_GITLAB_TEST_CREDENTIAL_SLEEP` (`plugins/ah-plugin-gitlab/src/lib.rs:2886`)
- `AH_POSTGRES_TEST_SYSTEM_PATH` (`plugins/ah-plugin-postgres/src/lib.rs:1124`) —
  overrides executable resolution
- `AH_RUN_IO_TEST` (`src/commands/run/io.rs`, test-scoped but still a name in the
  production namespace)

### 8.3 Large in-file test modules inflate the god files

`server.rs` is 3 991 lines, of which 1 340 are tests; `event_log.rs` is 1 845 with
564 test lines; `ah-runtime/src/lib.rs` is 2 612 with ~1 040. This is idiomatic Rust,
but at this scale it hides the real size of the production surface and makes the
files in group 05 look even more unmanageable than they are.

### 8.4 Missing test categories

| Category | Status | Why it matters |
|---|---|---|
| schema↔type agreement | **none** | the central drift risk of group 01; today only caught at runtime |
| golden snapshot of command catalog / manuals | **none** | prerequisite for every refactor in this program |
| golden snapshot of rendered text output | partial (a few in `src/output.rs`, `src/lib.rs`) | output determinism is a stated invariant |
| property/fuzz tests for redaction | **none** | ~550 lines of security-critical heuristics |
| fuzz tests for the curl and JSONPath parsers | **none** | untrusted input parsers (`src/commands/http/domain.rs:1086`, `:918`) |
| concurrency tests for parallel typed execution | minimal (`mcp_service/lifecycle/tests/concurrency.rs`, 54 lines) | the executor is the flagship path; the process-cwd issue (group 03) would be caught here |
| docs freshness | **none** | `docs/reference` has zero coupling to code |
| cross-version updater compatibility | manual (`scripts/release_smoke.py`) | highest-risk area (group 07) |

### 8.5 Determinism is asserted narrowly

`AGENTS.md` states text and JSON output must be deterministic, but only a handful
of tests assert exact rendered strings. Most integration tests assert substrings,
which passes even when spacing, ordering or styling changes.

## Why it hurts

- Feedback is slow, so contributors run less of the suite, so regressions land.
- The refactors proposed in groups 01–07 need a behavior-preserving safety net that
  does not currently exist at the required granularity.
- Security-critical code (redaction, credential handling, filesystem verification)
  has example-based tests only, which is the weakest coverage for exactly the code
  where adversarial input matters.

## Target design

### A. Seams first, tests second

The testability problems are consequences of the design problems in groups 02–04.
Each of these unlocks a test layer:

| Seam | From group | Unlocks |
|---|---|---|
| `Emitter<W>` | 04 | in-process assertions on all text and JSON output |
| `sdk::process` | 02 | credential/timeout tests without env vars or sleeps |
| `ServiceScheduler` | 06 | lifecycle tests on every platform |
| `ServiceGuard` | 07 | updater tests without a scheduler |
| request-scoped cwd | 03 | true concurrency tests |
| `schemars`-derived schemas | 01 | schema↔type agreement by construction |

### B. Golden snapshots as the refactoring harness

Introduce a snapshot layer (e.g. `insta`) covering:

- the full command catalog (descriptors, both schemas, effects, examples);
- all plugin manuals;
- `ah <domain> <command> --help` for every command;
- rendered text output for a fixed fixture set, with color on and off;
- rendered JSON output for the same fixtures;
- the full list of error codes with their rendered messages.

These snapshots are the mechanism that makes the rest of this program safe. They
should land **before** any structural change.

### C. Property and fuzz coverage where input is hostile

- `ah-redact`: property test — for any generated input containing a marked secret,
  the redacted output must not contain it. Plus a `cargo-fuzz` target.
- curl parser and JSONPath parser: fuzz for panics and unbounded allocation.
- `read_bounded_*` helpers: property test that the bound always holds.

### D. Move tests out of god files

Per-crate `tests/` directories for anything that only needs the public API; keep
`#[cfg(test)]` for genuinely private helpers. Combined with group 05, no file
should exceed ~800 production lines.

### E. Keep a thin, deliberate e2e layer

Process-level tests remain valuable for: argument parsing end to end, exit codes,
the managed-service lifecycle, and the updater. Target roughly 1–1.5k lines of
`assert_cmd` tests covering contracts that only exist at the process boundary,
down from 7.6k.

## Migration

1. Add the snapshot harness and capture the catalog, manuals and `--help` output.
   *Do this before anything in groups 01–07.*
2. Add rendered-output snapshots for one domain; use them to validate the `Emitter`
   migration; repeat per domain as `Emitter` rolls out.
3. Extract `ah-redact` with its tests; add property + fuzz targets.
4. Add the schema↔type agreement test as soon as `schemars` lands (group 01 step 2).
5. Replace env-var test hooks with injected ports as `sdk::process` lands.
6. Convert integration tests to in-process tests domain by domain, deleting the
   subprocess equivalents only once the in-process version asserts strictly more.
7. Add cross-version updater fixtures (v1.4 journal → current recovery, and back).
8. Add a CI job that regenerates `docs/reference` and fails on diff.

## Risks and invariants

- **Do not delete a subprocess test before its replacement asserts more.** The
  integration suite is currently the only regression net for several subsystems.
- **Snapshots must be reviewed, not blindly accepted.** Add a review rule: a PR that
  updates a snapshot must state why in the description.
- **Fuzz targets must not become required CI blockers** on first introduction; run
  them nightly until stable, then gate.

## Acceptance criteria

- Output, error rendering and catalog shape are covered by reviewed snapshots.
- No production code path reads a test-only environment variable.
- Redaction and both parsers have property and fuzz coverage.
- The process-level suite is a deliberate contract layer, not the default place to
  write a test.
- Full workspace test run stays under a few minutes on CI hardware.
