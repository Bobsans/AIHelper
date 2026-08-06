# Updater Review Fixes Design

## Goal

Close six review findings in the managed updater and release pipeline without new dependencies or workspace architecture.

## Design

- Make helper handoff cleanup conditional on confirmed child termination and preserve durable recovery state otherwise.
- Run installed smoke and MCP restoration commands with a timeout, bounded output capture, and Windows Job Object tree termination.
- Pin every third-party GitHub Action used by CI and release workflows to a full commit SHA.
- Reject unsupported Unix ZIP entry types while producing release archives, matching updater validation.
- Bound identity file reads with `maximum + 1` bytes after the metadata precheck.
- Preserve the caller-specific activation, rollback, or recovery error code through handoff setup failures.

## Validation

Add focused regression tests, then run formatting, workspace tests, and locked debug build. Release publication and production trust-anchor provisioning remain outside this change.
