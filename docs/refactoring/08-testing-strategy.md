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
| ~~fuzz tests for the curl and JSONPath parsers~~ generated-input tests | both parsers are their own modules now and have fixed-seed corpora; `cargo-fuzz` still deferred |
| concurrency tests for parallel typed execution | minimal (`mcp_service/lifecycle/tests/concurrency.rs`, 54 lines) | the executor is the flagship path; the process-cwd issue (group 03) would be caught here |
| docs freshness | ~~none~~ drift test | `docs/reference` is checked against the catalogs |
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

   **Started with `help.rs`, 129 lines to 88, and the first conversion found the
   thing that had been blocking every other one.** `AppError::print` renders to
   `eprintln!` and reads `std::env::args()`, so the text a user sees on a
   refusal could not be obtained without spawning `ah`. That is one instance of
   group 04's problem left behind by the `Emitter` migration.
   `AppError::console_text` returns the same string, and `print` calls it.

   With that, six of the nine tests moved in-process and now assert *more*:

   | Was | Is |
   |-----|----|
   | `--help` and `file --help` contain some names | already frozen whole, byte for byte, by `cli-help.snap` - the two tests were pure redundancy |
   | three unknown-domain suggestions, by `contains` | `cli::tests::diagnostics`, whole-string, against the real domain list |
   | a misspelled host subcommand, by `contains` | same, and it pinned a usage line the fragment assertions never checked |

   Two things the conversion turned up:

   - **A bare command tree is not the shipped one.** Every built-in domain is a
     plugin, so `build_cli_command(&[])` does not know `search` or `project`,
     and a typo against it resolves to nothing. The in-process tests build the
     real metadata from `plugins::builtins()`; a process-level test was
     supplying that implicitly, which is part of why these tests lived there.
   - **Where the parse error comes from decides where the test can live.** A
     *host* domain (`plugins`, `ai`, `secrets`, ...) has a clap subcommand tree,
     so `plugins lsit` fails during `parse_runtime_command` and is testable in
     process. A *plugin* domain parses its own arguments, so `project versoin`
     and `search text` fail inside the plugin and surface through the runtime;
     reaching those needs a `PluginManager`, and they stay process-level for now.

   What is left in `help.rs` is what only a process can show: that a refusal
   reaches stderr with a failing exit code and an empty stdout, that the plugin
   dispatch path renders the plugin's own parse error, and that `--json`
   switches the whole thing to a structured payload - which `print` decides from
   a process-global.

   **Then the harness the rest of this row needs.** `src/harness.rs`, behind a
   `harness` feature the package enables for itself as a dev-dependency, builds
   the registry the shipped binary builds - the built-in domains, the host
   commands, a temporary configuration directory and its own vault key - parses
   an argv with the production parser, dispatches it through the production
   manager, and returns what the command rendered:

   ```rust
   let run = Harness::new().in_directory(temp.path()).run(&["--json", "file", "read", "app.txt"]);
   assert_eq!(run.json()["line_count"], 1);
   ```

   What made it possible is `OutputSink`. Every built-in domain built its own
   `Emitter::stdio` *after* parsing its arguments, so nothing above it could
   reach the decision; the sink is now the host's parameter, threaded through
   `BuiltinPlugin::invoke_into` and `PluginManager::invoke_credentialed_into`.
   Production passes `Process` and nothing about it changes.

   Two things worth recording:

   - **The trait now has two pairs of entry points, and only one of each is the
     one to override.** A test's fake plugin overrode `invoke_observed`, which
     the runtime stopped calling, and its outcome silently disappeared - caught
     by that fake's own test. `invoke_observed` now says in its doc that it is
     the shorthand and `invoke_observed_into` is the override point.
   - **`file.rs` is the worked example**: its `file_json` helper was the choke
     point for thirteen tests, so replacing that one function moved all
     thirteen in process, and six refusal tests followed. Twenty-seven of its
     thirty-four spawns remain, and the rest of this row is that same work, file
     by file.

   The harness deliberately refuses what it cannot run: a command that is not a
   plugin-domain invocation panics naming itself, rather than asserting about
   something else.
7. ~~Add cross-version updater fixtures (v1.4 journal → current recovery, and
   back).~~ **(done.)** The on-disk plan, journal and helper self-check are
   frozen as fixtures, and the handoff command line is produced and validated
   from one ordered list, so a test can put the two halves together. Group 07's
   risk list says which part still has no in-process cover: the Windows
   launcher's handle plumbing.
9. ~~Share the plugins' HTTP mock server.~~ **(done as `ah-plugin-testkit`, a
   dev-dependency.)** Three hand-written copies had drifted - only two put the
   accepted socket back into blocking mode, only one bounded its accept loop,
   one carried its body as a `String` rather than bytes, and the three status
   reason tables listed different codes. The shared one is the union of every
   guard, and it removes 588 lines from the plugins.
8. ~~Add a CI job that regenerates `docs/reference` and fails on diff.~~ **(done as a
   drift test; generating it would rewrite prose)**

## Risks and invariants

- **Do not delete a subprocess test before its replacement asserts more.** The
  integration suite is currently the only regression net for several subsystems.
- **Snapshots must be reviewed, not blindly accepted.** Add a review rule: a PR that
  updates a snapshot must state why in the description.
- **Fuzz targets must not become required CI blockers** on first introduction; run
  them nightly until stable, then gate.

## Non-Windows failures in the process-level suite ~~(open)~~ **(fixed)**

Found by running the suite on Linux for the first time (see group 06, migration
step 5). **None of the three was a defect in the code under test.** All three
were the suite itself assuming the platform its author was on, which is the
argument for step 6 above stated by example.

| Test | What it turned out to be |
|------|--------------------------|
| `ai::ai_status_waits_for_a_slow_agent_probe` | The test replaces `PATH` with the project directory so only its shim is findable - and its shim then calls `sleep`, which lives on the `PATH` it just removed. `sh` reported the failure to a stderr nobody read, skipped the wait, and printed `[]`; `ah` correctly waited for a probe that returned in 3 ms. The Windows arm had never had the problem because it names `ping.exe` absolutely. The shim now sets its own `PATH`. |
| `mcp::http_transport_rejects_hostile_host_origin_and_oversized_body` | Both outcomes are correct refusals: answer `413`, or stop reading and close - and the second is the better one, because draining an attacker's body is the denial of service. Which happens depends on whether the socket buffers swallow the body before the close arrives. The test now accepts either and adds the assertion that was actually missing: the server still answers afterwards, which is what distinguishes a refusal from a crash. |
| `postgres::postgres_ping_uses_vault_credential_without_exposing_it` | `cargo test` has no reason to *link* a `cdylib`-only member - nothing depends on it at compile time - so it produced only the metadata. The test was passing on developer machines because a `cargo build` had happened at some point, which means it was asserting on ambient state. CI now builds the workspace before testing, because the suite genuinely needs the plugin libraries on disk. |

Two things worth keeping from this. The first test had been failing on two of
the three CI platforms since it was written, and its failure message
("status returned before the slow probe") pointed at the product rather than at
itself. And the third was the suite's dependency on ambient build state, which
no assertion had ever stated: `cargo test --workspace --all-targets` alone is
not enough to run this suite.

## Open flakes ~~(one open)~~ **(the ollama flake is fixed)**

The Ollama plugin's mock server set its listener non-blocking and never put the
*accepted* socket back into blocking mode. **On Windows an accepted socket
inherits the listener's non-blocking flag; POSIX does not**, so the first
`read_line` raced the client's request bytes and failed with `WouldBlock` - only
ever on Windows, and only when the timing went the wrong way.

It reproduced during this phase as
`ask_posts_generate_request_and_returns_json_output`, which is why the earlier
report named a different test: any test using that mock server could lose the
race. The two lines that fix it were **already present in the GitHub and GitLab
plugins' mock servers** - somebody hit this before, fixed their copy, and had no
way to carry it back. Three hand-written copies of `MockServer`, one missing a
fix, is the same lesson as group 02.

The earlier note said the cause was unknown and that the accept deadline had
been raised and reverted; neither touched the real cause.

**So the copies are gone.** `crates/ah-plugin-testkit` is a dev-dependency of
the three plugins and the only mock server left. It is the union of what the
three had, because each was missing something another had:

| | GitHub | GitLab | Ollama |
|---|---|---|---|
| accepted socket returned to blocking mode | yes | yes | **no - the flake** |
| bounded accept loop | yes | **no** | **no** |
| response body | bytes | bytes | **`String`, so no archive** |
| status reason phrases | 200/201/204/404 | fewer | 200/500 |

Two things the union added that none of them had. A `500` came back with the
reason phrase `OK` in two copies, because their tables did not list it - nothing
asserts on the phrase, but a mock that misreports its own status line is a bad
place to start debugging. And a test that queues a response the code never asks
for used to make `drop` wait out the whole 60-second accept timeout; the server
now stops when it is dropped.

588 lines left the plugins. The testkit has its own tests - ordering, header
case-insensitivity, byte bodies, an empty response, and the never-asked-for
queued response - which none of the three copies ever had.

## Acceptance criteria

- Output, error rendering and catalog shape are covered by reviewed snapshots.
- No production code path reads a test-only environment variable.
- Redaction and both parsers have property and fuzz coverage.
- The process-level suite is a deliberate contract layer, not the default place to
  write a test.
- Full workspace test run stays under a few minutes on CI hardware.
