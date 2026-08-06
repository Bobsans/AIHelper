# Managed MCP console detachment

## Problem

The per-user Task Scheduler action starts the managed MCP worker as the
console-subsystem `ah.exe`. Windows creates a visible console for that worker,
and the worker remains attached to the console lifecycle. Closing the launching
host can therefore terminate the service with `0xC000013A`.

## Decision

Keep the existing per-user Task Scheduler architecture and interactive user
token. For the internal `mcp serve --managed-config` invocation only, detach
the worker from its console through Win32 before updater recovery, plugin
discovery, logging, or runtime startup.

If the worker owns a private console, hide its window before detaching to avoid
a visible flash. If no console is attached, treat the worker as already
headless. A real detach failure returns a stable startup error.

The public `ah` CLI, stdio MCP transport, and `ah mcp service start` controller
remain attached to their caller's console. Task Scheduler continues to own the
same worker PID, so restart policy, instance enumeration, controlled shutdown,
and forced-stop fallback remain unchanged.

## Alternatives

- A Windows SCM service would run without a user session, but requires
  elevation and a separate service identity/profile. That is unnecessary for a
  local Codex MCP server.
- Task Scheduler S4U would be non-interactive, but would remove access to the
  user's network credentials and encrypted files.
- A wrapper process or second GUI-subsystem executable would add packaging and
  process-supervision complexity without improving the current per-user model.

## Validation

- Unit-test that console detachment is selected only for the internal managed
  serve invocation.
- Run the focused tests and workspace checks.
- Install the rebuilt binary, start the managed service, and verify readiness,
  MCP initialization, Scheduler ownership, and survival after the controller
  terminal exits.
