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
acknowledges. A successful launch reports `status=activation_launched`; final
activation then runs out of process. The helper replaces only signed managed
paths, verifies permanent hashes and the installed version, commits the exact
signed installed-release record, and automatically rolls back post-mutation
failures.

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

Production key activation is tracked separately. Until the protected signing
key is provisioned and its public key is embedded, source builds fail closed with
`UPDATER_TRUST` before network access.

## Acceptance boundary

Automated updater implementation and tests are complete. Production acceptance
still requires provisioning the protected key, consuming a real signed release
with a released updater, and passing the persistent Windows scheduler and
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

Errors use deterministic `UPDATER_<CATEGORY>` diagnostic codes. Categories
include `UNSUPPORTED_PLATFORM`, `NETWORK`, `RELEASE_CONTRACT`, `TRUST`,
`COMPATIBILITY`, `CANDIDATE`, `INSTALLATION`, `ACTIVATION`, `ROLLBACK`, and
`RECOVERY`. Diagnostics do not include response bodies, signatures, or untrusted
key IDs.

## Network limits

Release discovery uses the GitHub API media type and API-version headers, a
15-second request timeout, at most five pages of 100 releases, at most 500
releases, and at most 8 MiB per release-list response. Listing redirects are
rejected. Asset redirects are limited to three hops and approved HTTPS GitHub
download hosts.
