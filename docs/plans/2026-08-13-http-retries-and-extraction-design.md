# HTTP Retries and Cross-Case Extraction Design

**Date:** 2026-08-13  
**Status:** Approved

## Goal

Complete the two remaining HTTP engineering backlog items without changing the
released JSON output contracts:

1. opt-in retries for transient HTTP failures;
2. response value extraction for use by later assertion cases.

## Scope

Retry flags are available for the one-off HTTP commands, replay, and assertion
commands in both CLI and typed MCP invocation. Assertion extraction is declared
per case in the YAML specification and supports JSON paths, response headers,
and regular-expression captures from the text body.

Spec-level retry defaults, exponential backoff, jitter, and generic runtime
retry middleware are outside this change.

## Retry Contract

- `--retry N` requests up to `N` additional attempts after the initial attempt.
  The default is `0`.
- `--retry-delay-ms N` adds a fixed delay between attempts. The default is `0`.
- Retries apply to transport failures, request timeouts, response-body read
  failures, and HTTP statuses in the `500..=599` range.
- HTTP `4xx`, request construction errors, invalid input, and assertion failures
  are not retryable.
- Assertions are evaluated only against the final response.
- An expected `5xx` is still retried when retries are enabled because retry
  classification happens before assertion evaluation.
- Retries for mutating methods are explicitly opt-in. Documentation warns that
  POST, PUT, PATCH, and DELETE can repeat a remote mutation after an ambiguous
  transport failure or a `5xx` response.

The existing I/O adapter remains responsible for one request attempt. A small
domain-level retry executor owns classification, delay, and attempt control so
all entry points share identical behavior.

Typed MCP execution must respect `remaining_timeout_ms`. The retry executor
uses an absolute local deadline, checks the remaining budget before every delay
and attempt, and limits the next request timeout to that budget. It must not
start another attempt after the tool deadline has expired.

## Extraction Contract

An assertion case may declare an `extract` mapping:

```yaml
extract:
  token:
    json: data.token
  request_id:
    header: x-request-id
  csrf:
    text:
      regex: 'csrf=([^&]+)'
      group: 1
```

Each variable declares exactly one selector:

- `json` resolves an existing JSON path using the assertion engine's path
  resolver. JSON strings become raw strings; other JSON values become compact
  deterministic JSON text.
- `header` performs a case-insensitive response-header lookup.
- `text.regex` uses the first match in the response text. `group` defaults to
  `1`; group `0` selects the complete match.

Invalid selector shapes, invalid regular expressions, and invalid capture-group
references are specification errors detected before network execution when
possible. A missing path, header, match, or capture in an actual response fails
the current case with a source-specific message.

JSON and text extraction fail when the response body was truncated. Header
extraction can still be evaluated, but extraction is committed atomically, so
no value is published if any selector fails.

## Case Transaction Semantics

Assertion cases remain sequential. A case first evaluates assertions and all
declared extractors against its final response. Extracted values are collected
in a temporary ordered map and merged into the run's variable map only when the
case has no assertion or extraction failures.

This guarantees that later cases cannot observe partial or untrusted values.
A successful extraction may replace a variable with the same name, supporting
token-refresh workflows. Extracted values are never written to text, JSON, or
JUnit reports because they can contain secrets.

## Compatibility

- Existing command behavior remains unchanged when `--retry` is omitted.
- Existing specifications without `extract` continue to deserialize and run.
- Released text and JSON response field names remain unchanged.
- Plugin ABI types remain unchanged; retry and extraction stay inside the
  bundled HTTP domain.
- CLI and typed MCP expose the same retry arguments and validation.

## Validation

Tests cover:

- `503` followed by `200` retries and succeeds;
- `4xx` and assertion mismatches do not retry;
- retry exhaustion and delay behavior;
- retry deadline enforcement for typed MCP;
- JSON, header, and text-regex extraction across cases;
- extraction overwrite and atomic rollback;
- missing selectors, invalid regex/groups, and truncated bodies;
- absence of extracted secret values from all report formats;
- CLI and typed MCP schema/argument parity.

The implementation handoff runs the repository's formatting, workspace tests,
and locked build checks. HTTP reference and AI recipe documentation are updated,
and completed retry/extraction items are removed from the remaining backlog.
