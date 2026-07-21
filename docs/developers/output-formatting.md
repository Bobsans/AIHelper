# Text Output Formatting

AIHelper uses semantic terminal formatting to make interactive text easier to scan
without changing its plain-text or machine-readable contracts.

## Stream Policy

Create a formatter for the stream being rendered. ANSI styles are enabled only
when that stdout or stderr stream is an interactive terminal and `NO_COLOR` is not
set. Piped, redirected, and captured output remains plain. JSON and other
machine-readable formats must never contain ANSI sequences.

There is no user-facing `--color` override. Renderer tests may force color on or
off, but production output always follows the target stream and `NO_COLOR`.

Formatting may emphasize semantic roles, but it must preserve existing wording,
separators, indentation, ordering, and exit behavior. Use intent-based styles such
as heading, key, success, warning, error, and muted instead of embedding raw ANSI
sequences in command renderers.

## Plugin Management and Errors

`plugins list` calculates column widths from plain values before applying styles,
so ANSI sequences cannot affect table alignment. Headings and domains are
emphasized keys, enabled state is success, disabled state is error, dynamic source
is key, and built-in source is muted. The empty result remains the plain
`no plugins registered` contract.

Plugin state mutations preserve their wording. A changed state uses success, while
an idempotent or no-op result uses warning.

Errors emitted through the host error renderer style the diagnostic code as error
and the `hint:` label as warning. Diagnostic messages and hint text remain unchanged;
JSON diagnostics and non-interactive stderr retain their existing contracts.

## Warnings

Warning call sites pass only the warning content. The shared warning renderer owns
the `warning:` prefix, writes to stderr, and applies the stream color policy. In an
interactive terminal only the prefix uses the warning style; the message stays
plain and optional continuation details may be muted. With formatting disabled,
the output remains the stable `warning: <message>` contract.

## Structured and Raw Content

Apply semantic styles to structured metadata and statuses, not to payloads intended
for downstream processing. Raw file content, HTTP bodies, model responses, SQL
results, CI logs, and child-process output remain unformatted.

`ah ai info` preserves its existing layout while styling headings and note labels
as headings, flags and command usages as keys, and supporting example text as muted
where appropriate.

The built-in `file` and `ctx` renderers use these domain mappings:

| Surface                     | Semantic formatting                                                                                                                                   |
|-----------------------------|-------------------------------------------------------------------------------------------------------------------------------------------------------|
| `file stat`                 | Paths and regular files are keys; directories are headings; labels and numeric metadata are muted; symlinks and readonly state are warnings.          |
| `file tree`                 | Directories are headings, symlinks are warnings, and regular files are keys; indentation, markers, and suffixes stay unchanged.                       |
| `file read`, `head`, `tail` | File content is always unformatted; bounded-output diagnostics use the shared warning renderer.                                                       |
| `ctx pack`, `symbols`       | Presets and paths are keys; labels, counters, and line numbers are muted; item and symbol kinds are headings; non-zero skipped counters are warnings. |
| `ctx changed`               | Changed paths are keys; clean state is success; non-repository state is warning; Git-like statuses use their corresponding semantic state.            |

## Git

Git renderers style repository metadata while preserving source material. Branches,
upstreams, hashes, tags, remote names, providers, authors, and paths are keys;
secondary metadata and zero or unavailable counters are muted. Additions and staged
counts use success, deletions and conflicts use error, and modified, untracked,
ahead, behind, or otherwise concerning states use warning.

Git status codes share one priority mapping: conflicts and deletions are errors,
untracked and modified entries are warnings, additions are success, renames and
copies are keys, and unknown states are muted. Clean-state messages use success,
no-data messages use muted, and non-repository or missing-commit messages use
warning. Patch output from `git diff` and source text from `git blame` must remain
unformatted.

## GitHub and GitLab Plugins

Provider plugins use keys for repository or project names, issue identifiers,
tags, refs, workflow paths, assets, users, design files, and URLs. Successful or
active states use success; queued, pending, running, draft, and prerelease states
use warning; failed, cancelled, timed-out, and action-required states use error;
closed, skipped, neutral, and unavailable states use muted.

Issue and workflow titles remain plain source text. Full issue descriptions,
comment bodies, CI logs, job traces, and extracted log lines stay unformatted.
Partial-fetch diagnostics may use warning without styling the returned raw content.
Quiet mode remains silent, and plugin diagnostics continue through the host error
renderer.

## PostgreSQL Plugin

PostgreSQL renderers style tool and database metadata while keeping database output
safe for downstream processing:

| Surface                | Semantic formatting                                                                                                                                                                                                                                  |
|------------------------|------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| Tooling and connection | Available tools and successful pings are success; missing tools and remediation are warnings; rejected candidates and failures are warning or error; versions, paths, commands, databases, users, and schemas are keys; secondary metadata is muted. |
| Inspection rows        | Object names are keys; relation and constraint kinds are headings; owners, encodings, versions, sizes, sources, and counts are muted; true primary or unique flags are success.                                                                      |
| Activity and locks     | PIDs and object names are keys; active or idle-in-transaction states and blocking PIDs are warnings; blocked PIDs are errors; normal idle states are muted.                                                                                          |
| Describe               | Relation identity and sections use heading or key; column and index names are keys; data and constraint types are headings; nullable and default metadata is muted.                                                                                  |

Never format query result payloads, `postgres exec` stdout or stderr, explain plans,
SQL text or definitions, comments, active query text, or output passed through from
`psql`.

## Project and Tasks

Project detection uses keys for root paths, ecosystems, tools, roles, detected file
kinds, and file paths; file-group labels use headings and empty `-` values are
muted. Project command kinds are headings and their command text is a key. Confidence
and detection reasons remain JSON-only for `project commands`.

Project versions keep their existing line layout. Kinds and paths are keys; detected
versions and high confidence use success; medium confidence uses warning; low
confidence uses error; unavailable or unknown values and no-result messages are
muted.

Task save/list renderers use keys for task names and muted styles for arrows and
stored commands; a successful save uses success and an empty list message is muted.
`task run` must not wrap, prefix, or recolor child stdout or stderr.

## Run and HTTP

`run check` styles successful status as success, failed status as error, active
timeouts as warning, and inactive timeouts, exit codes, and durations as muted.
The `stdout:` heading is a key and `stderr:` is an error heading. Captured child
stdout and stderr remain byte-for-byte unformatted.

When an HTTP request has no response body, its fallback status line maps 2xx to
success, 3xx to key, 4xx to warning, 5xx to error, and other values to muted. A
non-empty response body is printed without an added status header and is never
inspected or highlighted.

HTTP assertion reports use keys for spec paths and case names, success for `PASS`,
error for `FAIL` and failure markers, and muted for totals and durations. A non-zero
failed count is error and a zero failed count is muted; failure text itself stays
plain. JSON and JUnit renderers do not use the text formatter.

## Search

Search formatting is navigation-only. Match and file paths are keys; line numbers,
context locations, and context separators are muted. Matched source text and context
source text remain unformatted, including punctuation and the existing
`path:line:text` and `path-line-text` shapes.

Substring highlighting is intentionally outside this contract because literal,
case-insensitive, regular-expression, and Unicode matches would require a separate
span model. Truncation diagnostics continue through the shared warning renderer.

## Verification

Unit-test renderers with color forced both on and off. Integration tests capture
text output and assert that it contains no ANSI escape sequences. JSON tests must
continue to validate the unchanged structured contract.
