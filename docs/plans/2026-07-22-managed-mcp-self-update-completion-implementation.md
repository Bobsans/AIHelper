# Managed MCP and Self-Update Completion Implementation Plan

## Goal

Complete the approved Windows-first managed MCP and self-update design through
dependency-ordered, security-first vertical slices. Each task ends with focused
tests, applicable workspace checks, an auditable roadmap update, and a focused
commit.

## Governing Documents

- `docs/plans/2026-07-22-managed-mcp-self-update-completion-design.md`
- `docs/prds/managed-mcp-self-update-v1.0-prd.md`
- `docs/decisions/2026-07-22-adopt-canonical-ed25519-release-manifest-v1.md`
- `roadmap/managed-mcp-and-self-update.md`

## Global Rules

- Preserve the user-owned `.agents/skills/release/SKILL.md` change.
- Mark completed roadmap entries with `[x]`; do not delete completion history.
- Keep manual client, persistent scheduler, live release, and Windows VM evidence
  as separately labelled open acceptance items until actually executed.
- Add success and failure tests for every behavior change.
- Keep existing CLI, MCP, plugin ABI, and released JSON contracts compatible.
- Do not expose mutation through the public CLI until transaction failure
  injection passes.
- Never place the private signing seed in source, argv, files, logs, artifacts,
  Basic Memory, or commit history.

## Task 0: Repair roadmap accounting

Affected files:

- `roadmap/managed-mcp-and-self-update.md`

Steps:

1. Restore all previously removed completed Stage 2 implementation entries as
   checked items using their original wording and Git history as evidence.
2. Restore the three completed signed-manifest entries as checked items.
3. Restore release-pipeline publication as implemented but leave production key
   provisioning and a live signed-release run open.
4. Split automated completion from manual/external acceptance without weakening
   the existing criteria.
5. Replace the stale top-level status with the actual baseline.
6. Reorder remaining self-update tasks into the approved vertical slices while
   preserving every original requirement and acceptance criterion.
7. Verify the resulting open/completed counts against Git history and current
   code, run `git diff --check`, and commit only the roadmap.

Expected commit: `docs: repair managed updater roadmap`

## Task 1: Correct release compatibility policy

Affected files:

- `crates/ah-release-tool/src/main.rs`
- `crates/ah-release-tool/src/release.rs`
- `crates/ah-release-tool/src/error.rs`
- release-tool tests
- `.github/workflows/release.yml`
- relevant release documentation

Steps:

1. Replace the derived `minimum_updater_version = release.version` behavior with
   an explicit canonical SemVer policy input.
2. Require the compatibility floor to be no newer than the release version.
3. Keep the current release version as a deliberate workflow input only until
   the first updater baseline version is chosen; future releases may retain an
   older compatible floor.
4. Add malformed, newer-than-release, and preserved-older-floor tests.
5. Update workflow contract tests and release documentation.
6. Run focused manifest/release-tool tests, formatting, and diff checks.

Expected commit: `fix: model updater compatibility floor`

## Task 2: Provision production trust and add updater-core foundation

Affected files:

- `Cargo.toml`
- `Cargo.lock`
- `crates/ah-updater-core/Cargo.toml`
- `crates/ah-updater-core/src/lib.rs`
- `crates/ah-updater-core/src/error.rs`
- `crates/ah-updater-core/src/release.rs`
- `crates/ah-updater-core/src/trust.rs`
- root dependency wiring
- protected GitHub `release-signing` environment configuration

Steps:

1. Perform one production Ed25519 key ceremony without persisting or printing
   the seed; provision it directly into the protected GitHub environment.
2. Commit only the derived public key and key ID in a typed default trust
   registry with an explicit rotation list.
3. Add the unpublished updater-core crate depending on `ah-release-manifest`.
4. Define versioned release-discovery DTOs, updater result/error categories, and
   target compatibility types without network or mutation implementations.
5. Add tests proving the embedded key ID, injected test registries, unknown-key
   rejection, and production-source absence of private material.
6. Run focused tests, `cargo metadata --locked`, dependency-tree checks, and
   secret-leak searches.

Expected commit: `feat: establish updater trust anchor`

## Task 3: Implement bounded GitHub stable-release discovery

Affected files:

- `crates/ah-updater-core/src/release.rs`
- `crates/ah-updater-core/src/http.rs`
- `crates/ah-updater-core/src/policy.rs`
- updater-core HTTP and policy tests

Steps:

1. Add a narrow HTTP port and production reqwest/rustls adapter for the public
   `Bobsans/AIHelper` GitHub Releases API.
2. Set explicit media type, API version, and AIHelper `User-Agent` headers.
3. Bound response bytes, pagination, redirects, timeouts, and release count;
   fail closed when completeness cannot be proven.
4. Reject drafts, prereleases, malformed or non-canonical tags, duplicate asset
   names, and unexpected asset states.
5. Select the highest canonical compatible SemVer, independent of GitHub
   creation order, and never fall back from a malformed highest release.
6. Locate only the exact Windows archive manifest and signature sidecars.
7. Cover ordering, pagination, rate limiting, redirect, timeout, partial body,
   oversized body, missing/duplicate asset, and no-release paths.

Expected commit: `feat: discover stable updater releases`

## Task 4: Deliver mutation-free `ah upgrade --check`

Affected files:

- `src/cli.rs`
- `src/runtime_flow.rs`
- new root updater orchestration/output modules
- `crates/ah-updater-core` verification and result modules
- CLI integration tests
- command reference and AI recipes

Steps:

1. Add `RuntimeCommand::Upgrade` routing early enough to avoid loading dynamic
   plugins for unsupported or recovery-sensitive updater operations.
2. Implement `ah upgrade --check` and reject incompatible flag combinations.
3. Fail unsupported platforms before network access.
4. Download bounded exact manifest/signature assets through the GitHub asset API
   with HTTPS-only bounded redirects.
5. Verify canonical manifest bytes and signature before trusting any manifest
   field, then enforce tag, target, architecture, archive URL, version, and
   minimum-updater compatibility.
6. Return `up_to_date`, `update_available`, or `current_newer` without writing,
   acquiring lifecycle locks, or stopping processes.
7. Implement and document the versioned deterministic text/JSON check envelope.
8. Prove no-mutation behavior with filesystem and lifecycle spies.
9. Run focused tests, root integration tests, formatting, and debug build.

Expected commit: `feat: add trusted upgrade check`

## Task 5: Download and safely extract a verified candidate

Affected files:

- updater-core candidate/archive modules
- root staging/download adapters
- hostile archive fixtures and tests

Steps:

1. Stream the archive to a private temporary file with hard and signed size
   limits and incremental SHA-256 verification.
2. Verify archive size and digest before extraction.
3. Preflight every ZIP entry before the first output file is created.
4. Reject absolute/traversal paths, backslashes, duplicates, ASCII case
   collisions, reserved Windows names, ADS, links, reparse points, encryption,
   unsupported types, missing/extra files, and size bombs.
5. Extract without following filesystem links and reconcile every normalized
   path, purpose, size, and digest with the verified manifest.
6. Clean only private staging on failure and prove installation/service
   immutability.
7. Run hostile archive tests, focused candidate tests, and formatting.

Expected commit: `feat: prepare verified update candidates`

## Task 6: Add isolated candidate smoke and installation classification

Affected files:

- updater-core installation and smoke contracts
- root smoke-process adapter
- CLI/internal self-check routing
- installation and process integration tests

Steps:

1. Add a side-effect-free internal self-check reporting executable version,
   helper protocol, plugin ABI, and staged plugin catalog.
2. Run it with isolated configuration, plugin, APPDATA, and timeout settings.
3. Derive the canonical installation root from the running executable.
4. Persist and validate per-user installation identity outside the installation.
5. Detect cargo-managed layouts and return package-manager guidance without
   mutation.
6. Implement legacy portable adoption only when a signed current-version
   manifest exists and all claimed hashes match.
7. Cover Unicode/space paths, modified legacy files, ambiguous identity,
   incompatible plugins, timeout, and configuration-isolation failures.

Expected commit: `feat: classify and smoke update installations`

## Task 7: Package the signed Windows update helper

Affected files:

- new `crates/ah-update-helper` package
- release profile and archive validation code
- manifest/release-set fixtures and tests
- `.github/workflows/release.yml`
- release documentation

Steps:

1. Add a minimal unpublished Windows helper binary without network, plugin, or
   general CLI dependencies.
2. Define a versioned helper protocol and a side-effect-free self-check.
3. Add `ah-update-helper.exe` to the Windows archive with the existing
   `FilePurpose::UpdateHelper` and required executable inventory.
4. Keep Linux/macOS profiles unchanged and keep the nine published assets
   unchanged while updating the Windows ZIP contents and manifest.
5. Add archive, manifest, helper-protocol, and workflow contract tests.
6. Prove the helper dependency tree contains no HTTP client or runtime plugin
   loader.

Expected commit: `feat: package signed update helper`

## Task 8: Implement durable transaction and recovery core

Affected files:

- updater-core plan, journal, backup, state-machine, and recovery modules
- helper transaction executor modules
- synthetic installation harness and failure-injection tests

Steps:

1. Define versioned immutable transaction plan and durable journal schemas.
2. Compute exact add/replace/remove operations from verified old/new manifests.
3. Create and verify a complete transaction backup before the first write.
4. Persist canonical manifest and detached signature bytes with active and backup
   release records.
5. Write-ahead every durable state and fsync required files/directories.
6. Rehash candidate and backup at the helper mutation boundary.
7. Implement idempotent replacement, rollback, and conservative recovery for
   unknown hashes.
8. Inject failures before and after every durable transition and file operation,
   including process termination and restart of the harness.
9. Do not expose mutation through the root CLI until this matrix passes.

Expected commit: `feat: add durable update transactions`

## Task 9: Add Windows blocker detection and gapless lock handoff

Affected files:

- updater-core process/lifecycle ports
- root lifecycle capture and helper-launch adapters
- helper Windows Restart Manager and process modules
- Windows adapter/component tests

Steps:

1. Extend the existing per-user lifecycle lease for upgrade/rollback operations.
2. Transfer one allowlisted inheritable Windows handle to the helper and require
   acknowledgement before the parent releases its copy.
3. Use Restart Manager to identify processes holding only exact managed paths.
4. Classify proven same-installation AIHelper processes separately from foreign
   blockers.
5. Abort before mutation while a foreign blocker remains.
6. Cooperatively stop the exact managed MCP, apply a bounded grace period to
   other proven AIHelper blockers, and force only those still holding files.
7. Cover lock race attempts, stale identity, foreign blockers, grace expiry, and
   helper-start failure.

Expected commit: `feat: coordinate updater process handoff`

## Task 10: Expose activation and stable update

Affected files:

- root updater orchestration and CLI output
- helper activation executor
- installed release record store
- managed MCP integration
- references, recipes, and integration tests

Steps:

1. Implement `ah upgrade` and `ah upgrade --version <VERSION>` without downgrade.
2. Complete all network, verification, extraction, smoke, and backup work before
   stopping a process.
3. Launch the helper, activate exact managed paths, and verify permanent hashes.
4. Run the installed smoke check and persist exact signed installed-release
   bytes only after permanent verification.
5. Restore managed MCP only when previously running and require new version and
   instance identity readiness.
6. Report activation, service restoration, and automatic rollback independently
   in deterministic text/JSON.
7. Cover success, no-op, explicit version, activation failure, rollback success,
   rollback failure, stopped-service, and running-service paths.

Expected commit: `feat: activate trusted self updates`

## Task 11: Add permanent backup, user rollback, and early recovery

Affected files:

- updater-core backup/recovery policy
- helper rollback/recovery executor
- root pre-plugin startup routing and CLI
- recovery and rollback integration tests
- command documentation

Steps:

1. Rotate one permanent backup only after activation and service restoration
   succeed; retain the older backup after a failed update and rollback.
2. Implement one-shot `ah upgrade --rollback` with rollback-of-rollback safety.
3. Remove the permanent backup only after successful user-requested rollback.
4. Detect pending durable transactions before configuration and dynamic plugin
   loading on every `ah` startup.
5. Start the helper to execute one deterministic recovery action and preserve
   state when human recovery is required.
6. Retain trust keys needed by the active installation or rollback backup.
7. Cover every durable state, corrupt/missing backup, `%APPDATA%` failure,
   interrupted rollback, and repeated recovery invocation.

Expected commit: `feat: recover and roll back self updates`

## Task 12: Close automated implementation and retain external acceptance

Affected files:

- updater and lifecycle references/recipes
- completion design and PRD verification sections
- `roadmap/managed-mcp-and-self-update.md`
- applicable release documentation

Steps:

1. Run focused package and adapter tests while iterating.
2. Run the applicable workspace checks:
   - `ah run check cargo fmt --all -- --check`
   - `ah run check cargo test --workspace --all-targets --locked`
   - `ah run check cargo build --locked`
   - `ah run check cargo build --release --locked`
3. Run dependency, private-key, unsafe-path, workflow, and deterministic-output
   audits.
4. Mark every automated engineering item complete without deleting it.
5. Keep real signed-release consumption, target-client connectivity, persistent
   scheduler, interruption, reboot, and Windows VM items open until executed.
6. Record each completed milestone and the remaining external acceptance in
   Basic Memory.

Expected commit: `docs: close automated managed updater implementation`
