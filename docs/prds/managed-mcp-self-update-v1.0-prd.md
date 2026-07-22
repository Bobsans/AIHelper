# Managed MCP and Self-Update v1.0 - Product Requirements Document

## Requirements Description

### Background

- **Business problem:** AIHelper can run as a managed per-user HTTP MCP process,
  but its remaining acceptance status is unclear and it cannot safely update its
  executable and dynamic plugins as one release bundle.
- **Target users:** Windows 10, Windows Server 2016, and newer Windows users who
  run a portable AIHelper installation without administrator privileges.
- **Value proposition:** users can determine whether a trusted stable release is
  available, install it without moving their chosen path, and recover
  automatically from interruption or activation failure.

### Feature Overview

Core features:

- auditable completion status for managed MCP implementation and acceptance;
- GitHub stable-release discovery and signed read-only update checking;
- bounded candidate download, safe extraction, exact verification, and offline
  smoke testing;
- installation identity, portable legacy adoption, and `cargo install` refusal;
- external Windows helper with durable transaction, backup, rollback, and early
  recovery;
- managed MCP state restoration;
- deterministic text and versioned JSON diagnostics.

Feature boundaries:

- update execution is Windows x64 only in v1;
- Linux and macOS release publication continues but self-update is unsupported;
- updates are user-initiated and stable-only;
- no arbitrary downgrade, background updater, release channels, or destructive
  configuration migration;
- manual client, persistent scheduler, and Windows VM evidence remains an
  explicit acceptance layer.

### User Scenarios

1. `ah upgrade --check` reports that the current version is up to date without
   writing files or stopping MCP.
2. `ah upgrade --check` reports a newer trusted compatible release.
3. `ah upgrade` prepares and verifies a complete candidate before stopping any
   process, then activates it and restores a previously running managed MCP.
4. Any failure after replacement begins automatically restores the verified
   previous version.
5. `ah upgrade --rollback` restores the one retained permanent backup and
   consumes it only after successful verification.
6. A legacy portable installation is adopted only when all signed managed-file
   hashes match; a cargo-managed installation is left unchanged with guidance.

### Inputs and Outputs

- `ah upgrade --check` performs trusted discovery only.
- `ah upgrade` selects and installs the highest compatible stable release.
- `ah upgrade --version <VERSION>` selects one stable non-downgrade release.
- `ah upgrade --rollback` consumes the single verified permanent backup.
- Selection flags are mutually exclusive and respect existing global output
  options.
- Text identifies the operation, current and selected versions, final state, MCP
  restoration state when applicable, and rollback outcome when applicable.
- JSON has an explicit schema version and stable operation, status, version,
  target, source, activation, service-restoration, and rollback fields.
- Errors use stable updater categories while untrusted response bodies, unsafe
  paths, signatures, and secret material are excluded from diagnostics.

### Success Metrics

- Every pre-mutation failure leaves installation bytes and managed MCP state
  unchanged.
- Every injected post-mutation failure restores the verified previous release or
  retains sufficient verified state for one deterministic recovery action.
- Automated tests observe zero changes to user-owned files in all update,
  rollback, and recovery scenarios.
- A released updater consumes a real signed release and passes the defined
  Windows VM acceptance matrix before the roadmap is closed.

## Design Decisions

### Technical Approach

- Preserve `ah-release-manifest` as the signed contract and verifier.
- Add an internal `ah-updater-core` shared by orchestration and helper tests.
- Keep network, candidate preparation, CLI, and lifecycle capture in `ah`.
- Keep replacement, rollback, and recovery in a network-free external
  `ah-update-helper.exe`.
- Use the public GitHub Releases REST API as discovery and Ed25519 as trust.
- Use exact managed-file manifests and never infer ownership by directory scan.
- Run recovery before configuration and dynamic plugin loading.

### Data Requirements

- Production public-key registry with deterministic key IDs and no private data.
- Canonical signed manifest and detached signature for each release archive.
- Installation metadata containing installation identity and canonical path.
- Verified installed manifest for the active managed release.
- Exact detached signature bytes for the installed manifest and retained backup.
- Immutable transaction plan containing old/new manifests and exact file ops.
- Durable transaction journal written before irreversible transitions.
- One complete transaction backup and at most one permanent rollback backup.

### Constraints

- No elevation or stored Windows password.
- Existing plugin ABI, released JSON field names, and MCP contracts remain
  compatible.
- All network requests are bounded, HTTPS-only, and redirect-limited.
- All archive and filesystem paths are normalized before diagnostics or use.
- Helper lock handoff has no release/reacquire race.
- Third-party processes are never terminated.
- User-owned files and the permanent installation path are preserved.

### Risks and Mitigations

- **Compromised metadata:** trust only verified canonical manifests.
- **Archive attacks:** reject unsafe entries and enforce exact inventory and size
  limits before activation.
- **TOCTOU:** helper rehashes staging and backup immediately before mutation.
- **Crash or reboot:** write-ahead durable states and early idempotent recovery.
- **Mixed executable/plugins:** replace only from one verified candidate and
  rollback the full managed set on failure.
- **Lock gap:** transfer one inherited Windows handle with acknowledgement.
- **Foreign blockers:** abort before mutation.
- **Key compromise:** multi-key registry permits overlap and later rotation;
  private seed remains only in the protected signing environment, and keys used
  by the active installation or rollback backup are not removed prematurely.

## Acceptance Criteria

### Functional Acceptance

- [ ] Managed MCP automated behavior and deferred manual evidence are separately
      visible in the roadmap.
- [ ] `ah upgrade --check` selects the highest compatible stable GitHub release,
      verifies its signed Windows manifest, and performs no mutation.
- [ ] The highest stable release never falls back to an older asset set, and an
      unsigned legacy release cannot produce a trusted `up_to_date` result.
- [ ] Candidate archive and every extracted file match the verified manifest.
- [ ] Offline smoke validates executable version, helper protocol, and plugin
      catalog without user configuration.
- [ ] Portable, legacy, and cargo-managed installations are distinguished safely.
- [ ] Helper changes only verified managed paths and preserves every user file.
- [ ] Managed MCP is restored only if it was running before update or rollback.
- [ ] Failure after mutation begins restores the previous verified installation.
- [ ] `--rollback` restores and consumes one verified permanent backup.
- [ ] Pending transactions recover before dynamic plugin loading.

### Quality Standards

- [ ] Private signing material is absent from source, binaries, logs, argv,
      artifacts, and state.
- [ ] Network, archive, and diagnostics limits have success and failure tests.
- [ ] Every durable transition and managed-file operation has failure injection.
- [ ] Text and JSON results are deterministic and documented.
- [ ] Existing CLI, MCP, plugin ABI, and released JSON compatibility tests pass.
- [ ] Applicable workspace format, test, debug build, and release build checks
      pass before each milestone is closed.

### External Acceptance

- [ ] Production key is provisioned and the matching public key is embedded.
- [ ] A real signed GitHub release is consumed by the released updater.
- [ ] Claude Code, Codex, and OpenCode connect to one managed HTTP MCP instance.
- [ ] Persistent Task Scheduler lifecycle and restart matrix passes on supported
      Windows versions.
- [ ] Update interruption and reboot matrix passes in Windows VMs.

## Execution Phases

### Phase 0: Progress Repair

- Restore removed completed roadmap tasks as checked entries.
- Label manual and external acceptance items without claiming completion.
- Record the implemented release pipeline as awaiting production activation.

### Phase 1: Trust-to-Check

- Provision the production Ed25519 keypair and embed its public identity.
- Make `minimum_updater_version` an explicit release-policy input representing
  the oldest compatible updater instead of copying the release version.
- Add shared updater core foundations and GitHub release selection.
- Implement mutation-free `ah upgrade --check` with deterministic text/JSON.
- Cover release, network, trust, compatibility, and no-mutation contracts.

### Phase 2: Verified Candidate

- Download and hash the archive with strict limits.
- Safely extract and reconcile the exact manifest inventory.
- Add isolated offline executable/helper/plugin smoke verification.

### Phase 3: Installation Model

- Derive installation root and persist installation identity.
- Refuse cargo-managed self-update.
- Adopt matching legacy portable releases without changing modified files.

### Phase 4: Transaction Helper

- Add `ah-update-helper.exe` to the signed Windows release inventory using the
  existing `update_helper` manifest purpose.
- Package and launch the helper outside the installation root.
- Implement lock handoff, verified backup, durable journal, replacement,
  rollback, and recovery against synthetic installations.
- Complete automated failure injection before exposing mutation publicly.

### Phase 5: Activation

- Integrate Restart Manager and managed MCP lifecycle capture/restoration.
- Implement permanent verification and installed-manifest persistence.
- Expose stable update and explicit-version installation without downgrade.

### Phase 6: Backup and User Rollback

- Rotate one permanent backup only after successful activation.
- Implement one-shot `ah upgrade --rollback` and rollback-of-rollback safety.
- Add deterministic update, rollback, and recovery diagnostics.

### Phase 7: Acceptance

- Run real signed-release, target-client, persistent scheduler, Windows VM,
  interruption, and reboot matrices.
- Close the roadmap only when automated and external acceptance are both visible.

---

**Document Version:** 1.0

**Created:** 2026-07-22

**Clarification Rounds:** 4

**Quality Score:** 96/100
