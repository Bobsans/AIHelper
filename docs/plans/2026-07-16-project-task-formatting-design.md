# Project and Task Text Formatting Design

## Goal

Apply semantic terminal formatting to project detection and task management
metadata while preserving plain captured output, JSON contracts, and executed
task streams.

## Scope

- Format `project detect`, `project commands`, and `project version`.
- Format `task save` and `task list`.
- Keep `task run` stdout and stderr byte-for-byte unchanged.
- Preserve JSON schemas, text content, command syntax, and exit behavior.
- Update tests and command reference documentation.

## Project Detect

The current line-oriented layout remains unchanged.

Interactive styles:

- root path: key
- ecosystem, tool, and role values: key
- empty `-` values: muted
- file group labels (`package`, `lock`, `ci`, and others): heading
- detected file kind: key
- detected file path: key

No new text fields are added.

## Project Commands

The existing `<kind>: <command>` layout remains unchanged.

Interactive styles:

- command kind: heading
- command text: key

Confidence and reason remain JSON-only.

## Project Versions

The existing line layout remains:

```text
<kind> <path> name=<name> version=<version> confidence=<confidence>
```

Interactive styles:

- kind and path: key
- name: plain, or muted when unavailable
- version: success, or muted when unavailable
- `confidence=high`: success
- `confidence=medium`: warning
- `confidence=low`: error
- unknown confidence: muted
- `no project versions found`: muted

## Task Save and List

`task save` keeps:

```text
saved task '<name>' -> <command>
```

Interactive styles:

- `saved task`: success
- task name: key
- arrow: muted
- command: muted

`task list` keeps:

```text
<name> => <command>
```

Interactive styles:

- task name: key
- arrow and command: muted
- `no tasks saved`: muted

`task run` output is not wrapped, prefixed, or recolored.

## Renderer Structure

Use small pure renderer helpers in the existing project and task output modules.
Helpers accept `TextFormatter` so forced-color tests remain deterministic.

## Testing

- Unit-test colored and plain project detect metadata.
- Unit-test confidence style mapping.
- Unit-test colored and plain task save/list rendering.
- Verify captured project and task text output contains no ANSI sequences.
- Verify task-run child output remains unchanged.
- Verify JSON output remains ANSI-free.
- Run workspace formatting, tests, and build checks.

## Documentation

Update `docs/reference/project.md` and `docs/reference/task.md` with the
interactive formatting and automatic no-color policy.
