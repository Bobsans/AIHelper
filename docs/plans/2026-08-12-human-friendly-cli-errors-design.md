# Human-friendly CLI errors

## Goal

Make the default text-mode CLI useful to people when an invocation is invalid or an operation fails. An error should explain what went wrong, show the correct invocation when relevant, and point to the most useful next action. Machine-readable JSON, MCP responses, released JSON fields, and the plugin ABI must remain unchanged.

## User experience

Text-mode invocation errors follow a Docker-like structure:

```text
ah: unrecognized command 'versoin'.

Did you mean:
  ah project version

Usage:
  ah project <COMMAND>

Run 'ah project --help' for more information.
```

The renderer answers up to four questions, omitting sections that are not relevant:

1. What did the user do wrong?
2. Is there a likely correction?
3. What is the valid command shape?
4. Which help command should the user run next?

Operational errors such as missing files keep the same human-oriented heading and provide contextual next actions, but do not show unrelated command usage.

## Design

### Preserve structured contracts

`AppError::diagnostic()` remains the source of JSON errors. No fields are added to `ErrorDiagnostic`, and no plugin request/response or C ABI structure changes. The richer presentation applies only to `AppError::print()` when `--json` is absent.

### Preserve complete parser diagnostics

Clap already produces high-quality diagnostics containing missing arguments, spelling suggestions, usage, and a help command. The current text renderer reduces that payload to its first line. The new renderer parses the stable semantic sections from the complete message and rewrites them into the common `ah` presentation without discarding useful lines.

### Add invocation-aware guidance

For errors that occur after parsing, the application attaches lightweight text-rendering guidance to the error before returning it to `main`:

- attempted invocation or invalid value;
- optional suggested command;
- optional usage line;
- optional help command;
- one or more actionable hints.

Unknown top-level domains use the enabled plugin catalog and host commands. Exact conventional aliases such as `ah version` can recommend `ah --version`; sufficiently close misspellings may recommend an available domain or command. Weak matches are not shown.

### Consistent rendering

The default console renderer uses these sections:

- `ah: <human message>` as the primary error;
- `Did you mean:` for a confident correction;
- `Usage:` for syntax and missing-input errors;
- a final `Run '<command> --help' for more information.` line;
- `Hint:` lines for operational recovery actions.

ANSI styling remains limited to labels and headings so redirected output stays deterministic and readable without color.

## Error categories

### Unknown domain or command

Explain that the token is not a command, suggest a confident correction when available, show root usage, and point to `ah --help`. Do not tell the user only to inspect the plugin registry.

### Invalid subcommand or argument

Retain Clap's specific cause, spelling suggestion, required argument names, usage, and scoped help command.

### Missing subcommand

Explain that a command is required, show scoped usage, and point to the scoped `--help` command.

### Operational failure

Show the complete normalized message or diagnostic cause and a practical recovery hint. Avoid syntax usage unless the failure was caused by invocation syntax.

## Testing

Add unit tests for deterministic plain and colored rendering, plus integration coverage for:

- `ah version` recommending `ah --version`;
- a misspelled plugin subcommand retaining Clap's suggestion;
- a missing required argument retaining its name and usage;
- an operational error retaining its human explanation and hint;
- `--json` preserving the existing structured diagnostic contract.

Run formatting, focused tests, the workspace test suite, and a locked build before handoff.
