# Cross-Platform Update Helper Build Fix

## Context

Release preflight run `31081107886` succeeded on Windows but failed while
compiling `ah-update-helper` on Linux and macOS. `activation_command.rs`
imports `wait_for_process_exit` unconditionally, while that function is
defined only under `cfg(windows)`. The call itself already runs only inside a
Windows-gated block.

## Decision

Gate only the `wait_for_process_exit` import with `cfg(windows)`. Keep
`RecoveryCommand` imported on every platform because the public activation
function uses it in its cross-platform signature.

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
