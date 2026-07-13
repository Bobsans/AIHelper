# UTF-8 Prefix Boundary Design

- Date: 2026-07-13
- Status: Approved
- Scope: Prevent valid UTF-8 text from being classified as binary when the safety sniff buffer ends inside a multibyte character.

## Context

The shared text-file safety check reads an 8192-byte prefix and validates that isolated prefix with `std::str::from_utf8`. A valid file is rejected when byte 8191 starts a multibyte UTF-8 character whose continuation lies beyond the sniff boundary. The check is shared by the `file`, `ctx`, and `search` domains.

## Goals

- Accept an otherwise valid UTF-8 prefix with only an incomplete trailing character.
- Continue rejecting NUL-containing data and definite malformed UTF-8.
- Preserve existing command output, JSON fields, limits, and error conventions.
- Cover the shared helper and an end-to-end `file read` regression.
- Audit other bounded UTF-8 conversions for the same false-rejection pattern.

## Non-goals

- Changing the 8192-byte sniff limit.
- Adding encoding detection or support for non-UTF-8 text.
- Changing intentionally lossy decoding of command, HTTP, or plugin output.
- Refactoring unrelated file-reading code.

## Considered Approaches

### 1. Boundary-aware prefix validation

Inspect `Utf8Error`: accept an error only when `error_len()` is `None`, which means the valid prefix ends with an incomplete UTF-8 sequence. Reject errors with a known invalid length. This is the selected approach because it changes only the incorrect boundary classification.

### 2. Extend the read through a character boundary

Read additional bytes until the prefix ends on a UTF-8 boundary. This couples binary sniffing to more complex incremental I/O without improving full-file validation.

### 3. Remove UTF-8 validation from binary sniffing

Check only for NUL and let command-specific readers reject malformed UTF-8. This weakens early filtering and changes when invalid files are rejected or skipped.

## Design

`is_probably_binary` keeps its empty-input and NUL checks. For non-NUL data it matches `std::str::from_utf8(prefix)`:

- valid UTF-8 returns `false`;
- an error with `error_len() == None` returns `false` because only the trailing sequence is incomplete;
- an error with `error_len() != None` returns `true` because the prefix contains definite malformed UTF-8.

Full readers remain responsible for validating bytes beyond the sniff prefix. Consequently, a file with a valid sniff prefix but malformed later content is still rejected or skipped by the existing full-read path.

## Audit Result

The same arbitrary-prefix validation is centralized in `src/safety.rs`; `file`, `ctx`, and `search` all consume it. Other `from_utf8` uses found in the scoped audit operate on complete line buffers. Bounded HTTP and command output uses lossy conversion and therefore cannot produce this false binary rejection.

## Testing

- Unit tests accept incomplete trailing 2-, 3-, and 4-byte UTF-8 sequences.
- Unit tests reject definite malformed UTF-8 and NUL-containing data.
- An integration test creates valid text whose multibyte character crosses byte 8192 and verifies `ah file read` succeeds.
- Relevant integration tests run first, followed by formatting, workspace tests, and locked build checks.

