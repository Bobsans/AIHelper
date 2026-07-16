# Git Text Formatting Design

## Goal

Apply semantic terminal formatting to Git command metadata while preserving raw
diff content, source text, JSON contracts, and plain captured output.

## Scope

- Format `git status`, `tags`, `remotes`, `changed`, `blame`,
  `commit-info`, and `tag create`.
- Format no-data and non-repository messages semantically.
- Keep `git diff` patch content byte-for-byte unchanged.
- Keep blame source text unchanged.
- Preserve JSON schemas, text content, command syntax, and exit behavior.
- Update tests and Git command reference documentation.

## Git Status

The existing two-line summary layout remains unchanged.

Interactive styles:

- branch and upstream tokens: key
- latest commit hash and latest tag: key
- `clean=true`: success
- `clean=false`: warning
- staged count when non-zero: success
- changed, unstaged, untracked, ahead, and behind when non-zero: warning
- zero or unavailable counters: muted
- commit subject: plain text

## Changed Entries

The existing `<status> <path>` and rename layout remains unchanged.

Status priority:

- conflicts or unmerged states: error
- deleted: error
- untracked or modified: warning
- added: success
- renamed or copied: key
- unknown states: muted

Paths use the key style. Rename arrows remain plain.

## Commit Information

- commit hash and file paths: key
- author identity and date: muted metadata
- subject: plain
- additions: success
- deletions: error
- file count: muted
- per-file status: same semantic status mapping as changed entries

## Tags, Remotes, Blame, and Tag Creation

- tag names: key
- remote names and provider hints: key
- fetch and push URLs: muted
- blame line numbers: muted
- blame commit hashes: key
- blame authors: key
- blame source text: plain
- successful tag creation phrase: success
- created tag and target commit: key

## Diff and No-Data Messages

Raw diff content is printed without formatter involvement.

Semantic messages:

- clean working tree: success
- no local diff or no blame data: muted
- not a Git repository or commit not found: warning

Truncation warnings continue to use the shared warning renderer.

## Renderer Structure

Use small pure helpers in `git/output.rs`:

- render status summary lines
- map Git status codes to semantic styles
- render changed and commit file entries
- render metadata tokens

Helpers accept `TextFormatter` so forced-color unit tests remain deterministic.

## Testing

- Unit-test Git status-code style mapping.
- Unit-test colored and plain status summary rendering.
- Unit-test colored and plain changed and commit-file entries.
- Verify integration text output contains no ANSI sequences when captured.
- Verify raw diff output remains unchanged.
- Verify JSON output remains ANSI-free.
- Run workspace formatting, tests, and build checks.

## Documentation

Update `docs/reference/git.md` with the semantic formatting policy and explicit
guarantees for raw diff and blame source text.
