# Windowless Windows Child Processes

## Problem

The managed MCP service runs as a Windows GUI-subsystem process, but its
non-interactive child commands are console-subsystem executables. Windows may
therefore create a visible terminal window for each Git, PowerShell, shell,
database, or other external tool invocation.

## Decision

All non-interactive child processes started by AIHelper on Windows use the
native `CREATE_NO_WINDOW` process creation flag. Interactive `ah` CLI startup
is unchanged.

Apply the flag at each shared process-launch boundary:

- the Job Object launcher used by `run.check`, `task.run`, and updater smoke;
- the runtime command helper used by Git and dependency checks;
- independent dynamic-plugin launchers for GitHub, GitLab, and PostgreSQL.

Do not add shell-specific hiding arguments or wrapper processes.

## Preserved Behavior

- stdout and stderr remain captured;
- stdin remains null or piped according to the existing caller;
- Job Object process-tree tracking, timeout, cancellation, and parallel calls
  remain unchanged;
- non-Windows behavior is unchanged.

## Validation

- add a focused Windows check that a captured child has no console window;
- run formatting, workspace tests, debug build, and release build;
- deploy the rebuilt Windows binaries and run multiple parallel MCP commands
  while visually confirming that no terminal windows appear.
