# AI Info and Warning Formatting Design

## Goal

Extend the shared text formatter to the remaining host information command and
all current warning messages without changing machine-readable contracts.

## Scope

- Add a shared warning renderer to the `output` module.
- Route existing command and runtime warnings through that renderer.
- Add semantic formatting to `ah ai info` text output.
- Preserve existing warning text, JSON output, command syntax, and exit codes.
- Update relevant tests and command reference documentation.

Formatting the primary text output of plugin domain commands remains outside
this change.

## Warning Rendering

Call sites pass warning content without the `warning:` prefix. The shared
renderer owns the prefix and stderr color policy.

In interactive terminals:

- `warning:` uses the warning style.
- warning content remains normal text.
- continuation details may use the muted style.

Outside an interactive terminal, with `NO_COLOR`, and during captured test
output, the emitted text remains identical to the current plain contract.

The following warning sources move to the shared mechanism:

- truncated `ctx` output
- truncated or bounded `file` output
- truncated `git` output
- truncated HTTP response and line output
- truncated project detection output
- truncated `run` stdout and stderr
- truncated search output
- truncated task output
- dynamic plugin discovery and conflict warnings

## AI Info Rendering

`ah ai info` keeps its existing layout and text while applying semantic styles:

- title and section headings: heading
- flags, domains, and command usages: key
- note labels: heading
- example labels and commands: muted or key as appropriate
- descriptions and note content: plain text

The formatter is created once for stdout and passed through the text renderer.
JSON rendering is unchanged.

## Shared API

The `output` module exposes small intent-based helpers rather than macros:

- render or emit a warning line
- render muted continuation text
- reuse the existing `TextFormatter` and `TextStyle`

This keeps formatting testable and avoids coupling command modules to raw ANSI
sequences.

## Testing

- Unit-test colored and plain warning rendering.
- Verify warning text remains unchanged when color is disabled.
- Verify `ah ai info` captured text contains no ANSI sequences.
- Verify `ah ai info --json` remains valid and ANSI-free.
- Run existing warning and truncation integration tests.
- Run workspace formatting, tests, and build checks.

## Documentation

Update `docs/reference/ai.md` to describe interactive text formatting and the
automatic no-color behavior for pipes, redirects, JSON, and `NO_COLOR`.
