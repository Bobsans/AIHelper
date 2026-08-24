# `ah http`

Bundled plugin domain for HTTP request and API assertion workflows.

Designed to support both:
- quick one-off calls from terminal
- repeatable API checks for CI

## `ah http request`

Universal request command for any method.

```bash
ah http request --method <METHOD> <url> [--credential basic=ID] [--header "K: V"] [--query "k=v"] [--timeout-secs N] [--max-response-bytes BYTES] [--retry N] [--retry-delay-ms N] [--bearer TOKEN] [--basic USER:PASS] [--json "<obj>"|--json-file <path>] [--body "<text>"|--body-file <path>] [--expect-status <code|range>] [--expect-header "K: V"] [--expect-body-contains "<text>"] [--expect-json "<PATH:OP[:VALUE]>"]
```

## `ah http get|post|put|patch|delete`

Method-specific shortcuts over `request`.

```bash
ah http get <url> [same flags as request]
ah http post <url> [same flags as request]
ah http put <url> [same flags as request]
ah http patch <url> [same flags as request]
ah http delete <url> [same flags as request]
```

Behavior:
- no duplicated transport logic; method commands map to `request`
- assertion flags can be used in one-off calls
- `--retry N` enables up to `N` additional attempts; the default is `0`
- `--retry-delay-ms N` adds a fixed delay between attempts; the default is `0`
- transport failures, timeouts, response read failures, and `5xx` are retryable; `4xx` and assertion failures are not
- retries are opt-in and can repeat remote mutations for POST, PUT, PATCH, and DELETE after an ambiguous failure
- response bodies are bounded while read; `--max-response-bytes` defaults to `8388608`
- JSON output sets `body_truncated=true` and `truncated=true` when the body exceeds the limit
- status and header assertions still run for truncated bodies; body and JSON assertions fail explicitly because the complete body is unavailable
- interactive fallback status lines use status-class colors (`2xx`, `3xx`, `4xx`, `5xx`)
- response body content is never recolored

Vault-backed Basic authentication is available for `request`, `get`, `post`,
and `replay`:

```bash
ah http get https://api.example.test/private --credential basic=service-api
```

`service-api` is the ID of an `http-basic` entry created with `ah secrets add`.
The host resolves the entry only after CLI validation; the username and password
are never copied into plugin argv or invocation logs. Do not combine the mapping
with `--basic`, `--bearer`, an `Authorization` header, or embedded curl auth.
Malformed mappings, duplicate `basic` slots, and credential-kind mismatches fail
before the request is sent.

## `ah http replay`

Replay a single curl command through a stable CLI contract.

```bash
ah http replay --curl "<curl ...>" [request/assert flags]
```

Behavior:
- parses supported curl options into internal request model
- allows overriding via explicit `ah http` flags
- unsupported curl options are rejected with `INVALID_ARGUMENT`

## `ah http assert`

Run multi-case API checks from spec file.

```bash
ah http assert <spec-path> [--var KEY=VALUE ...] [--retry N] [--retry-delay-ms N] [--fail-fast] [--report text|json|junit]
```

Flags:
- `--var KEY=VALUE`: override spec variables (repeatable)
- `--retry N`: use up to `N` additional attempts for each case
- `--retry-delay-ms N`: fixed delay between case attempts in milliseconds
- `--fail-fast`: stop on first failing case
- `--report text|json|junit`: output mode for assertion run

Behavior:
- default mode runs all cases and returns summary at end
- returns non-zero exit when at least one case fails
- `--report junit` writes XML to stdout
- interactive text reports color `PASS`, `FAIL`, and summary counters semantically
- assertions and extraction evaluate only the final response after retries

## `ah http run`

Alias for `assert`.

```bash
ah http run <spec-path> [same flags as assert]
```

## Output Contract

- text mode:
  - default for `request/get/post/.../replay`
  - default report mode for `assert/run`
- json mode:
  - global `--json` supported
  - for `assert/run`, global `--json` maps to `--report json`
- junit mode:
  - only for `assert/run`
  - `1 testcase = 1 case`

Conflict rule:
- if `--json` and `--report` are both set and conflict, command returns `INVALID_ARGUMENT`

Colors are disabled automatically for pipes, redirects, captured output, JSON,
and JUnit reports. Set `NO_COLOR` to disable colors explicitly.

## Spec Format (`assert`/`run`)

Primary format:
- YAML (`*.yaml`, `*.yml`)

Also supported:
- JSON (`*.json`) with same schema

Minimal shape:

```yaml
version: 1
defaults:
  base_url: http://127.0.0.1:8080
  timeout_secs: 10
  max_response_bytes: 8388608
vars:
  token: dev-token
cases:
  - name: health
    request:
      method: GET
      path: /health
    expect:
      status: 200
      json:
        - path: status
          eq: ok
```

`max_response_bytes` can be set in `defaults` and overridden for an individual case under `request`.

### Cross-case extraction

A successful case can extract values for later `{{variable}}` interpolation:

```yaml
cases:
  - name: create session
    request:
      method: POST
      path: /sessions
    expect:
      status: 201
    extract:
      token:
        json: data.token
      request_id:
        header: X-Request-Id
      user_path:
        text:
          regex: '"next":"([^"]+)"'
          group: 1
  - name: use session
    request:
      path: '{{user_path}}'
      headers:
        authorization: 'Bearer {{token}}'
        x-request-id: '{{request_id}}'
```

Each variable must declare exactly one selector:

- `json`: JSON path; strings are stored without quotes and other values as compact JSON
- `header`: case-insensitive response-header name
- `text`: first regex match; `group` defaults to `1`, while `0` selects the full match

Missing paths, headers, matches, or capture groups fail the current case. JSON
and text extraction also fail for truncated response bodies. Values are
published atomically only when every assertion and extractor in the case passes;
successful values replace variables with the same name. Extracted values are
never included in text, JSON, or JUnit reports because they may contain secrets.

## Assertion Model (`path + operator`)

JSON assertions in v1 use `path + operator` checks:
- `eq`
- `contains`
- `exists`
- `match`

CLI expression format for `--expect-json`:
- `path:eq:value`
- `path:contains:value`
- `path:exists[:true|false]`
- `path:match:<regex>`

Example:

```yaml
expect:
  json:
    - path: data.user.id
      exists: true
    - path: data.user.role
      eq: admin
```

## Implemented Scope

Implemented:
- one-off requests (`request`, method shortcuts, `replay`)
- spec-based checks (`assert`, `run`)
- variables from `vars` and `--var`
- retry flags (`--retry`, `--retry-delay-ms`)
- atomic cross-case extracted variables (`extract`)

Status: the implemented scope is available as a bundled plugin.
