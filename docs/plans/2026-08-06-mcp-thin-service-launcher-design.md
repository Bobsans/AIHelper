# MCP Thin Service Launcher Design

## Goal

Keep managed MCP execution windowless and owned by Task Scheduler without shipping a second copy of the full `ah` application.

## Design

`ah-mcp-service.exe` remains the GUI-subsystem task action, but becomes a standalone launcher that does not call `aihelper::run()`. It resolves the sibling `ah.exe`, forwards its command-line arguments, starts it with `CREATE_NO_WINDOW`, atomically assigns it to a kill-on-close Windows Job Object, waits for completion, and returns the child's exit code.

The launcher passes its PID through a private environment variable. Managed runtime state uses that scheduler-owned supervisor PID, while direct `ah mcp serve` execution continues to use its own process PID. This preserves the existing exact PID checks used by `status`, `stop`, and `restart` without changing the persisted schema.

## Scope

- Replace the current full application worker with the thin Windows launcher.
- Teach managed preflight to use the validated supervisor PID when present.
- Keep the existing release archive and updater support-file contracts.
- Update focused tests and user-facing documentation.

No shared DLL, extra workspace crate, protocol proxy, or public configuration option is introduced.

## Validation

- Unit-test supervisor PID parsing and Windows command-line forwarding.
- Run formatting, workspace tests, debug build, and release build.
- Compare release binary sizes.
- Install and exercise managed service `start`, `status`, `restart`, and `stop`.
- Run parallel MCP commands and visually confirm that no console windows appear.
