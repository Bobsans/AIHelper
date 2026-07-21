# Managed HTTP MCP and Self-Update Roadmap

## Status

Planned. Implementation starts after the HTTP MCP transport is complete and
its foreground lifecycle is stable.

## Objective

Make the HTTP MCP server usable as a reliable background process and allow
AIHelper to update itself without leaving a partially updated executable and
plugin set.

The first implementation targets the currently supported Windows releases:
Windows 10, Windows Server 2016, and newer. It installs for the current user,
does not require administrator privileges, and starts after that user logs in.

## Scope

The roadmap has two related deliverables:

1. Managed HTTP MCP lifecycle through `ah mcp install`, `start`, `stop`,
   `restart`, `status`, and `uninstall`.
2. Safe self-update through `ah upgrade`, including package verification,
   service coordination, health validation, and automatic rollback.

The stdio MCP transport remains client-managed. The lifecycle commands in this
document apply to the long-running HTTP transport.

## Goals

- Install HTTP MCP autostart for the current Windows user without elevation.
- Restart the MCP process after unexpected failures without creating a rapid
  crash loop.
- Make all lifecycle commands deterministic, idempotent, and safe under
  concurrent invocation.
- Update `ah.exe` and its dynamic plugins as one compatible release bundle.
- Never activate an unverified or incomplete download.
- Preserve the MCP state across an upgrade: restart it only when it was running
  before the upgrade.
- Automatically restore the previous working version when the new version does
  not become ready.
- Recover safely when installation or upgrade is interrupted.

## Non-Goals for the First Version

- A machine-wide Windows Service that runs before user login.
- Linux systemd user services or macOS LaunchAgents.
- Multiple independently configured MCP instances for one user.
- Automatic unattended updates.
- Updating stdio MCP processes launched and owned by external clients.
- A custom always-running supervisor unless Task Scheduler proves insufficient.

## Command Surface

### `ah mcp install`

Register the managed HTTP MCP for the current user.

Expected behavior:

- Create or reconcile a per-user Task Scheduler task.
- Configure startup when the current user logs in.
- Run with least privilege and without storing the user's password.
- Configure bounded restart-on-failure behavior.
- Use a stable working directory and explicit configuration paths.
- Default to a loopback listener.
- Refuse to create a second managed instance.
- Be idempotent: a matching installation is a no-op, while a changed supported
  configuration updates the existing task.

Per-user installation intentionally means that MCP is unavailable before the
user logs in. A future `--system` mode may use a Windows Service when pre-login
availability is required.

### `ah mcp start`

Start the installed MCP instance when it is not already running. Success means
that the server reaches readiness, not merely that a process was created.

### `ah mcp stop`

Stop the current MCP instance without removing its autostart registration.
The command should request graceful shutdown, wait for active work to drain up
to a bounded timeout, and use a forced Task Scheduler stop only as a fallback.

### `ah mcp restart`

Perform a coordinated stop and start, then wait for the new instance to become
ready.

### `ah mcp status`

Report separate installation and runtime states. At minimum, status should
distinguish:

- not installed;
- installed but stopped;
- starting;
- running but not ready;
- ready;
- failed or in a restart loop.

Text and JSON output should include the active version, process ID when known,
listener address, readiness result, and the last useful scheduler error.

### `ah mcp uninstall`

Stop the managed instance and remove its Task Scheduler registration. Removing
downloaded versions and user configuration should require separate explicit
cleanup behavior rather than happen implicitly.

### `ah upgrade`

Upgrade to the newest eligible stable release. Planned options include:

- `ah upgrade --check` to report availability without changing files;
- `ah upgrade --version <VERSION>` to select a specific trusted version;
- a future channel option if prerelease or long-term support channels are
  introduced.

An explicit downgrade must not happen accidentally. If downgrade support is
added, it should require a dedicated flag.

## Recommended Architecture

### Per-User Task Scheduler Integration

Use the Windows Task Scheduler 2.0 API rather than parsing localized
`schtasks.exe` output. The task should run for the current user with least
privilege and an interactive logon token.

The task definition should:

- trigger at user logon;
- prevent parallel instances;
- have no execution time limit;
- start when available after a missed trigger;
- remain allowed during normal battery operation unless a later product policy
  decides otherwise;
- restart after unexpected failure with bounded attempts and delay between
  attempts.

Lifecycle and upgrade operations must share a per-user lock so that `start`,
`stop`, `install`, `uninstall`, and `upgrade` cannot race each other.

### Stable Launcher and Versioned Installation

Do not replace the active `ah.exe` or plugin DLLs in place. Windows may lock
loaded executables and libraries, and replacing only part of a release can
produce an ABI-incompatible installation.

Use immutable, side-by-side version directories under a managed local data
location, for example:

```text
%LOCALAPPDATA%\AIHelper\
  launcher\
  versions\
    1.2.0\
      ah.exe
      plugins\
    1.3.0\
      ah.exe
      plugins\
  current.json
  update-state.json
```

User configuration remains separate from binaries, under the existing
configuration location. The scheduled task invokes a stable launcher, and the
launcher resolves the active release from an atomically replaced pointer such
as `current.json`.

Each version directory is prepared completely before activation. Keep at least
one previously working version to support rollback. Old versions are eligible
for cleanup only after the new release has passed readiness validation.

The launcher should remain small and stable. Updating the launcher itself is a
separate concern and may require a short-lived helper process in a later phase.

### HTTP Lifecycle Contract

The HTTP MCP must expose enough local lifecycle information for reliable
management:

- a readiness endpoint that returns success only after the listener, plugin
  catalog, and executor are ready;
- a graceful shutdown path that stops accepting work and drains active requests;
- active version and unique instance identity in readiness output;
- a non-zero process exit code for fatal startup and runtime failures;
- explicit listener and working-directory configuration;
- loopback binding by default.

The instance identity prevents lifecycle commands from mistaking an unrelated
process on the same port for the managed MCP instance.

Local HTTP access still needs an authentication and browser-origin policy.
Loopback binding alone does not protect against other local processes, browser
requests, or DNS rebinding.

## Release Metadata and Package Verification

Self-update requires machine-readable release metadata. The release pipeline
should publish a manifest containing at least:

- release version and channel;
- supported target triple or architecture;
- archive URL and expected size;
- SHA-256 digest;
- minimum compatible updater version when needed.

The manifest should be signed by a release key whose public key is embedded in
AIHelper. HTTPS and a checksum downloaded from the same compromised source do
not independently authenticate a release. Platform signature verification,
such as Authenticode on Windows, may be added as an additional check.

Archive extraction must reject:

- path traversal and absolute paths;
- symbolic links or other unsupported entry types;
- duplicate or case-colliding paths;
- unexpected executable layout;
- incorrect architecture;
- unreasonable compressed or extracted sizes;
- missing required files.

The downloaded release must be validated and smoke-tested from staging before
the active MCP is stopped.

## Upgrade Transaction

`ah upgrade` should execute the following transaction:

1. Acquire the per-user lifecycle and upgrade lock.
2. Read the installed version and current MCP installation, running, and
   readiness states.
3. Resolve the requested release and download it into staging.
4. Verify the signed manifest, digest, size, architecture, and archive layout.
5. Extract into a new immutable version directory on the same volume as the
   active installation.
6. Run an offline smoke check against the candidate binary and plugin bundle.
7. Record a durable update transaction before changing runtime state.
8. Gracefully stop MCP only if it is currently running.
9. Atomically switch the active-version pointer.
10. Start MCP only if it was running before the upgrade.
11. Wait for readiness from the expected version and a new instance identity.
12. Mark the transaction successful and retain the prior version for rollback.

If activation, startup, or readiness validation fails, restore the previous
pointer and restart the previous version when it had been running. A failed
upgrade must return an error even when rollback succeeds, while clearly
reporting that service was restored.

If the process or machine stops during the transaction, the next lifecycle or
upgrade command should inspect the durable transaction state and complete a
safe rollback or activation. Recovery must choose one complete release bundle;
it must never combine files from different versions.

## Delivery Phases

### Phase 1: Stabilize HTTP MCP Foreground Operation

- Complete the HTTP transport.
- Add loopback-safe defaults and explicit configuration.
- Add readiness, instance identity, and graceful shutdown.
- Return meaningful exit codes for fatal failures.
- Define authentication and browser-origin behavior.

Completion criteria:

- The server can be started, probed, drained, and stopped deterministically.
- A port conflict or invalid configuration fails quickly and visibly.
- Readiness identifies the exact running version and instance.

### Phase 2: Managed MCP Lifecycle

- Add the Task Scheduler adapter.
- Implement `install`, `start`, `stop`, `restart`, `status`, and `uninstall`.
- Add per-user locking and single-instance enforcement.
- Add restart-on-failure policy and useful diagnostics.

Completion criteria:

- A normal user installs MCP without elevation or a stored password.
- MCP starts once after login and is restarted after a bounded unexpected
  failure.
- Repeated lifecycle commands are safe and idempotent.
- `status` distinguishes process execution from HTTP readiness.

### Phase 3: Managed Version Layout and Release Metadata

- Introduce the stable launcher and immutable version directories.
- Publish release manifests, checksums, and signatures.
- Implement secure download, extraction, and candidate smoke checks.
- Define migration from an unmanaged archive or Cargo installation.

Completion criteria:

- A complete candidate bundle can be installed alongside the active version.
- The active version can be switched without replacing loaded files.
- Invalid, corrupted, or malicious packages cannot reach activation.

### Phase 4: Self-Update and Rollback

- Implement `ah upgrade` and `ah upgrade --check`.
- Preserve pre-upgrade MCP state.
- Add atomic activation, readiness validation, rollback, and durable recovery.
- Retain and clean old versions according to a bounded policy.

Completion criteria:

- Successful upgrade activates the complete new release and restores prior MCP
  running state.
- Network, verification, extraction, startup, and readiness failures leave the
  previous version usable.
- Interrupted updates recover deterministically.

### Phase 5: Platform Expansion

After the Windows per-user path is proven:

- evaluate `ah mcp install --system` using a Windows Service;
- add a systemd user service for Linux;
- add a LaunchAgent for macOS;
- consider a custom supervisor only if native managers cannot meet operational
  requirements.

## Validation Matrix

Automated and VM-level validation should cover:

- Windows 10 and Windows Server 2016;
- install, reinstall, start, repeated start, stop, restart, status, and
  uninstall;
- user logoff and login;
- process crash, bounded restart, and crash-loop behavior;
- concurrent lifecycle and upgrade commands;
- port conflicts and slow or failed readiness;
- active requests during graceful shutdown;
- network interruption and partial downloads;
- invalid signature, checksum, architecture, and archive layout;
- locked binaries and plugin DLLs;
- antivirus-induced delays during download, extraction, and startup;
- failure before and after every durable upgrade phase;
- failed new-version startup followed by successful rollback;
- failed rollback with actionable diagnostics;
- preservation of an intentionally stopped MCP state across upgrade.

## Important Design Constraints

- `ah.exe` and dynamic plugins form one release bundle and must be activated
  together.
- Lifecycle and self-update commands should use an early runtime path that does
  not require loading plugin DLLs from a version that is being changed.
- Lifecycle and upgrade commands should not be exposed as MCP tools by default:
  a synchronous MCP call must not terminate or replace the server handling it.
- Task Scheduler process state is not equivalent to application readiness.
- Process discovery must use managed identity and state, not broad process-name
  matching or termination.
- Text and JSON output remain deterministic and follow existing AIHelper error
  conventions.

## Future Decisions

The following choices should be finalized during implementation design:

- exact HTTP MCP authentication and local control channel;
- exact managed installation and configuration paths;
- launcher packaging and rare launcher-update procedure;
- release manifest format and signing-key rotation;
- stable release source and optional update channels;
- graceful shutdown timeout and restart backoff policy;
- retention count and disk cleanup policy;
- migration behavior for existing manually installed copies.
