# `ah http`

Bundled plugin domain for HTTP request and API assertion workflows.

Designed to support both:
- quick one-off calls from terminal
- repeatable API checks for CI

## `ah http request`

Universal request command for any method.

```bash
ah http request --method <METHOD> <url> [--header "K: V"] [--query "k=v"] [--timeout-secs N] [--max-response-bytes BYTES] [--bearer TOKEN] [--basic USER:PASS] [--json "<obj>"|--json-file <path>] [--body "<text>"|--body-file <path>] [--expect-status <code|range>] [--expect-header "K: V"] [--expect-body-contains "<text>"] [--expect-json "<PATH:OP[:VALUE]>"]
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
- response bodies are bounded while read; `--max-response-bytes` defaults to `8388608`
- JSON output sets `body_truncated=true` and `truncated=true` when the body exceeds the limit
- status and header assertions still run for truncated bodies; body and JSON assertions fail explicitly because the complete body is unavailable

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
ah http assert <spec-path> [--var KEY=VALUE ...] [--fail-fast] [--report text|json|junit]
```

Flags:
- `--var KEY=VALUE`: override spec variables (repeatable)
- `--fail-fast`: stop on first failing case
- `--report text|json|junit`: output mode for assertion run

Behavior:
- default mode runs all cases and returns summary at end
- returns non-zero exit when at least one case fails
- `--report junit` writes XML to stdout

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

## v1 Scope and v1.1 Notes

Included in v1:
- one-off requests (`request`, method shortcuts, `replay`)
- spec-based checks (`assert`, `run`)
- variables from `vars` and `--var`

Deferred to v1.1:
- retry flags (`--retry`, `--retry-delay-ms`)
- cross-case extracted variables (`extract`)

Status: implemented (bundled plugin).
