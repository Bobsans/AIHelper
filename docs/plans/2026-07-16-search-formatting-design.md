# Search Text Formatting Design

## Goal

Extend semantic terminal formatting to the `search` domain while preserving
search result content and all existing plain-text contracts.

## Text Search

For `ah search text` interactive text output:

- match paths use the key style;
- line numbers use the muted style;
- context locations use the muted style;
- context separators (`--`) use the muted style;
- matched source text and context source text remain unformatted;
- punctuation and the existing `path:line:text` and `path-line-text` shapes
  remain unchanged.

Exact substring highlighting is intentionally out of scope. It would require
additional span handling for literal, case-insensitive, regex, and Unicode
matches, while the current requirement is safe navigation-oriented formatting.

## File Search

For `ah search files` interactive text output, each matched path uses the key
style. Ordering and line structure remain unchanged.

## Compatibility

- ANSI sequences are emitted only for interactive terminal text output.
- JSON output remains deterministic and unchanged.
- Redirected, piped, and captured text output remains plain.
- Search result source text is never modified or decorated.
- Existing truncation warnings continue through the shared warning formatter.

## Validation

- Add unit tests for plain and styled text-match rendering.
- Add unit tests for styled file-path rendering.
- Keep existing integration tests as captured-output contract coverage.
- Update the search command reference and AI search recipe.
- Run formatting, workspace tests, and a locked debug build.
