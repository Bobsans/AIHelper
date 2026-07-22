# `ah upgrade`

The built-in `upgrade` command checks and, in later slices, installs trusted
AIHelper releases. It is routed before configuration and dynamic plugin loading.

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

Production key activation is tracked separately. Until the protected signing
key is provisioned and its public key is embedded, source builds fail closed with
`UPDATER_TRUST` before network access.

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

Errors use deterministic `UPDATER_<CATEGORY>` diagnostic codes. Categories
include `UNSUPPORTED_PLATFORM`, `NETWORK`, `RELEASE_CONTRACT`, `TRUST`, and
`COMPATIBILITY`. Diagnostics do not include response bodies, signatures, or
untrusted key IDs.

## Network limits

Release discovery uses the GitHub API media type and API-version headers, a
15-second request timeout, at most five pages of 100 releases, at most 500
releases, and at most 8 MiB per release-list response. Listing redirects are
rejected. Asset redirects are limited to three hops and approved HTTPS GitHub
download hosts.
