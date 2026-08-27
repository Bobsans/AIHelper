# 06 — Platform Portability

**Severity: Medium** for correctness today, **High** for extensibility: the managed
service — a headline capability — cannot be brought to a second platform without
restructuring first.

## Findings

### 6.1 The managed MCP service is Windows-only by construction

- [`src/mcp_service/lifecycle.rs:44`](../../src/mcp_service/lifecycle.rs)
  `execute` is one `#[cfg(windows)]` block; the `#[cfg(not(windows))]` arm returns
  `MCP_SERVICE_UNSUPPORTED_PLATFORM`.
- Ten more `#[cfg(windows)]` gates in the same file (`:99`–`:172`) hide the entire
  update-integration surface on other platforms.
- [`src/bin/ah-mcp-service.rs:9`](../../src/bin/ah-mcp-service.rs) — the non-Windows
  `main` prints an error and exits 1; the whole binary is a Windows Job Object
  supervisor.
- `src/mcp_service/paths.rs` has 11 `cfg` sites.

### 6.2 The right abstraction exists but is unreachable

`LifecycleService<S: Scheduler, R: ReadinessProbe>`
([`src/mcp_service/lifecycle.rs:191`](../../src/mcp_service/lifecycle.rs)) is
correctly generic. But everything *above* it is `cfg(windows)`, and the only
implementation is `WindowsTaskScheduler`. The seam was built and then sealed shut:
today the generic parameter buys testability (the lifecycle tests do use fakes) but
not portability.

Worse, the *desired* spec type is Windows-shaped:
`DesiredTaskSpec` ([`src/mcp_service/scheduler.rs:18`](../../src/mcp_service/scheduler.rs))
has 27 fields, all Task Scheduler concepts — `principal_logon_type`,
`disallow_start_on_batteries`, `multiple_instances`, `run_only_if_idle`. A systemd
or launchd adapter cannot implement this trait meaningfully.

### 6.3 173 `cfg` sites across 31 files

Platform branching is spread through business logic rather than concentrated in
adapters: `src/persistence.rs` (two `replace_file` implementations),
`src/commands/run/io.rs` (Windows program resolution, `PATHEXT`),
`src/updater/installation.rs` (reparse points, hard links),
`crates/ah-update-helper/*` (six files). Some of this is unavoidable and correct;
the problem is that it lives inline instead of behind a named platform port.

### 6.4 Path resolution is reimplemented per subsystem

| Subsystem | Location | Reads |
|---|---|---|
| host config | `src/config.rs:74` | `AH_CONFIG_DIR`, `XDG_CONFIG_HOME`, `APPDATA`, `HOME` |
| ai targets | `src/ai/targets.rs:342` | `HOME` |
| opencode config | `src/ai/opencode_config.rs` | `XDG_CONFIG_HOME` |
| service paths | `src/mcp_service/paths.rs` | `USERNAME`, Windows known folders |
| updater | `src/updater/installation.rs`, `recovery.rs` | `APPDATA` |
| postgres plugin | `plugins/ah-plugin-postgres/src/lib.rs` | `AH_CONFIG_DIR`, `AH_CACHE_DIR`, `XDG_CONFIG_HOME`, `XDG_CACHE_HOME`, `APPDATA`, `LOCALAPPDATA`, `HOME` |

Six independent notions of "where do AIHelper files live", with different fallback
orders. A user who sets `AH_CONFIG_DIR` gets different degrees of respect from
different subsystems.

Note also that `ai/targets.rs:244` hard-codes `.config/opencode` as `user_dir` on
every platform, including Windows — correct for OpenCode, but it demonstrates that
target path policy is per-tool knowledge that belongs in one place.

**Corrected while acting on this.** Two of the six are not duplication and must
not be merged away:

- The managed service resolves LocalAppData through `SHGetKnownFolderPath`, not
  `%LOCALAPPDATA%`. A service must not take its state directory from an
  environment variable a caller can set; the difference is the point.
- `copilot_user_mcp_path` and `opencode_config::config_directory` locate *other
  tools'* configuration. That is per-tool knowledge, as the paragraph above
  says — a different owner from AIHelper's own directories.

The remaining four — host config, postgres plugin, updater installation,
updater recovery — plus `targets::home_dir` were genuine duplication, and the
postgres plugin's copy was character-for-character identical to the host's.

## Why it hurts

- macOS and Linux users get the CLI and `mcp serve` but not the managed service,
  and the code shape means adding it is a rewrite rather than an addition.
- CI runs `ubuntu-latest` and `windows-latest` only (group 09), so macOS-specific
  path and keyring behavior is unverified despite `keyring` being configured with
  `apple-native`.
- Inline `cfg` blocks double the reading cost of every affected function and make it
  easy to fix a bug on one platform only.

## Target design

### A. A platform-neutral service model

Replace the Windows-shaped `DesiredTaskSpec` with a neutral desired state plus a
platform-specific projection:

```rust
pub struct ServiceSpec {
    pub id: ServiceId,
    pub executable: PathBuf,
    pub arguments: Vec<String>,
    pub working_directory: PathBuf,
    pub start: StartPolicy,          // AtLogon | OnDemand
    pub restart: RestartPolicy,
    pub resource: ResourceLimits,    // execution time limit, instance policy
}

pub trait ServiceScheduler {
    fn install(&self, spec: &ServiceSpec) -> Result<(), SchedulerError>;
    fn observe(&self) -> Result<Option<ObservedService>, SchedulerError>;
    fn start(&self) -> Result<(), SchedulerError>;
    fn stop(&self, grace: Duration) -> Result<(), SchedulerError>;
    fn uninstall(&self) -> Result<(), SchedulerError>;
}
```

Adapters: `WindowsTaskScheduler` (projects `ServiceSpec` onto the existing 27-field
task definition), `SystemdUserScheduler` (writes a `~/.config/systemd/user` unit),
`LaunchdScheduler` (writes a `LaunchAgents` plist), and `UnsupportedScheduler`
returning a structured `Unsupported` — so the *call sites* stop being `cfg`-gated
and the failure mode is data, not compilation.

Drift detection stays generic by comparing `ServiceSpec` against `ObservedService`;
platform-specific drift fields (battery policy, idle policy) become adapter-internal
concerns reported through a generic `DriftEntry`.

### B. One `ah-paths` module

Single owner for: config dir, cache dir, data/state dir, log dir, plugin dirs,
service dirs, home. Env overrides resolved once, provenance recorded once
(this is also the honest version of `ConfigSource`, see group 03). Plugins receive
resolved paths through the execution context instead of re-deriving them.

### C. Platform ports, not inline `cfg`

Concentrate the remaining platform code into named modules with identical
signatures — `platform::fs` (atomic replace, hard links, reparse points),
`platform::process` (spawn flags, job objects, process groups),
`platform::exec` (`PATHEXT`, shebang handling). Business logic calls the port; only
the port has `cfg`.

## Migration

1. ~~Extract `ah-paths`; migrate host, updater, then plugins.~~ **(done)** No
   behaviour change: every platform's policy is now a table the tests read on
   any machine, which the `cfg` blocks made impossible. The service is
   deliberately excluded, see 6.4.
2. ~~Introduce `platform::fs` / `platform::exec`; move existing `cfg` pairs in
   as-is.~~ **(done)** as the `ah-platform` crate - a crate rather than a module
   because `ah-update-helper` and the postgres plugin held copies too.

   `platform::process` was **not** extracted, and the reason is that 6.3
   overstated it:

   - Spawn flags already have a port. `ah_plugin_api::noninteractive_command`
     is it, and every plugin already goes through it. Moving it would churn
     the surface every plugin links against for no behavioural gain.
   - The two job-object users overlap but are not copies.
     `commands/run/windows_job.rs` also queries accounting information and
     terminates a tree; `bin/ah-mcp-service.rs` is a supervisor whose whole
     body is that one job. Step 6 below removes the second one from
     non-Windows builds entirely, which is the cheaper fix and is already on
     the list.
   - Process *groups* off Windows are `command_group`, a dependency, not code
     of ours.

   What was genuine duplication: the reparse-point check (five copies, three
   spellings of one constant), the hard-link check (two), `MoveFileExW` (two),
   and PATH+PATHEXT resolution (three, which disagreed - one defaulted to
   `.exe` alone when `PATHEXT` was unset, and sorted its candidates
   alphabetically, so a `.bat` shim would beat a real `.exe`).
3. ~~Introduce `ServiceSpec` + `ServiceScheduler`; reimplement
   `WindowsTaskScheduler` as a projection.~~ **(done)** `spec.rs` holds the
   neutral model, `scheduler.rs` the port, `windows_task.rs` the projection and
   the 27-property comparison, and `windows_scheduler.rs` only the COM calls.
   Three things the sketch above got wrong, each for the same reason - it was
   written from the outside:

   - **The five-method trait would have narrowed a safety check.**
     `install/observe/start/stop/uninstall` cannot say "stop the one instance
     whose id is X and whose engine pid is Y", which is how the lifecycle
     avoids terminating a process it has not identified. The port kept its six
     operations; what became neutral is their *types*.
   - **Drift belongs to the adapter, behind an associated type.** Comparing a
     desired `ServiceSpec` against an observation would only ever see the
     properties the neutral model happens to name - six of twenty-seven. So
     `ServiceScheduler::Native` carries the platform's own full reading,
     `drift()` compares it against the adapter's own projection, and the
     lifecycle only ever sees `Vec<DriftEntry>`.
   - **`identity()` was the change that mattered.** The lifecycle asked Windows
     who the current user was in five places (`current_user_sid`), which is
     what actually pinned it to one platform - not the scheduler trait, which
     was already generic. Asking the adapter instead is what let the tests
     move.

   The projection is data, not COM, so it compiles and is tested everywhere.
   `command_line` reproduces the registered argument string byte for byte from
   a `Vec<String>` by quoting only what could be re-split; the byte-identical
   task the risk list demands is asserted directly, on any platform.
4. ~~Remove the `cfg(windows)` gates from `lifecycle::execute`; route the
   unsupported case through `UnsupportedScheduler`~~ **(done)** `lifecycle/`
   went from 57 `cfg` attributes to none, and `ah-service` as a whole from 72
   to 17 - `paths.rs`'s Windows API calls, and the one alias that names this
   platform's adapter. The unsupported error text and code are unchanged: they
   now come from `scheduler::unsupported_platform`, which the non-Windows
   `ServicePaths::discover` also uses, so the four copies of that message
   became one.

   `ai/managed.rs` lost four of its five gates the same way - by asking
   `is_supported()` for a value rather than branching at compile time. Its own
   `AI_MANAGED_UNSUPPORTED` stayed: it is a different released diagnostic from
   the scheduler's, and collapsing `detect()` naively would have changed which
   code a non-Windows `ah ai install --transport managed` reports.

   **The lifecycle's 57 tests now run on every platform.** They drove fake
   schedulers already, but every one was `#[cfg(windows)]`, because `install`
   reached past the fake for the user's SID. That is the acceptance criterion's
   real content, and it is what proves the layer above the port is neutral: the
   suite also compiles and passes with `UnsupportedScheduler` substituted for
   the Windows adapter.

   The update helpers keep their gates. They are `ah-updater`'s, the updater is
   Windows-only as a whole, and a second implementation of `ServiceGuard` is
   still the dead code phase 3 declined to write.
5. ~~Add `SystemdUserScheduler`~~ **(done)**, then `LaunchdScheduler`; add
   `macos-latest` to CI. **(the matrix already has macOS; see the acceptance
   criteria below.)**

   `systemd_unit.rs` is the projection and the comparison, data only and tested
   on every platform; `systemd_scheduler.rs` runs `systemctl --user`. The split
   is `windows_task` / `windows_scheduler` again, for the same reason.

   Four design answers the Windows side did not have to give:

   - **The unit file is the registration.** Windows hands a definition to the
     Task Scheduler and reads it back through COM; systemd reads a file we own,
     so the readback is that file - located through the manager's
     `FragmentPath`, which is what proves the manager loaded *ours* rather than
     one shadowing it from `/etc/systemd/user`.
   - **The ownership marker lives in an `[X-AIHelper]` section.** systemd
     ignores a section whose name begins with `X-` outright:
     `systemd-analyze verify` exits 0 with no output and the journal is silent.
     A marker in an unknown *key* would have produced a warning on every load.
   - **`InvocationID` is the instance identity.** The lifecycle stops "the
     invocation whose id is X and whose main pid is Y", and systemd's
     per-start 128-bit `InvocationID` answers that exactly. This is the second
     time the port's shape was decided by the stop path, and the second time it
     turned out to fit.
   - **`NeedDaemonReload` is drift.** A file the manager has not re-read is a
     unit whose behaviour differs from what the file says, which the Windows
     model has no equivalent for - the Task Scheduler holds one copy. It is a
     compared property rather than a special case.

   **The second adapter found three places where the "neutral" layer was still
   Windows-shaped.** All three were invisible while there was one adapter:

   | Where | What it did | Fix |
   |-------|-------------|-----|
   | `CurrentPointer::validate` | required `task_path` to start with `\`, so no unit name could ever be persisted | `scheduler::validate_registration_identity`, one rule per platform: rooted on Windows, a single path component elsewhere |
   | `status::is_registration_drift` | a list of the ten field prefixes *Windows* produces, so `service.*` and `unit_file.*` were classified as not-registration drift - status said `installed` for a unit somebody had edited | the two exceptions (`lifecycle.`, `runtime.`) named instead, which says the same thing about Windows and cannot go stale |
   | `ai::managed::is_supported` | `cfg!(windows)` | `ServiceScheduler::SUPPORTED`, an associated const only `UnsupportedScheduler` sets to false - so a platform gaining an adapter gains `ai install --transport managed` without an edit |

   The middle one is the one worth remembering: a *classifier* keyed on a
   platform's vocabulary reports the wrong status rather than failing, so
   nothing would have caught it except a second vocabulary.

   **The lifecycle tests now wear the host's projection.** The scripted
   scheduler's `Native` is `TaskSpec` on Windows and `UnitSpec` elsewhere, and
   the three tests that used to poke a Windows field ask the harness to
   "introduce drift" and report which fields it changed. So the suite exercises
   the real identity rule, the real registration shape and the real comparison
   of whichever platform CI is on, and no test names a platform's property any
   more.

   One deliberate difference on Unix: uninstall leaves `instance.lock` and
   `lifecycle.lock` behind. They are the lease, held while the uninstall runs;
   unlinking a file whose `flock` you hold would let a concurrent process create
   a new one and take a second "exclusive" lease. Windows has no such files
   because its lease is a named mutex.

   Verified against a real user manager (systemd 255): install, idempotent
   reinstall, restart, stop, start, drift detection after a hand-edited
   `Restart=`, repair by reinstall, uninstall, idempotent uninstall, and a
   foreign unit at our name - which is refused with
   `MCP_SERVICE_TASK_CONFLICT` and left untouched.

   **Four things had to be fixed before this could be started, and all four
   were found by building the workspace on Linux for the first time.** The CI
   matrix has covered `ubuntu-latest` and `macos-latest` since phase 0, so
   these were failing there and nobody had looked; everyone develops on
   Windows.

   | Defect                                                                    | Where it came from |
   |---------------------------------------------------------------------------|--------------------|
   | `libc::O_NOFOLLOW` used with no `libc` dependency                          | phase 3's `ah-observability` extraction: the code moved, the root crate's dependency did not |
   | `REPLACE_RETRY_TIMEOUT` gated `#[cfg(windows)]` but used unconditionally    | phase 3's `ah-persist` extraction |
   | 35 dead-code and unused-import errors under `-D warnings` in `ah-update-helper`, 71 in `ah-updater` | both crates are Windows-only in substance; only their entry points were gated |
   | `FileLease` implemented for Windows only                                   | it always was - it is the *reason* the lifecycle tests could not run elsewhere |

   The last one is the substantive one. `ah_platform::lease` now has a Unix
   implementation: `flock` on a file, which the kernel releases when the
   descriptor closes - the same "owned by the holder, gone when it dies"
   property the Windows named mutex was chosen for, reached the other way
   round. Two consequences worth naming:

   - **Which of path and name is the identity now differs by platform.** On
     Windows the mutex name is (two paths naming one mutex are one lease), on
     Unix the path is. Every caller derives the name from the path, so the two
     agree in practice; the module documentation says so rather than leaving it
     to be discovered.
   - **Taking a lock file writes, and `status` must not.** A lease whose
     directory does not exist cannot be held, so `lock::is_free` answers
     without opening anything in that case. The read-only-status test caught
     this immediately, on the first Linux run - it had been asserting a real
     invariant that Windows could not violate.

   With those fixed, the 43 platform-neutral lifecycle tests pass on Linux, and
   `cargo clippy --workspace --all-targets -- -D warnings` and the MSRV check
   are clean there. What is left is three integration failures, recorded in
   group 08.
6. ~~Make `ah-mcp-service` (the supervisor binary) Windows-only *at the build
   level* (a target-gated `[[bin]]`)~~ **- not possible as written.** Cargo has
   no `[target.'cfg(windows)'.bin]`; a `[[bin]]` is built for every target.
   The two workarounds are worse than the stub: `required-features` would let
   an ordinary `cargo build` on Windows omit the worker and break
   `require_managed_service_executable` at install time, and a separate crate
   is still built by the workspace. The `main` that exits 1 stays, and this row
   is closed rather than carried.

## Risks and invariants

- **Windows behavior must not regress.** The task definition currently written is
  the product of several rounds of hardening, recorded in the Git history. The
  projection must produce a byte-identical task; assert with a snapshot of the
  generated definition before step 3.
- **Do not lose drift detection fidelity.** The current implementation compares 27
  fields deliberately; the neutral model must keep comparing all of them via the
  adapter, not silently narrow the check.
- **systemd/launchd introduce new failure modes** (user lingering, session buses,
  plist approval). Ship them as explicitly experimental until they have the same
  acceptance coverage as Windows.

## Acceptance criteria

- ~~`lifecycle::execute` contains no `cfg` attributes.~~ **(met)** Nor does
  anything else in `lifecycle/`.
- ~~A second scheduler adapter exists and passes the same lifecycle test
  suite.~~ **(met.)** `SystemdUserScheduler` is the second working adapter, and
  the suite runs against the host's own projection rather than a Windows-shaped
  fake - so on Linux CI it is the systemd identity, unit and comparison under
  test. `UnsupportedScheduler` remains the third, for platforms with no
  implementation.
- ~~One module resolves every AIHelper path; no plugin reads `XDG_*` or
  `APPDATA`.~~ **(met in phase 3.)**
- CI covers Linux, Windows and macOS. **The matrix has since phase 0; what was
  missing is that it passed.** Linux and Windows are now clean end to end -
  format, clippy `-D warnings`, MSRV, build and the whole suite, with the
  managed service exercised against a real systemd user manager. macOS is the
  one platform still taken on trust: it is Unix, so the lease and the lifecycle
  suite should behave as Linux does, and what is left unverified is its path
  canonicalisation, `keyring`'s `apple-native`, and - once
  `LaunchdScheduler` exists - launchd itself.
