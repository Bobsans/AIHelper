# Windowless Managed MCP Worker Design

## Problem

Windows creates a console window before the console-subsystem `ah.exe` enters
`main`. Hiding and detaching that console after startup shortens the visible
flash but cannot prevent it. Managed Task Scheduler launches therefore still
briefly flash a terminal for service starts and scheduler restarts.

## Decision

Ship a Windows-only release companion named `ah-mcp-service.exe`. It reuses the
existing `aihelper::run()` entry point but is linked with the Windows GUI
subsystem, so Windows never creates a console for the process.

Task Scheduler will execute the companion directly with the existing internal
managed arguments. The durable service definition continues to identify the
installed `ah.exe`; the companion path is derived as a sibling of that main
executable. This preserves installation ownership, the current-user logon
context, and the invariant that the Scheduler instance PID is the MCP runtime
PID.

`mcp service install` must fail before registration when the required companion
is missing. Existing owned tasks that still execute `ah.exe` are reconciled to
the companion on the next install.

## Packaging And Updates

The Windows release workflow builds and packages `ah-mcp-service.exe`. The
release profile records it as a support file, not as the main executable, so
updater activation and process-control rules continue to see exactly one main
executable. Because it is present in the signed managed-file inventory, normal
upgrade, rollback, and recovery transactions replace it atomically with the
rest of the installation.

Linux and macOS archives are unchanged.

## Removed Workaround

The runtime `FreeConsole`/`ShowWindow` workaround and its Windows API features
are removed. Foreground `ah mcp serve`, stdio MCP, and all normal CLI commands
keep their existing console behavior.

## Verification

- Unit tests cover sibling worker-path derivation and missing-worker failure.
- Release-profile and workflow-contract tests require the companion only in the
  Windows archive.
- Workspace format, test, debug build, and release build checks pass.
- A deployed Windows installation registers the companion as the Task
  Scheduler action, reaches exact readiness, survives controller exit, and
  starts without a console window.
