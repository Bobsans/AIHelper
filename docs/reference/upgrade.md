# `ah upgrade`

The built-in `upgrade` command checks and installs trusted AIHelper releases. It
is routed before configuration and dynamic plugin loading.

## Read-only check

```text
ah upgrade --check
ah upgrade --check --json
ah upgrade --check --quiet
```

`--check` is supported on Windows x64. Unsupported platforms fail before any
network access. The command:

1. requires a non-empty embedded production public-key registry;
2. lists bounded pages from the public `Bobsans/AIHelper` GitHub Releases API;
3. selects the highest canonical stable SemVer without trusting API order;
4. requires the exact Windows archive, manifest, and signature assets;
5. downloads only the bounded manifest and detached signature;
6. verifies canonical manifest bytes and the Ed25519 signature before using
   manifest fields;
7. checks the release tag, target, architecture, archive URL and size, and
   `minimum_updater_version`;
8. reports `up_to_date`, `update_available`, or `current_newer`.

The check does not download the release archive, write updater or installation
state, acquire lifecycle locks, or stop managed MCP processes.

## Install an update

```text
ah upgrade
ah upgrade --version 1.2.0
ah upgrade --rollback
ah upgrade --json
```

The default selects the highest stable signed release. `--version` selects one
exact canonical stable SemVer and refuses downgrade. Both paths finish release
discovery, archive verification, extraction, offline smoke, transaction staging,
and verified backup before stopping a process.

On Windows x64 the command copies the verified helper outside the installation,
hands it the lifecycle lock without a gap, and exits after the helper
acknowledges. A successful JSON launch reports `status=activation_launched`; final
activation then runs out of process. The helper replaces only signed managed
paths, verifies permanent hashes and the installed version, commits the exact
signed installed-release record, and automatically rolls back post-mutation
failures.

Text output reports each stage as it starts: checking the release, downloading,
verifying, testing the new version, preparing the installation,
stopping a previously running MCP service, and starting the installer. In a
terminal the active row animates; finished stages remain marked with `✓` and a
failed stage with `✗`, followed by the normal error diagnostic. Redirected output
contains sequential lines without animation or terminal control codes.

After helper acknowledgement, the final message says `Update started` and
`Installation will finish in the background.` It mentions the automatic MCP
restart only when restoration is pending. This is not a claim that activation
has already completed. Rollback uses the same display with backup verification
and no download stages. `--check` reports a readable availability result.
`--json` retains the existing structured result without progress lines;
`--quiet` suppresses progress and the final result. Successful checks keep only
the availability result; an already installed exact version is reported as such.

Colors follow the shared terminal formatter: green for success, cyan for
versions and the active spinner, yellow for warnings, and red for errors.
`NO_COLOR` disables colors. Redirected output and JSON have no ANSI styling.

Examples (versions are illustrative):

```text
✓ AIHelper 2.0.1 is up to date.
```

```text
↑ AIHelper update available: 2.0.1 → 2.1.0

Run ah upgrade to install it.
```

```text
AIHelper update · 2.0.1 → 2.1.0

  ✓ Checking for updates
  ✓ Downloading update
  ✓ Verifying update
  ✓ Testing the new version
  ✓ Preparing installation
  ✓ Starting the installer

✓ Update started: 2.0.1 → 2.1.0

Installation will finish in the background.
```

The MCP stop stage and automatic restart note appear when MCP was running.
Network, trust verification, missing-backup and downgrade errors use readable
summaries and applicable retry instructions; JSON keeps their original codes
and diagnostic details.

If managed MCP was ready before the update, its previous instance identity is
bound into the durable transaction. After activation or rollback, the helper
starts the active `ah`, waits for readiness, and requires the active release
version and a different instance identity.

## Roll back once

`ah upgrade --rollback` performs no network access. It loads the single signed
backup from `%APPDATA%\AIHelper\backup\<INSTALLATION_ID>`, verifies every managed
file, and prepares a new durable transaction before stopping managed MCP. The
currently installed release becomes that transaction's safety backup, so an
interrupted or failed rollback deterministically returns to the pre-rollback
version.

The permanent backup is consumed only after the restored release passes its
installed smoke check and any previously running managed MCP returns ready. A
failed rollback retains the permanent backup. A later successful update rotates
it only after activation and service restoration succeed.

Release builds must retain every public trust anchor needed by either the active
installation or its retained backup until that backup is rotated or consumed.

Production builds embed the public trust anchor matching the protected
`release-signing` environment. The signing seed remains external to the source
tree and runtime binaries.

## Acceptance boundary

Automated updater implementation, tests, and production key activation are
complete. Production acceptance still requires consuming a real signed release
with a released updater and passing the persistent Windows scheduler and
update/rollback/interruption/reboot VM matrices. Target-client MCP connectivity
is tracked separately with the managed service acceptance.

## JSON result

Successful JSON uses schema version 1 and keeps optional fields explicit:

```json
{
  "schema_version": 1,
  "operation": "check",
  "status": "update_available",
  "current_version": "1.1.0",
  "selected_version": "1.2.0",
  "target": "x86_64-pc-windows-msvc",
  "source": "github_release"
}
```

Mutation launch results additionally contain `activation`,
`managed_mcp_restoration`, and `rollback`. `managed_mcp_restoration` is `pending`
only when a previously ready managed MCP must be restored by the helper.
Rollback results use `operation=rollback`, `source=permanent_backup`, and
`rollback=launched`.

## When recovery consumes the command

An interrupted update leaves a journalled transaction behind, and the next `ah`
of any kind finishes it before running what you typed. When that happens the
command you asked for **does not run**, and the invocation reports why:

```json
{
  "command": "update.recovery",
  "consumed_invocation": true,
  "message": "update recovery started; rerun the command after recovery completes",
  "transaction_id": "0f1c...",
  "operation": "upgrade",
  "state": "commit_started"
}
```

`consumed_invocation` is the field to check: it distinguishes an invocation that
ran and printed nothing from one that never ran. `state` is where the interrupted
update was when recovery found it. Without `--json` the same event is a warning
block on standard error:

```text
! An interrupted update was detected.

Recovery has started.
Run your command again after recovery finishes.
```

The exit code is 0 either way: the recovery helper was launched, and your
command has to be repeated after recovery finishes.

Either way one `system` record is written to the event log, severity `warning`,
diagnostic code `UPDATE_RECOVERY_CONSUMED_INVOCATION`, so an invocation that did
something other than what was asked is auditable after the fact. See
[logging](logging.md).

Errors use deterministic `UPDATER_<CATEGORY>` diagnostic codes. Categories
include `UNSUPPORTED_PLATFORM`, `NETWORK`, `RELEASE_CONTRACT`, `TRUST`,
`COMPATIBILITY`, `CANDIDATE`, `INSTALLATION`, `ACTIVATION`, `ROLLBACK`, and
`RECOVERY`. Diagnostics do not include response bodies, signatures, or untrusted
key IDs.

On Windows, replacing journals, managed files and release records supports
paths longer than `MAX_PATH` through the extended-length namespace. This does
not require changing the system's long-path registry setting and retains
write-through replacement.

## Network limits

Release discovery uses the GitHub API media type and API-version headers, a
15-second request timeout, at most five pages of 100 releases, at most 500
releases, and at most 8 MiB per release-list response. Listing redirects are
rejected. Asset redirects are limited to three hops and approved HTTPS GitHub
download hosts.
