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
3. Introduce `ServiceSpec` + `ServiceScheduler`; reimplement `WindowsTaskScheduler`
   as a projection. The lifecycle tests already use fake schedulers, so this step is
   well covered from day one.
4. Remove the `cfg(windows)` gates from `lifecycle::execute` and the update helpers;
   route the unsupported case through `UnsupportedScheduler` so the error text stays
   identical on non-Windows.
5. Add `SystemdUserScheduler`, then `LaunchdScheduler`; add `macos-latest` to CI.
6. Make `ah-mcp-service` (the supervisor binary) Windows-only *at the build level*
   (a target-gated `[[bin]]`), rather than compiling a stub that exits 1.

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

- `lifecycle::execute` contains no `cfg` attributes.
- A second scheduler adapter exists and passes the same lifecycle test suite.
- One module resolves every AIHelper path; no plugin reads `XDG_*` or `APPDATA`.
- CI covers Linux, Windows and macOS.
