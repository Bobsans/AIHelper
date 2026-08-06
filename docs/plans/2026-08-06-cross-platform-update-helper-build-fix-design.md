# Cross-Platform Update Helper Build Fix

## Context

Release preflight run `31081107886` succeeded on Windows but failed while
compiling `ah-update-helper` on Linux and macOS. `activation_command.rs`
imported `wait_for_process_exit` unconditionally, while that function is
defined only under `cfg(windows)`. The call itself already runs only inside a
Windows-gated block.

After that import was gated, run `31082432625` exposed the same issue in the
root updater: `activate.rs` imports three Windows-only managed-service
lifecycle functions, and `recovery.rs` implements its process runner through
the Windows-only `handoff` module. Both uses are reachable only from existing
Windows x64 blocks.

## Decision

Use the narrow existing platform boundaries:

- gate only the `wait_for_process_exit` import with `cfg(windows)` while
  keeping `RecoveryCommand` available for the cross-platform function
  signature;
- gate the three managed-service lifecycle imports with the same Windows x64
  condition as their caller;
- gate `ProcessRecoveryRunner` and its implementation with that Windows x64
  condition while keeping the runner trait and shared recovery logic available
  for cross-platform tests.

Do not add a non-Windows process-wait implementation or change workspace and
workflow dependency boundaries. Update activation remains Windows-only, and
the existing non-Windows path continues to return `UnsupportedPlatform`.

## Validation

- Run the four required local release checks.
- Confirm the working tree contains only the focused fix and this design.
- Commit and push the fix without amending the existing release commit.
- Dispatch a new `release.yml` preflight on the new exact commit.
- Require successful Linux x64, Windows x64, macOS ARM64, archive validation,
  warnings review, and all three expected archives before tagging.
