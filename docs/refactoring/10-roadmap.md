# 10 — Roadmap

Sequencing for the findings in groups 01–09. The ordering is chosen so that every
phase leaves the project shippable, and so that each phase makes the next one
cheaper rather than harder.

## Guiding rule

**Build the safety net, then remove duplication, then move code, then add capability.**

Structural moves without golden coverage are how behavior-preserving refactors stop
being behavior-preserving. Every phase below assumes phase 0 is complete.

## Phase 0 — Safety net and free wins

No architectural change. Everything here is mechanical and independently valuable.

| Work | Group | Why first |
|---|---|---|
| Golden snapshots: catalog, manuals, `--help`, text/JSON output, error codes | 08 | prerequisite for every later phase |
| `[workspace.package]` + `[workspace.dependencies]`, unify `sha2`, `rust-toolchain.toml`, MSRV | 09 | one PR, removes duplicate dependency builds |
| CI: clippy (allows retained), macOS, release build, `cargo deny`/`audit` | 09 | turns CI into a gate |
| Delete or implement `ConfigSource::Flags`/`Project` | 03 | stops the code describing a system that does not exist |
| Rename the two `transaction.rs` files | 07 | zero risk, immediate clarity |
| Extract `ah-redact` with property + fuzz tests | 04, 05, 08 | highest security value per line moved |

**Exit criterion:** a byte-level snapshot exists for every user-visible artifact, and
CI fails on clippy regressions.

## Phase 1 — Kill the duplication

The highest-leverage phase. Nothing moves between crates yet.

| Work | Group |
|---|---|
| `schemars`-derived output schemas, then input schemas; delete `typed_args` mappers | 01 |
| Generate manuals from descriptors; delete `src/plugins.rs` literals | 01 |
| Generate `docs/reference/*.md`; CI diff check | 01 |
| `ah-plugin-sdk`: `render`, `http`, `credentials`, `process` | 02 |
| Runtime-owned cancellation; delete five global registries | 02 |
| Delete `AH_*_TEST_*` environment seams | 02, 08 |
| `Emitter<W>`; remove `println!` from adapters; single `--quiet` handling | 04 |
| `From<RuntimeError> for ErrorDiagnostic`; delete three mapping tables | 04 |
| Box error payloads; remove `result_large_err` allows | 04, 09 |

**Exit criterion:** adding a command is a one-file change; no security-relevant code
exists in more than one copy; output is testable in-process.

## Phase 2 — Straighten the boundaries

Mostly moves, made safe by phases 0–1.

| Work | Group |
|---|---|
| `ctx_symbols` and `project/rules` become table-driven | 05 |
| Split `http/domain.rs` into `client`/`curl`/`jsonpath`/`assert`/`spec`; fuzz the parsers | 05, 08 |
| Move the secret-setup UI out of `ah-mcp`; split protocol/transport/shutdown/jobs | 05 |
| Split `event_log` into sink and rotation | 05 |
| Separate `ai/install` logic from progress rendering | 04, 05 |
| One `commands/*` layout; delete the eight `mod adapters` shims | 05 |
| Single-parse `Entry` enum; hidden subcommands replace env-var routing | 03 |
| `run()` split into parse → bootstrap → dispatch → report | 03 |
| Remove `set_current_dir`; request-scoped cwd; concurrency test | 03 |
| `ah-paths`; `platform::{fs,process,exec}` ports | 03, 06 |

**Exit criterion:** no file over ~800 production lines; one parse of argv; no
process-global working directory.

## Phase 3 — Decompose the crate

Now that boundaries are clean and the SDK exists, extraction is mechanical.

| Work | Group |
|---|---|
| `ServiceGuard` trait; invert the updater↔service dependency | 07 |
| Extract `ah-updater` from `src/updater/` | 07, 09 |
| Extract `ah-secrets`, `ah-observability`, `ah-service` | 09 |
| One `fsverify` module for updater hardening primitives | 07 |
| Extract domain crates last | 09 |

**Exit criterion:** the root crate is CLI wiring; every subsystem builds and tests
independently.

## Phase 4 — New capability

Only now is this cheap.

| Work | Group |
|---|---|
| Platform-neutral `ServiceSpec` + `ServiceScheduler`; Windows adapter as a projection | 06 |
| `SystemdUserScheduler`, `LaunchdScheduler` | 06 |
| `Forge` abstraction; collapse GitHub/GitLab duplication into a shared core | 02 |
| Cross-version updater compatibility fixtures | 07, 08 |
| Convert the integration suite to a deliberate thin contract layer | 08 |

**Exit criterion:** the managed service runs on three platforms with one lifecycle
test suite; a new forge plugin is a few hundred lines.

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
