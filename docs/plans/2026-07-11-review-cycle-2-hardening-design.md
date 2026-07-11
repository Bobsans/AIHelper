# Review Cycle 2 Hardening Design

Date: 2026-07-11

## Context

The second project-wide code review found eight correctness and resource-safety gaps:

1. relative `--cwd` is applied twice;
2. host-global flags are removed from `run check` child arguments;
3. `task run` has no timeout and buffers output without a byte bound;
4. GitHub run logs buffer the complete archive and expanded contents;
5. GitLab job traces buffer the complete response and all parsed lines;
6. GitHub issue pages are filtered for pull requests only after fetching one page;
7. GitLab pipeline wait can sleep past its deadline and poll again;
8. a panic in a generated plugin invocation entrypoint aborts the host process.

This design closes those gaps without changing the plugin ABI version or removing existing JSON fields. The existing uncommitted Sphinx detection refactor in `src/commands/project/rules.rs` is outside this work and must be preserved.

## Goals

- Preserve current CLI behavior except where it is demonstrably incorrect.
- Bound process and provider-log memory use while data is read.
- Keep resource limits configurable with safe defaults.
- Preserve existing JSON field names, plugin symbols, and ABI layout.
- Return deterministic errors for timeouts, oversized responses, and plugin panics.
- Add success and failure regression coverage for every changed contract.

## Non-goals

- A new workspace-wide resource-limit framework.
- Plugin ABI v2 or out-of-process plugin isolation.
- Replacing GitHub issue listing with the Search API.
- Redesigning all global CLI option placement.
- Adding retries or provider-wide pagination unrelated to the findings.

## Chosen Approach

Use focused fixes inside existing boundaries and extract only narrow reusable primitives where duplication already exists. Process execution is shared by `run check` and `task run`. GitHub and GitLab keep plugin-local bounded response helpers so transport utilities do not leak into the ABI crate.

Alternatives rejected:

- A shared `ResourceLimits` framework would create new cross-crate boundaries and a much larger regression surface.
- A CLI/wire/ABI redesign would solve more general problems but break compatibility and exceed the bug-fix scope.

## Design

### 1. Working-directory bootstrap

`--cwd` must be applied once before plugin discovery so configuration and dynamic plugins are resolved in the requested directory. The later directory change in `parse_runtime_command` is removed.

The raw startup scan treats `--` as an opaque boundary. A `--cwd` after that boundary belongs to a child command and cannot alter the host working directory. Relative paths are resolved by the single early directory change, so `--cwd src` never becomes `src/src`.

### 2. Opaque child arguments for `run check`

Host-global normalization applies only to host arguments. Once `run check` reaches its child command, the remaining tokens are preserved in their original order, including `--json`, `--quiet`, `--limit`, and `--cwd`.

An explicit `--` is always a hard boundary. The boundary must survive both host-side and plugin-side normalization; otherwise a second normalization pass could consume protected tokens. Existing global flags before the boundary continue to work.

The fix remains compatible with current direct-command syntax and does not require callers to add a shell.

### 3. Shared bounded process execution

The current `run check` process-group implementation is generalized to execute a prepared `Command` while retaining:

- concurrent stdout and stderr draining;
- independent byte budgets;
- process-group or Windows job termination on timeout;
- prefix capture by default and suffix capture for tail mode;
- deterministic timeout and truncation metadata.

`run check` prepares a direct command. `task run` prepares the platform shell command and passes it to the same runner.

`task run` adds:

- `--timeout-secs`, default `600`;
- `--max-output-bytes`, default `65536`, applied separately to stdout and stderr.

Both values must be at least one. Existing `TaskRunOutput` fields remain unchanged. Its `truncated` field becomes true when either the byte budget or the existing global line limit truncates output. A signaled or timed-out task never reports a fabricated exit code of zero.

### 4. Bounded GitHub run logs

`run logs` and `run warnings` add command-local overrides:

- `--max-body-bytes`, default `8388608`;
- `--max-expanded-bytes`, default `33554432`.

The compressed HTTP body is read through a `limit + 1` reader. `Content-Length` may reject an oversized response early, but the streaming byte count is authoritative. An incomplete ZIP is not parsed.

`ZipArchive` still uses a bounded in-memory buffer because it requires `Read + Seek`. Entries are then read incrementally. A cumulative counter measures actual expanded bytes across all entries; ZIP metadata is not trusted as the sole check. ANSI stripping, grep/warning filtering, and the global line limit happen while entries are read rather than after building all intermediate strings and vectors.

Compressed or expanded budget overflow returns `GITHUB_RESPONSE_TOO_LARGE`. It does not return a misleading partial archive result.

### 5. Bounded GitLab job traces

`job trace` and `job warnings` add `--max-body-bytes`, default `8388608`.

The response is read incrementally with an authoritative byte counter. Lines are ANSI-stripped, filtered, and retained only up to the global line limit while reading. The implementation does not create a complete response `String`, a complete line vector, and a second filtered vector.

Budget overflow returns `GITLAB_RESPONSE_TOO_LARGE` rather than partial output.

### 6. GitHub issue pagination

The non-search `/issues` flow filters pull requests on each page and continues requesting pages until one of these conditions is met:

- the requested number of real issues is collected;
- GitHub returns an empty or short page;
- no next page remains.

The existing default result target remains `20`, and the current effective maximum remains `100`. Ordering and REST filter semantics are preserved. Search requests continue to include `is:issue`.

### 7. GitLab pipeline deadline

The first pipeline poll remains immediate. After each nonterminal response, the wait loop computes the remaining time:

- zero remaining time returns timeout;
- sleep duration is `min(interval, remaining)`;
- the deadline is checked again before another HTTP request.

This prevents interval-based overshoot and a post-deadline poll. An HTTP request already in progress remains governed by the plugin HTTP timeout; full request cancellation is outside this focused fix.

### 8. Panic-safe generated plugin invocation

The generated `ah_plugin_invoke_json` entrypoint wraps parser and executor invocation in `catch_unwind(AssertUnwindSafe(...))`.

A caught panic becomes a normal `InvocationResponse` with error code `PLUGIN_PANIC` and the plugin domain. Panic payloads are not exposed because they may be unstable or contain sensitive values. The response is serialized through the existing owned C-string path.

ABI version, struct layout, exported symbols, and function signatures do not change. This protects unwind-enabled builds; a plugin compiled with `panic=abort` remains unrecoverable by definition.

## Error Contracts

- Zero resource limits return `INVALID_ARGUMENT`.
- `task run` timeout returns `TASK_TIMEOUT` and does not fabricate an exit code.
- GitHub compressed or expanded overflow returns `GITHUB_RESPONSE_TOO_LARGE`.
- GitLab trace overflow returns `GITLAB_RESPONSE_TOO_LARGE`.
- Plugin parser or executor panic returns `PLUGIN_PANIC` without a panic payload.
- Existing error codes remain unchanged for existing failure paths.

## Compatibility

- No existing JSON fields are renamed or removed.
- Existing successful command identifiers remain unchanged.
- New resource flags are optional and have safe defaults.
- The plugin ABI version and exported symbol set remain unchanged.
- GitHub issue ordering and REST filtering are retained.
- Existing absolute `--cwd` calls and host-global flags before the child boundary remain valid.

## Test Plan

### CLI and processes

- relative and absolute `--cwd`;
- child `--cwd` after `--` does not alter the host;
- `run check` preserves child `--json`, `--quiet`, `--limit`, and `--cwd`, with and without an explicit delimiter;
- host globals before the child boundary still apply;
- double normalization preserves the protected suffix;
- `task run` success, nonzero exit, byte truncation, line truncation, timeout, and descendant termination;
- zero and override limit validation.

### GitHub

- body exactly at the compressed budget and one byte over;
- missing or incorrect `Content-Length` cannot bypass the budget;
- cumulative expanded overflow across multiple ZIP entries;
- a very long line cannot bypass the expanded budget;
- grep, warnings, and global line limit work without full accumulation;
- pull-request-only and mixed first pages continue to later issue pages;
- exhaustion and the effective 100-item maximum;
- search queries retain `is:issue`.

### GitLab

- trace exactly at the byte budget and one byte over;
- streamed grep, warning matching, ANSI stripping, and line limiting;
- interval greater than remaining timeout;
- no HTTP poll at or after the deadline.

### Plugin API

- panic in a parser returns valid `PLUGIN_PANIC` JSON;
- panic in an executor returns valid `PLUGIN_PANIC` JSON;
- a later normal invocation still succeeds;
- ABI metadata and entrypoint signatures remain unchanged.

### Full validation

- `cargo fmt --all -- --check`;
- `cargo clippy --workspace --all-targets --locked -- -D warnings`;
- `cargo test --workspace --all-targets --locked`;
- `cargo build --locked`.

## Documentation

Update the task, GitHub, and GitLab command references and corresponding AI manuals/recipes for the new override flags and bounded behavior. Document `--` as the opaque child-argument boundary for `run check`.
