# 10 — Roadmap

Sequencing for the findings in groups 01–09. The ordering is chosen so that every
phase leaves the project shippable, and so that each phase makes the next one
cheaper rather than harder.

## Status

**Phase 0 is complete.** What changed, and what it bought:

| Item                            | Outcome                                                                                                                                                                                                                                                                                                           |
|---------------------------------|-------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| Golden snapshots                | `tests/snapshots/` freezes the typed command catalog, the plugin manuals, the whole CLI help tree, and the error-code table. Generated in-process by `src/snapshots.rs`, so they need no terminal, no git repository and no plugin directory. Regenerate with `AH_UPDATE_SNAPSHOTS=1 cargo test --lib snapshots`. |
| `ah-redact` extracted           | The redaction engine and the credential detectors moved out of `src/event_log.rs` and `crates/ah-mcp/src/server.rs` into one crate with 13 dedicated tests, including generated-secret leak properties and a hostile-input case. `event_log.rs`: 1 845 → 1 195 lines.                                             |
| Workspace dependency management | `[workspace.package]` and `[workspace.dependencies]` added. `sha2` drift resolved (0.10.9 → 0.11), removing the duplicate `sha2`/`digest`/`block-buffer` trees from `Cargo.lock`.                                                                                                                                 |
| Toolchain and MSRV              | `rust-toolchain.toml` pins 1.97.1. `rust-version = "1.88"` is declared and **verified by building it** — the metadata-derived guess of 1.86 was wrong, because `jsonc-parser` uses let-chains.                                                                                                                    |
| CI as a gate                    | clippy `-D warnings` on all three platforms, a macOS runner, an MSRV job, a release build, `cargo doc -D warnings`, and `cargo deny`.                                                                                                                                                                             |
| Clippy clean                    | 12 findings fixed, including an unset `truncate` on the vault lock file and two `MutexGuard`s held across await points.                                                                                                                                                                                           |
| `ConfigSource`                  | Deleted. All five variants and all three accessors were dead code, and two variants had no producer at all.                                                                                                                                                                                                       |
| `transaction.rs` collision      | Renamed to `ah-updater-core::plan` (the model) and `ah-update-helper::apply` (the execution), each with a module doc stating the split.                                                                                                                                                                           |

Deliberately **not** done in phase 0:

- Rendered text/JSON output snapshots per domain. They belong with the `Emitter`
  migration in phase 1, where they can validate something instead of just
  recording the status quo.
- Converging the three redaction *policies*. The engine is shared now; merging the
  field-selection logic is a semantic change, not a move, so it does not belong in
  a safety-net phase.
- A `cargo-fuzz` target for the redaction engine and the parsers. It needs a
  nightly toolchain and a separate CI lane; the generated-input tests cover the
  leak property in the meantime.

A struck-through row is landed and verified. Everything else is open.

## Guiding rule

**Build the safety net, then remove duplication, then move code, then add capability.**

Structural moves without golden coverage are how behavior-preserving refactors stop
being behavior-preserving. Every phase below assumes phase 0 is complete.

## Phase 0 — Safety net and free wins

No architectural change. Everything here is mechanical and independently valuable.

| Work                                                                                                         | Group      | Why first                                              |
|--------------------------------------------------------------------------------------------------------------|------------|--------------------------------------------------------|
| ~~Golden snapshots: catalog, manuals, `--help`, error codes~~ **(done)**                                     | 08         | prerequisite for every later phase                     |
| ~~`[workspace.package]` + `[workspace.dependencies]`, unify `sha2`, `rust-toolchain.toml`, MSRV~~ **(done)** | 09         | one PR, removes duplicate dependency builds            |
| ~~CI: clippy `-D warnings`, macOS, release build, `cargo deny`~~ **(done)**                                  | 09         | turns CI into a gate                                   |
| ~~Delete or implement `ConfigSource::Flags`/`Project`~~ **(done)**                                           | 03         | stops the code describing a system that does not exist |
| ~~Rename the two `transaction.rs` files~~ **(done)**                                                         | 07         | zero risk, immediate clarity                           |
| ~~Extract `ah-redact` with property tests~~ **(done)**                                                       | 04, 05, 08 | highest security value per line moved                  |

**Exit criterion:** a byte-level snapshot exists for every user-visible artifact, and
CI fails on clippy regressions.

## Phase 1 — Kill the duplication

The highest-leverage phase. Nothing moves between crates yet.

| Work                                                                                        | Group  |
|---------------------------------------------------------------------------------------------|--------|
| ~~`schemars`-derived output schemas for the eight built-in domains~~ **(done)**             | 01     |
| ~~`schemars`-derived input schemas and `typed_args` for seven built-in domains~~ **(done)** | 01     |
| ~~`http` input schemas: wire type separate from the CLI type~~ **(done)**                   | 01     |
| ~~Host command schemas, input and output~~ **(done)**                                       | 01     |
| ~~Schemas for the dynamic plugins: ollama, gitlab, postgres, github~~ **(done)**            | 01     |
| ~~Tie the manual to the CLI it documents~~ **(done; generating it would rewrite prose)**    | 01     |
| ~~Tie `docs/reference/*.md` to the catalog~~ **(done as a drift test)**                     | 01     |
| ~~`ah-plugin-sdk`: `credentials`, `render`, `http`~~ **(done; `process` dropped)**          | 02     |
| ~~One cancellation registry instead of five~~ **(done)**                                    | 02     |
| ~~Delete `AH_*_TEST_*` environment seams~~ **(done; one of three was real)**                | 02, 08 |
| ~~`Emitter`; remove `println!` from adapters; single `--quiet` handling~~ **(done)**        | 04     |
| ~~`From<RuntimeError> for ErrorDiagnostic`; delete three mapping tables~~ **(done)**        | 04     |
| ~~Box error payloads; remove `result_large_err` allows~~ **(done)**                         | 04, 09 |

**Exit criterion:** adding a command is a one-file change; no security-relevant code
exists in more than one copy; output is testable in-process.

## Phase 2 — Straighten the boundaries

Mostly moves, made safe by phases 0–1.

| Work                                                                                     | Group  |
|------------------------------------------------------------------------------------------|--------|
| ~~`ctx_symbols` and `project/rules` become table-driven~~ **(done)**                     | 05     |
| ~~Split `http/domain.rs` into `client`/`curl`/`jsonpath`/`assert`/`spec`~~ **(done)**    | 05, 08 |
| ~~Move the secret-setup UI out of `ah-mcp`; split protocol/transport/shutdown/jobs~~     | 05     |
| ~~Split `event_log` into record shaping and rotation~~ **(done)**                        | 05     |
| ~~Separate `ai/install` logic from progress rendering~~ **(done)**                       | 04, 05 |
| ~~One `commands/*` layout; delete the eight `mod adapters` shims~~ **(done)**            | 05     |
| ~~Single-parse `Entry` enum; hidden flag replaces env-var routing~~ **(done)**            | 03     |
| ~~`run()` split into parse → bootstrap → dispatch → report~~ **(done)**                  | 03     |
| ~~Remove `set_current_dir`; request-scoped cwd; concurrency test~~ **(done)**            | 03     |
| ~~`ah-paths`; `platform::{fs,process,exec}` ports~~ **(done)**                           | 03, 06 |

**Exit criterion:** no file over ~800 production lines; one parse of argv; no
process-global working directory.

**Measured. Phase 2 is complete.**

*No process-global working directory:* met. `set_current_dir` is gone from the
workspace.

*One parse of argv:* met as far as it can be, and the criterion was overstated.
argv is **scanned** exactly once - `entry::detect` walks it and returns an
`entry::Route` - and that answer selects **at most one** further parse. Before,
each candidate route asked argv for itself, so a plain `ah git status` walked
the argument list four times: `detect`, the upgrade route's gate, the
managed-service route's gate, and then the real parse. A literal single parse is
not reachable and should not have been written as the target: the full CLI's
shape depends on which plugins loaded, and answering `ah mcp service status`
before that discovery is the entire point of the early routes. Two parsers over
one scan is the floor. The three `NotUpgrade`/`NotManaged` "not mine" variants
are gone with the duplicate scans that produced them.

*Env-var routing:* met, by a hidden global flag rather than the hidden
subcommands the row proposed - `--internal-handoff <kind>` is a contract between
two of our own binaries, and a flag keeps it in the parser. The two legacy
variables stay on the read side only, because invariant 4 requires a helper from
one release to be able to drive the `ah` of another.

*Files over ~800 production lines:* met for every file of production logic, and
the criterion needed the distinction it did not draw. Twenty files were over;
fourteen were split, each as a pure move verified by sorting every line of code
before and after:

| Was                                | Lines | Now                                                 | Largest |
|------------------------------------|-------|-----------------------------------------------------|---------|
| `ah-update-helper/src/apply.rs`    |  2195 | 7 modules: layout, fsverify, journal, record, managed, backup | 704 |
| `mcp_service/lifecycle.rs`         |  2027 | one module per operation + the lease and lookups     |     545 |
| `ah-runtime/src/lib.rs`            |  1826 | error, ports, outcome, manager, invoke, dynamic      |     521 |
| `commands/project/domain.rs`       |  1181 | output, detect, suggest, version                     |     589 |
| `cli.rs`                           |  1177 | command, parse, passthrough, redact, suggest          |     341 |
| `ai/install.rs`                    |  1304 | report, spec, apply, inspect, paths                  |     500 |
| `ah-plugin-api/src/lib.rs`         |  1259 | abi, invocation, typed, catalog, metadata, text, sdk  |     430 |
| `error.rs`                         |  1047 | render, message                                      |     670 |
| `plugins.rs`                       |   921 | one module per builtin                                |     143 |
| `runtime_flow.rs`                  |   884 | invoke, mcp_serve, record                            |     494 |
| `commands/git/domain.rs`           |   848 | output, parse                                        |     461 |
| `commands/http.rs`                 |   846 | args, catalog                                        |     577 |
| `ah-mcp/src/server.rs`             |   833 | config, state, catalog                               |     602 |
| `commands/http/domain/spec.rs`     |   803 | format, interpolate, extract, junit                  |     495 |
| the three plugins' `typed.rs`      | 901/848/847 | each gets `typed/catalog.rs`                    |     791 |

What is still over 800, and why it stays:

- **`commands/project/rules.rs` (1040).** Lines 79-953 are one
  `static RULES: &[FileRule]` literal. Splitting it moves rows between files and
  buys nothing; `commands/layout.rs` already registers it as data rather than
  logic.
- **Eight test files**, the largest `tests/integration/mcp.rs` at 2280. The
  criterion says *production* lines, so these are outside it by its own wording,
  and phase 4 owns them: "convert the integration suite to a deliberate thin
  contract layer".

Two things the splits turned up that the criterion did not ask for. Sixteen
functions were reachable from outside their file with nothing outside using
them; moving them one level deeper made the compiler reject the re-export, so
they narrowed. And `commands/layout.rs` caught two attempts that would have
grown a command module a fourth layer - it exists for exactly that, and the two
files it made me justify are registered in it with a reason.

## Phase 3 — Decompose the crate

Now that boundaries are clean and the SDK exists, extraction is mechanical.

| Work                                                        | Group  |
|-------------------------------------------------------------|--------|
| ~~`ServiceGuard` trait; invert the updater↔service dependency~~ **(done)** | 07 |
| ~~Extract `ah-updater` from `src/updater/`~~ **(done)**      | 07, 09 |
| ~~Extract `ah-secrets`, `ah-observability`, `ah-service`~~ **(done, plus `ah-config`)** | 09 |
| ~~One `fsverify` module for updater hardening primitives~~ **(done)** | 07 |
| ~~Extract domain crates last~~ **(done as one `ah-domains`, plus `ah-output`)** | 09 |

**Exit criterion:** the root crate is CLI wiring; every subsystem builds and tests
independently. **Phase 3 is complete.** Nineteen crates; `src/*.rs` is 3 296
lines and holds the CLI, the plugin registration, the host commands and the
snapshots. Every crate builds and its tests pass with `aihelper` absent from its
graph.

**`ServiceGuard` landed, with two notes.** The trait is `hold`/`capture`/`stop`/
`restore`, and every method after `hold` takes the hold as a parameter, so what
the three replaced functions said in their names (`*_while_locked`) is now
checked by the signature. `src/updater/` no longer names anything in
`mcp_service::lifecycle`.

- **`NoServiceGuard` was not written.** The row proposed it to replace the
  `cfg(windows)` gates, but those gates are on the updater as a whole - a
  non-Windows `upgrade` returns `UnsupportedPlatform` before any guard is
  consulted - so a second implementation would be dead code today. It becomes
  real when a second platform gets a managed service (phase 4).
- **`FileLease` is still `mcp_service`'s**, named once behind a `ServiceHold`
  alias, because the update helper inherits its handle. Giving it a neutral home
  is part of extracting `ah-updater`, and so is the larger question that row has
  to answer: `AppError` is the root crate's type, and every updater signature
  returns it.

**`ah-updater` landed, and the blockers were never its own code.** `cargo tree`
shows neither `aihelper` nor anything of `mcp_service` in its graph, and its 35
tests run without them. Five things had to move first, and four of them were
worth doing on their own:

| Blocker                                          | Answer                        |
|--------------------------------------------------|-------------------------------|
| `AppError`, named by every signature             | `ah-error` (a relocation - it depended on nothing but `ah_plugin_api`) |
| atomic JSON writes in `installation.rs`          | `ah-persist`, which nine subsystems share |
| the lifecycle lease held across an activation    | `ah_platform::lease`, reporting `io::Error` |
| `CREATE_PROCESS_LOCK`, in three copies           | `ah_platform::exec`, one lock and one explanation |
| the bounded process runner, and the rendering    | ports (`Host`) and `upgrade::render` |

Two of those were latent problems rather than obstacles. The lock guards
process-global inheritance state, and a fourth spawn site added beside one of
the three copies would not have known to take a lock at all. And building
`ah-platform` alone surfaced two `windows-sys` features that workspace feature
unification had been supplying for it.

What stayed in the root is what the criterion allows: `src/upgrade/` is the clap
route, the two port implementations and the renderer. The root crate also lost
`sha2`, `zip` and `ed25519-dalek`, which it no longer needs.

**`ah-secrets`, `ah-observability` and `ah-service` followed, and so did
`ah-config`,** which was not on the list and had to go first: the log directory
and the vault both resolve against it. Each of the four builds and tests with
the root crate absent from its graph.

`ah-service` needed the same two separations the updater did - the mechanism
printed its own output and its input type carried `GlobalOptions` through six
variants - so `src/service/` now holds the route and the renderer, and
`lifecycle::run` takes an `Operation` and returns a `Report`. `InstallSettings`
replaced `InstallOptions`: the only CLI value the mechanism actually needed was
`--limit`, which is written into the definition and outlives the invocation.

Two things worth recording about doing it this way:

- **Building each crate alone is the point, not a side effect.** It found five
  missing `windows-sys`/`windows` features across `ah-platform` and
  `ah-service` that workspace feature unification had been supplying. A crate
  that only ever builds inside the workspace never states what it needs.
- **The orphan rule pointed at the right answer twice.** `SecretResolver for
  VaultStore` could no longer live in the root once both halves were foreign to
  it, and the impl belongs with the vault. Same for the `ServiceGuard` impl and
  `mcp_service`.

The root crate is 4441 lines of `src/*.rs` plus the command modules, against a
`crates/` directory of nineteen.

**The hardening primitives are one implementation each,** and the row's wording
turned out to matter. The reparse-point and hard-link checks were already single
copies in `ah-platform`; what was duplicated was the *composition* of them - "a
plain regular file with exactly one name" - six times, under four different
names, which is why counting by name had missed two of them. Three of the six
omitted the hard-link half.

Group 07 says the union is the requirement, not the intersection, so
`ah_platform::fs::direct_file` performs all four checks and three call sites
gained a refusal they did not have: the candidate staging check, the recovery
identity read and the transaction file check. `read_bounded` went from three
copies to one, and `encode_digest` from six to one - a digest compared as text
has to be spelled the same everywhere or the comparison silently fails.

Two sites keep their own check on purpose and now say so in the code: the
running executable may legitimately be hard-linked into place by whoever
unpacked a portable install, and the cargo-marker probe is detection rather than
verification. Naming them is the point - the next reader "deduplicating" either
would change what an installation is allowed to look like.

**The domains went last, as one `ah-domains`,** with `ah-output` extracted first
because they all render and so all reached into the CLI for `Emitter`,
`OutputMode` and `GlobalOptions`. `GlobalOptions` left the clap layer with them:
it is what a request reports under, not how a flag was spelled.

One crate rather than eight, which group 09 offers as an alternative. The only
cross-domain edge is `task` → `run::io`, so per-domain crates would enforce a
separation nothing is currently violating, at the cost of nine manifests. The
boundary worth drawing was the CLI/domain one. If compile times ever justify
splitting further, the `safety` (file, search, ctx) and `git_status` (git, ctx)
clusters are where the seams already are.

Three things this row taught that the earlier ones had not:

- **The `pub(crate)` caution is real, and the compiler is the auditor.** Group 09
  warns against blanket-`pub`. Driving it from the errors instead produced a
  named surface - `execute`, `command_catalog`, `invoke_typed`,
  `bind_resolved_credentials`, `run::io`, two corpora - rather than a module
  opened wholesale. The earlier extractions did widen in bulk; that is a debt
  worth revisiting if any of those crates ever grows a second consumer.
- **One crate's `cfg(test)` is invisible to another's.** Two snapshot corpora had
  to move to the domains (a domain test cannot reach up into the CLI), and then
  the CLI's snapshots could not see them. They are behind a `fixtures` feature
  the root enables as a dev-dependency, so a release build compiles neither.
- **`commands/layout.rs` moved with the modules it checks.** An invariant that
  reads `CARGO_MANIFEST_DIR` belongs to the crate whose layout it is.

The three lifecycle helpers that remain (`snapshot_status`, `install_quietly`,
`start_quietly`) serve `ai install --transport managed`. Group 07 counted them
among "six update-specific helpers"; they are a different consumer, and the
inversion does not touch them.

## Phase 4 — New capability

Only now is this cheap.

| Work                                                                                 | Group  |
|--------------------------------------------------------------------------------------|--------|
| ~~Platform-neutral `ServiceSpec` + `ServiceScheduler`; Windows adapter as a projection~~ **(done)** | 06     |
| ~~`SystemdUserScheduler`~~ **(done)**, `LaunchdScheduler`                            | 06     |
| ~~`Forge` abstraction~~; **collapse GitHub/GitLab duplication into a shared core** *(the two duplicated loops done; see below)* | 02     |
| ~~Cross-version updater compatibility fixtures~~ **(on-disk done; the handoff argv half done)** | 07, 08 |
| Convert the integration suite to a deliberate thin contract layer                    | 08     |

**Exit criterion:** the managed service runs on three platforms with one lifecycle
test suite; a new forge plugin is a few hundred lines.

**The scheduler port landed, and the seam that was sealed shut was not the one
group 06 named.** `LifecycleService<S, R>` was already generic; what pinned it
to Windows was that it asked Windows who the user was, five times, through
`current_user_sid`. `ServiceScheduler::identity` replaced those calls, and the
lifecycle's 57 tests - which already drove fakes - lost the `#[cfg(windows)]`
that had kept them from ever running anywhere else. The whole suite also
compiles and passes with `UnsupportedScheduler` in place of the Windows
adapter, which is the check that the layer above the port is genuinely neutral.

`lifecycle/` went from 57 `cfg` attributes to zero and `ah-service` from 72 to
17; the remainder is `paths.rs` calling Windows APIs plus the one alias that
names this platform's adapter.

**Then the row's second half turned out to be blocked on Linux not building at
all,** which only became visible once the lifecycle tests stopped being
`#[cfg(windows)]` and someone actually ran them there. Four defects, three of
them left by phase 3's extractions and all four sitting in a CI matrix that has
covered Linux and macOS since phase 0:

| Defect | Fix |
|--------|-----|
| `ah-observability` used `libc::O_NOFOLLOW` with no `libc` dependency | `libc` became a workspace dependency, declared by the two crates that use it |
| `ah-persist`'s `REPLACE_RETRY_TIMEOUT` was `cfg(windows)` but used unconditionally | ungated; only Windows ever waits, but the value is always passed |
| 106 dead-code and unused-import errors under `-D warnings` across `ah-update-helper` and `ah-updater` | one `cfg_attr(not(windows), allow(...))` per crate, with the reason: off Windows both refuse before reaching any of it |
| `FileLease` was Windows-only, so every lifecycle operation failed off Windows | `flock` on Unix, which the kernel releases with the descriptor - the property the named mutex was chosen for |

The lease is the one with consequences. Taking a lock file *writes*, and
`status` is read-only - so `lock::is_free` answers "nobody holds it" without
opening anything when the lease's directory does not exist. The read-only-status
test caught that on the first Linux run, which is the argument for the whole
exercise: the invariant was already written down, and only a platform where the
lease is a file could violate it.

**Linux is now clean end to end** - `cargo fmt --check`, `cargo clippy
--workspace --all-targets -- -D warnings`, the MSRV check, `cargo build
--workspace`, and the whole test suite including all 207 process-level tests. 43
lifecycle tests run there where none did before.

The last three failures were in the process-level suite and none was a defect in
the code under test; each is recorded in group 08 with what it turned out to be.
The pattern across all three: a test that asserts about the product while
actually depending on its author's platform - a shim that calls `sleep` after
deleting `PATH`, a client that assumes a refusal always arrives as a status
code, and an assertion on a build artifact the test's own command does not
produce. CI now runs `cargo build --workspace` before `cargo test`, because the
suite loads plugins as dynamic libraries and `cargo test` never links a
`cdylib`-only member.

Three notes worth keeping about the port itself, because each is a place the
target design had to be argued with rather than transcribed:

- **The proposed five-method trait would have narrowed a safety check.**
  `install/observe/start/stop/uninstall` cannot express "stop the instance whose
  id is X and whose engine pid is Y", which is how a stop avoids terminating a
  process it has not identified. Invariant 5 says security checks may be
  deduplicated, never narrowed; the port kept all six operations and made their
  *types* neutral instead.
- **Drift had to become the adapter's, behind an associated type.** A generic
  comparison of `ServiceSpec` against an observation sees only the properties
  the neutral model names - six of the twenty-seven Windows compares. So
  `Native` carries the platform's own reading, `drift()` compares it against
  the adapter's own projection, and the lifecycle sees only `DriftEntry`.
- **Cargo cannot target-gate a `[[bin]]`,** so the last migration step of group
  06 is closed as impossible rather than carried: `required-features` would let
  an ordinary Windows build omit the worker binary and fail at install time,
  and a separate crate is still built. The stub `main` that exits 1 stays.

The Windows task is byte-identical, asserted rather than assumed: the
projection builds the argument string from a `Vec<String>` and a unit test pins
the result, on any platform. No snapshot moved.

**Then the second adapter landed, and it paid for the port twice over.** The
managed MCP service now installs, starts, stops, restarts, reports drift and
uninstalls on Linux as a systemd user unit - verified against a real user
manager, not simulated. `systemd_unit.rs` is the projection and the comparison
(data, tested on every platform), `systemd_scheduler.rs` runs
`systemctl --user`, and the lifecycle above them did not change.

What the second adapter was worth is what it found. Three places were still
Windows-shaped while calling themselves neutral, and none could have been
noticed with one adapter:

| Where | What it did |
|-------|-------------|
| `CurrentPointer::validate` | required the persisted registration identity to start with `\`, so no unit name could be stored at all |
| `status::is_registration_drift` | matched the ten field prefixes *Windows* produces, so a second adapter's drift was classified as not-registration drift and status reported `installed` for a unit somebody had edited |
| `ai::managed::is_supported` | `cfg!(windows)`, so `ai install --transport managed` refused on a platform that now has a service |

The middle one is the lesson: a classifier keyed on one platform's vocabulary
does not fail when a second arrives, it answers wrongly. It is now expressed as
the two exceptions rather than the ten matches, which says the same thing about
Windows and cannot go stale when an adapter compares a new property. The third
became `ServiceScheduler::SUPPORTED`, an associated const, so the next platform
to gain an adapter gains the `ai` route with it.

Two smaller things worth keeping:

- **The lifecycle tests now wear the host's projection.** The scripted
  scheduler's `Native` is `TaskSpec` on Windows and `UnitSpec` elsewhere, and
  the three tests that poked a Windows field now ask the harness to introduce
  drift and report which fields it changed. The suite tests whichever platform
  CI is on, and no test names a platform's property.
- **Unix uninstall leaves the two lock files.** They are the lease, held while
  the uninstall runs; unlinking a file whose `flock` you hold would let a
  concurrent process take a second "exclusive" lease. Windows has no such files
  because its lease is a named mutex. Recorded in group 06 rather than fixed.

**The cross-version fixtures landed, and the two directions turned out to need
different kinds of test.** Reading what an older release wrote is a real test:
the frozen v1 plan, journal and helper self-check are parsed and validated by
today's types. Writing what an older release can read cannot run the old code,
so it is a byte comparison against the same fixtures - and it is `deny_unknown_
fields` that makes that the right test. An older reader *rejects* a document
carrying a field it does not know, so the rule is that every field added since
v1 must be `skip_serializing_if`. The upgrade fixture proves all three of them
are absent from an ordinary plan today.

The command-line handoff is the more fragile contract, because the helper
validates by position: the frozen argv test refuses a renamed flag at any of the
six positions and a swapped pair.

Both directions are covered. The order now lives once, in
`ah_updater_core::HANDOFF_FLAGS`, with two builders beside it - the roots `ah`
knows before the launch, and the inherited lease and event only the launcher
knows. The parser validates against that list rather than against literals at
hard-coded indices, and one test puts the two halves together and parses them.
That pairing exists nowhere else in the repository: `ah` builds the halves in
two different modules, one of them Windows-only, so before this there was no
place where "what `ah` sends" and "what the helper accepts" could be compared.

What no in-process test can reach is the Windows launcher's handle plumbing -
duplicating the lease into the child, the attribute list, the acknowledgement
wait. `scripts/release_smoke.py` is its cover, and that is stated rather than
implied.

`LaunchdScheduler` and macOS acceptance are what remain of the portability row.
It is
deliberately not attempted here: there is no macOS to run `launchctl` against,
and an unverified service manager is worse for a macOS user than the honest
refusal they get today. What can land without a Mac is the plist projection,
and it has no consumer until the adapter exists.

**The `Forge` row was started from the other end, and the row's own sketch
turned out to be wrong.** A `trait Forge` with `issues()`/`logs()` describes the
product surface, which is exactly where the two plugins genuinely differ -
GitHub pages past pull requests and has a search API, GitLab has neither;
GitHub unzips a log archive, GitLab streams a trace. Their `execute_issues`
functions are thirty lines each and share almost no control flow.

What was still duplicated, after phases 1-3 had taken out credentials, HTTP and
rendering, was two loops, and both are now in the SDK:

- `sdk::logs::scan_lines` - read a stream against a byte budget, strip the
  runner's control sequences, keep the lines `--grep`/`--warnings-only` select,
  stop at the line limit and report that it stopped. Fifty-five lines,
  near-verbatim in both. The budget is a value, so GitHub spends one
  `--max-expanded-bytes` across every archive entry; the failures are returned
  rather than rendered, so each plugin keeps its own diagnostic codes.
- `sdk::poll::until_ready` - the wait loop. **The two copies had drifted:**
  GitLab re-checked the deadline after sleeping and GitHub did not, so
  `github.run.wait` could issue one more request after `--timeout` had passed.

239 lines left the two plugins and 161 came back, and both loops now have unit
tests of their own. Two further findings:

- **The state/status style tables were left alone, deliberately.** Same shape,
  different vocabularies - `open` vs `opened`, `cancelled` vs `canceled`,
  non-overlapping pending sets. Merging them would either lie about both forges
  or narrow the styling, and the shared part is a six-line `match`. Recorded in
  group 02 so the next reader does not "finish the job".
- **The last of the cancellation row went with it:** five copies of
  `cancelled_response`, differing only in the domain name and the one-line
  summary, became one function in `ah_plugin_api::cancellation`.

And a flake the program had written off got its cause: the Ollama mock server
never put its *accepted* socket back into blocking mode, and on Windows an
accepted socket inherits the listener's non-blocking flag while on POSIX it does
not. The fix was already in the GitHub and GitLab mock servers - somebody hit it
before and had no way to carry the fix back. Three hand-written `MockServer`
copies, one missing a fix, is group 02's lesson applied to the test harness.

**So the copies became one crate,** `ah-plugin-testkit`, a dev-dependency of the
three plugins. It is deliberately the union, because each copy was missing
something another had: the blocking-mode line (one lacked it - the flake), a
bounded accept loop (two lacked it), a byte response body (one carried a
`String`, so it could not express a log archive at all), and a status reason
table that listed the codes its own tests used. Two of the three answered a
`500` with the reason phrase `OK`.

588 lines left the plugins, and the shared server has tests of its own -
ordering, header case-insensitivity, byte bodies, an empty response, and a
queued response the code never asks for, which used to make `drop` wait out the
whole 60-second accept timeout. None of the three copies had ever been tested.

## Cross-cutting invariants

Restating, because every phase is constrained by them:

1. **Deterministic output.** Text and JSON stay byte-stable unless a diff is
   deliberately recorded in a snapshot review.
2. **Released JSON field names are frozen** absent an explicit breaking-change decision.
3. **Plugin ABI compatibility.** Rust-side conveniences never change the C ABI;
   ABI changes go through version negotiation.
4. **Updater cross-version compatibility.** On-disk journals and cross-process
   handoffs must interoperate across at least one release in both directions.
5. **Security checks may be deduplicated, never narrowed.** The union of existing
   checks is the requirement.
6. **Every behavior change ships with success-path and failure-path tests.**

## Sequencing dependencies

```
Phase 0 (snapshots, workspace, CI, ah-redact)
   |
   +--> Phase 1 (schemas, SDK, Emitter, error model)
            |
            +--> Phase 2 (module splits, single-parse bootstrap, paths)
                     |
                     +--> Phase 3 (crate extraction, ServiceGuard)
                              |
                              +--> Phase 4 (portability, Forge, e2e slimming)
```

Two hard edges worth naming:

- **Domain crate extraction must follow the SDK** (phase 1 → phase 3), otherwise each
  extracted crate carries a private copy of the shared helpers.
- **The updater handoff change is a release of its own.** It is the only item in the
  program that can break an in-flight upgrade on a user machine; give it a dedicated
  acceptance run and a deprecation window rather than bundling it into a phase.
