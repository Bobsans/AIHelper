# Version Fast Path Design

## Goal

Return `ah --version` and `ah -V` without initializing configuration, event logging, built-in plugins, or dynamic plugin discovery.

## Behavior

- Activate the fast path only when the executable receives exactly one user argument: `--version` or `-V`.
- Preserve Clap's existing `ah <version>` output format.
- Do not write a `command.completed` event for fast-path version requests.
- Route all other argument combinations through the existing startup flow.

## Validation

- Integration tests cover both accepted version flags and verify that version requests do not create log records.
- Existing help, parsing, and command logging behavior remains covered by the integration suite.
- A release benchmark should target a warm-run median below 15 ms on the current Windows development machine.
