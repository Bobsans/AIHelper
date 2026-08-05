# Managed MCP and Self-Update Completion Design

## Status

Approved in conversation on 2026-07-22. This design replaces line-by-line
roadmap execution with dependency-ordered vertical slices. It preserves the
existing managed MCP lifecycle, signed manifest, and signed release pipeline
decisions except for two explicitly superseded release policies: the updater
compatibility floor is no longer forced to equal the release version, and the
Windows archive gains the signed update helper when that binary is introduced.

The corresponding product requirements are recorded in
[`managed-mcp-self-update-v1.0-prd.md`](../prds/managed-mcp-self-update-v1.0-prd.md).

## Current Baseline

The managed HTTP MCP implementation already includes its Windows per-user Task
Scheduler integration, lifecycle lease, readiness identity, graceful shutdown,
bounded scheduler fallback, restart policy, deterministic status, and automated
component tests. Its remaining work is acceptance evidence:

- connect Claude Code, Codex, and OpenCode to one HTTP MCP instance;
- run the persistent Windows VM lifecycle matrix.

The release foundation already includes:

- strict signed manifest schema v1 and canonical Ed25519 verification;
- exact Windows x64, Linux x64, and macOS arm64 release profiles;
- a CI-only archive validator and release signer;
- a validation-only manual workflow and protected release sign/publish path.

Schema v1 already has the `update_helper` file purpose, but current release
profiles do not place a helper in any archive. The release tool also currently
sets `minimum_updater_version` equal to the release version. Both policies must
be corrected before the runtime updater consumes production manifests.

The main `ah` binary does not yet consume the manifest verifier and exposes no
`upgrade` command. No production trust anchor has been provisioned. Candidate
download, extraction, installation discovery, transaction execution, rollback,
and recovery remain unimplemented.

## Goals

- Finish the automated managed MCP implementation while keeping manual evidence
  visible and separate.
- Add a Windows-first self-updater that consumes stable GitHub Releases from
  `Bobsans/AIHelper`.
- Verify all release metadata and content before changing the installation.
- Preserve an arbitrary portable installation path and every user-owned file.
- Replace `ah.exe` and dynamic plugins only as one signed managed bundle.
- Use an external helper, durable state, verified backup, and automatic rollback
  so interruption never leaves an unrecoverable mixed installation.
- Restore a previously running managed MCP only after the active installation is
  verified.
- Keep deterministic text and versioned JSON diagnostics.

## Non-Goals

- Linux or macOS self-update in v1. Their release assets remain published, but
  `ah upgrade` returns `unsupported_platform` outside Windows x64.
- Background or automatic update installation.
- Prerelease channels, arbitrary downgrade, or configuration migrations.
- Moving an installation into a versioned directory or introducing a permanent
  launcher.
- Terminating third-party processes that hold installation files.
- Publishing lifecycle or upgrade operations as MCP tools.
- Treating deferred manual or live-release evidence as automated test coverage.

## Considered Approaches

### Shared updater core, thin orchestrator, autonomous helper

Use a platform-neutral internal core shared by the main process and a minimal
Windows helper. Deliver it through security-first vertical slices.

This is the chosen approach. Independent design reviews agreed that it gives the
best audit boundary, allows recovery to be tested before exposure through the
public mutating command, and prevents network or plugin loading from entering
the replacement process.

### Transaction engine before any user-facing command

Build the durable state machine and helper first, then add GitHub discovery and
trust verification. This addresses the hardest recovery risks early, but leaves
no complete user-visible trust path for a long period and risks changing the
transaction model when candidate verification is connected later.

### Monolithic updater inside the main `ah` process

This reduces initial package boundaries but couples critical recovery behavior
to CLI bootstrap and dynamic plugin loading. An external helper would still be
required to replace the running executable, causing late duplication of trust,
state, and error contracts. This approach is rejected.

## Package and Process Boundaries

### `ah-release-manifest`

Remains the only owner of the signed wire contract, canonical representation,
key identifiers, trust registry, and signature verification. It never accepts a
private key and performs no network or filesystem mutation.

### `ah-updater-core`

A new internal, non-publishable crate owns:

- GitHub release and asset models after bounded decoding;
- stable release selection and compatibility policy;
- installation classification and identity;
- verified candidate and installed-manifest models;
- exact managed-file update plans;
- durable transaction schema and state transitions;
- recovery decisions and deterministic updater error codes;
- ports used by tests and Windows adapters.

The core does not load plugins, own CLI parsing, or contain a private signing
key. Network and mutation enter through narrow adapters.

### Main `ah`

The main process owns:

- CLI parsing and text/JSON presentation;
- GitHub release discovery;
- bounded manifest, signature, and archive download;
- candidate staging, extraction, and offline smoke orchestration;
- managed MCP state capture;
- creation of an immutable transaction plan;
- copying and launching the helper outside the installation root;
- early pending-transaction detection before dynamic plugins are loaded.

### `ah-update-helper.exe`

The helper has no network client and never loads dynamic plugins. It owns:

- lock handoff acknowledgement;
- final re-verification of the transaction plan, candidate, and backup;
- Windows Restart Manager inspection;
- managed process quiescence;
- per-file replacement, installed verification, rollback, and recovery;
- managed MCP restoration after successful activation or rollback.

The helper may touch only the exact union of verified old and new managed paths.
It never performs recursive cleanup of the installation directory.

## Delivery Slices

### 0. Roadmap and managed MCP accounting

Restore previously removed completed items as checked entries. Keep manual and
external acceptance items open and labelled. Completed entries are no longer
deleted, so the roadmap remains an auditable progress record.

No new managed MCP behavior is planned unless the automated or later manual
matrix exposes a defect.

### 1. Production trust and `ah upgrade --check`

Provision one production Ed25519 keypair outside the repository. Store only the
32-byte seed in the protected `release-signing` GitHub environment; commit the
derived public key and key ID to the runtime trust registry. The registry
supports overlapping public keys for future rotation, but v1 has one active key.

The command lists public GitHub releases using the versioned REST API, rejects
drafts and prereleases, parses canonical `v<SemVer>` tags, and selects the
highest compatible stable version rather than trusting release creation order.
GitHub documents that release listing is public and that its latest endpoint is
ordered by `created_at`, so SemVer selection remains an AIHelper policy:

- <https://docs.github.com/en/rest/releases/releases>
- <https://docs.github.com/en/rest/releases/assets>

Discovery requests use the public API without a token, an explicit AIHelper
`User-Agent`, the recommended media type, and a pinned supported API version.
Pagination and response bytes are bounded. If the bound is reached while GitHub
still advertises another page, discovery fails instead of silently claiming the
observed subset is complete.

The resolver locates the exact manifest and signature asset names for
`ah-windows-x64.zip`. GitHub metadata is discovery input, not a trust anchor.
Every response is bounded; redirects are limited and every hop must remain
HTTPS. Asset downloads use the GitHub asset API, which may redirect or stream
the content directly.

The highest canonical stable release is authoritative for the check. Missing,
duplicate, or malformed sidecars on that release are a release-contract error;
the resolver never falls back to an older release. The selected release is
verified even when its version equals the running version, so an unsigned legacy
release cannot produce a trusted `up_to_date` result.

The detached signature is verified against the embedded registry before any
manifest URL, version, digest, or inventory field becomes trusted. The verified
manifest must match the release tag, Windows x64 profile, current updater
compatibility, and expected archive URL.

`minimum_updater_version` is the oldest released updater protocol that can
safely activate the bundle. The release tool must accept it as an explicit
release-policy input and require it to be no newer than the release version; it
must not derive it from the release version. The first release containing the
updater establishes the baseline. Later releases keep that baseline until an
incompatible updater feature genuinely requires raising it.

`--check` downloads no archive, writes no state, and does not acquire the
lifecycle lease or stop a process. It returns one of `up_to_date`,
`update_available`, `current_newer`, or a deterministic network,
release-contract, trust, or compatibility error. Unsupported platforms fail
before network access.

### 2. Verified candidate without installation mutation

Download the archive with response, file, and total byte limits. Verify its
signed size and SHA-256 before extraction. Extract into a newly created private
staging directory without following links or reparse points.

Reject absolute and traversal paths, backslashes in archive names, duplicates,
case collisions, Windows reserved names, alternate data streams, unsupported
entry types, links, reparse points, encrypted entries, missing or extra files,
and archive/file/expanded-size bombs. Reconcile every extracted path, size,
digest, and purpose against the verified manifest.

Run the staged executable in an isolated configuration environment with a
bounded timeout. The smoke operation must report the exact version, validate the
helper protocol, and load the built-in and staged dynamic plugin catalog without
reading the user's configuration or plugin directories.

Any failure removes only staging and leaves the installation and managed MCP
untouched.

### 3. Installation classification and bootstrap

Derive the installation root from the running executable without changing its
path. Persist a random installation identity in per-user application data and
bind it to the canonical executable path.

Detect `cargo install` layouts and refuse self-update with package-manager
guidance. A legacy portable installation becomes managed only when a signed
manifest for the current version exists and every claimed managed file matches
its signed hash. Modified or ambiguous legacy files stop adoption without
mutation.

### 4. Transaction core and external helper

Build and failure-test the helper against synthetic portable installations
before exposing the mutating public command. Create a verified complete backup
of all current managed files, installed manifest, and installation metadata
before the first write.

Add `ah-update-helper.exe` to the Windows release profile with the existing
`update_helper` purpose and include it in the required executable set. The
release workflow still publishes three archives and six sidecars, but the
Windows archive inventory and its manifest gain the helper. Candidate smoke
checks the helper protocol before it can be copied to transaction staging.

Write durable state before every irreversible boundary. Hand the existing
lifecycle/upgrade lock to the helper through an allowlisted inheritable Windows
handle. The parent exits only after helper acknowledgement, leaving no release
and reacquire gap.

The helper rehashes staging and backup immediately before mutation. Unknown file
hashes or foreign blockers abort safely. A crash in or after replacement always
enters deterministic recovery rather than guessing whether to continue.

### 5. Activation and managed MCP integration

Restart Manager identifies processes that hold the exact managed paths. The
updater cooperatively stops the proven managed MCP and gives other proven
AIHelper processes a grace period. It may force only AIHelper processes tied to
the same installation and still holding managed files. A persistent third-party
blocker aborts before mutation.

Replace only the exact update-plan paths, verify the permanent installation,
persist the exact canonical manifest and detached signature as the installed
release record, and run the installed smoke check.
Restore managed MCP only if it was previously running and wait for the new
version and instance identity.

### 6. Stable update, backup, and user rollback

Expose `ah upgrade`, `ah upgrade --version <VERSION>`, and one-shot
`ah upgrade --rollback`. Downgrade remains forbidden except restoration of the
single verified backup. Rotate the previous permanent backup only after the new
installation and any required MCP restart are verified.

### 7. Early recovery and acceptance

Every `ah` startup checks pending durable state before configuration and dynamic
plugin loading. It starts the external helper to finish rollback or a safe
idempotent recovery action.

Automated failure injection covers interruption before and after each durable
transition and each filesystem operation. Real GitHub release, target-client,
persistent Task Scheduler, logon, crash/restart, and Windows VM/reboot scenarios
remain explicit acceptance evidence.

## Durable and Security Invariants

- GitHub API metadata never replaces signature verification.
- The private release seed never enters the repository, runtime binary, argv,
  artifacts, logs, or updater state.
- Network and archive processing complete before any managed process is stopped.
- The helper never accesses the network or loads a plugin.
- Candidate and backup hashes are rechecked at the mutation boundary.
- The lifecycle/upgrade lock has no handoff gap.
- A complete verified backup exists before the first managed-file write.
- Only the union of verified old and new managed paths may be added, replaced,
  moved, or removed.
- User-owned files are never inferred from directory contents and never changed.
- A third-party file lock prevents mutation rather than authorizing termination.
- Durable recovery runs before dynamic plugin loading.
- Installed manifest persistence occurs only after permanent-file verification.
- Trust keys required to verify the active installation or retained rollback
  backup remain embedded during key rotation.

## Diagnostics

Updater text output remains concise and deterministic. JSON uses a new versioned
updater result contract and does not change existing plugin or MCP schemas.

The v1 success envelope contains `schema_version`, `operation`, `status`,
`current_version`, `selected_version`, `target`, and `source`. Optional values
are explicit JSON `null`, not omitted. `operation` is one of `check`, `upgrade`,
`version`, `rollback`, or `recovery`; check status is one of `up_to_date`,
`update_available`, or `current_newer`. Mutation results additionally report the
activation, managed MCP restoration, and rollback outcomes as separate fields.
Once released, these field names are compatibility-controlled.

Errors are categorized as argument, unsupported platform, network, release
contract, trust, compatibility, candidate, installation, lock, blocker,
transaction, activation, rollback, and recovery failures. Diagnostics never
include response bodies, secret material, detached signatures, or untrusted
paths that failed normalization.

When automatic rollback succeeds, the command still reports the update failure
and separately reports that the previous version was restored. Failed rollback
retains backup and durable state and prints an exact recovery action.

## Automated Verification

- Unit tests for release selection, trust, compatibility, installation models,
  update plans, state transitions, and deterministic diagnostics.
- Mock HTTP tests for pagination, redirects, timeouts, partial and oversized
  responses, duplicate assets, and release policy.
- Hostile ZIP tests for every path, link, collision, type, and size rule.
- Candidate smoke tests for executable version, helper protocol, plugin ABI, and
  catalog mismatch.
- Synthetic-installation tests for add, replace, remove, user-file preservation,
  legacy adoption, and `cargo install` refusal.
- Failure injection before and after every durable state and file operation.
- Windows adapter tests for Restart Manager and inherited lock handoff.
- CLI integration tests for all text/JSON success and failure paths.
- Workspace format, test, debug build, and release build checks after applicable
  milestones.

## Completion Accounting

Engineering completion means all code slices, automated checks, documentation,
and roadmap entries are complete. External completion evidence remains visible
until the production key is provisioned, a real signed release is consumed, the
three target clients connect, and the Windows VM matrices pass.

As of 2026-08-05 the automated engineering scope is complete. The production
key, live signed-release consumption, target-client connectivity, persistent
Task Scheduler matrix, and updater interruption/reboot matrix remain open as
external evidence.

Each implementation slice uses the established workflow: implement, run the
smallest relevant checks and applicable workspace checks, mark rather than
delete completed roadmap entries, commit only the intended files, and record the
milestone in Basic Memory.
