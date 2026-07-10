# Runtime and Contract Hardening Design

- Date: 2026-07-10
- Status: Approved
- Scope: Resolve the seven findings from the project-wide code review without a broad runtime rewrite.

## Context

AIHelper promises bounded, predictable, agent-friendly behavior. The review found seven places where the implementation does not yet meet that contract:

1. `run check` kills only the direct child and can wait past its timeout for descendants that retain inherited pipes.
2. `run check` reads complete stdout and stderr into memory before applying `--max-output-bytes`.
3. HTTP responses are read into memory without a body-size limit.
4. Git porcelain output is parsed as quoted, line-delimited text and fails for valid unusual path names.
5. Search discovery changes depending on whether `rg` is installed.
6. The GitLab plugin guesses a GraphQL endpoint from arbitrary custom REST API roots.
7. Flattened unknown GitLab response fields leak into existing public JSON contracts.

The existing plugin architecture, diagnostic model, and user changes for `gitlab issue view --full` remain authoritative. This design hardens the affected boundaries without replacing them.

## Goals

- Make command timeouts apply to the complete process tree on Windows and Unix.
- Bound stdout, stderr, and HTTP response memory while streams are being read.
- Preserve useful prefix and tail output semantics with explicit truncation metadata.
- Parse Git status output without path quoting or delimiter ambiguity.
- Make search results independent of optional tools installed on the host.
- Support standard and reverse-proxied GitLab GraphQL endpoints explicitly.
- Preserve stable JSON response shapes when upstream GitLab adds fields.
- Add focused regression coverage and keep the full workspace suite passing.

## Non-goals

- Moving the CLI to an asynchronous runtime.
- Building a general transport framework shared by every plugin.
- Adding a raw GitLab payload mode.
- Preserving the accidental behavior where `--tail-lines` retained the start of an oversized tail.
- Representing non-UTF-8 paths losslessly in JSON; public path strings remain lossy UTF-8 for compatibility.

## Considered Approaches

### 1. Minimal local patches

Use `command-group`, add local bounded readers, duplicate a NUL parser in `git` and `ctx`, retain both search discovery implementations, and suppress serialization of flattened GitLab maps.

This minimizes the diff, but it leaves duplicated parsing and two search implementations that can drift again.

### 2. Deterministic boundaries

Use small focused dependencies for process groups and ignore-aware walking, introduce a shared Git-status parser, bound streams at their I/O boundaries, and expose explicit configuration for HTTP and GitLab endpoint limits.

This has moderate implementation cost and removes the causes of the findings. This is the selected approach.

### 3. Async infrastructure rewrite

Move command and HTTP I/O to an async runtime and introduce common stream and transport abstractions.

This offers strong backpressure primitives but creates a large regression surface and adds complexity not required by the current CLI.

## Architecture

Two focused dependencies are added to the root crate:

- `command-group` 5.x for Unix process groups and Windows Job Objects.
- `ignore` 0.4.x for deterministic recursive discovery using the same ignore semantics as ripgrep.

No new service, crate, plugin, or async boundary is introduced.

### Process execution

`run check` spawns the command through `command-group`. The direct child's stdout and stderr are drained concurrently by reader threads. Each reader owns a bounded accumulator and reports its captured bytes plus a truncation flag.

The coordinator polls both process status and reader completion until the deadline:

- If the leader exits and both pipes close, return normally.
- If the leader exits but descendants keep pipes open, continue enforcing the same deadline.
- At the deadline, kill the process group or Job Object, wait for the leader, then join the readers after the group closes the pipes.

This prevents both surviving descendants and an unbounded join after the advertised timeout.

### Bounded command output

Each stream has an independent `max_output_bytes` budget:

- Without `--tail-lines`, keep the first `max_output_bytes` bytes and drain the remainder without retaining it.
- With `--tail-lines`, keep a byte-bounded suffix and render the last requested lines from that suffix.
- With `--tail-lines 0`, return an empty stream while still draining it.
- Adjust UTF-8 boundaries only when converting the captured bytes to the public lossy UTF-8 string.
- Set the stream truncation flag whenever bytes were discarded.

The memory bound is proportional to twice `max_output_bytes` plus fixed-size reader buffers.

### Bounded HTTP responses

Add `max_response_bytes` to the request model with a default of 8 MiB. Expose it as:

- `--max-response-bytes` for one-off request commands and method shortcuts.
- An optional value in HTTP spec defaults.
- An optional per-request override in an HTTP spec case.

The blocking response reader consumes at most `limit + 1` bytes. A response above the limit returns the retained prefix and `body_truncated=true`.

Status and header assertions remain valid. Body and JSON assertions fail explicitly when the body is truncated because an incomplete body cannot prove those assertions. Truncated bodies are not parsed as JSON and cannot silently satisfy body-derived extraction.

### Git status parsing

Introduce one internal byte-oriented parser for `git status --porcelain=v1 -z`. It is shared by `git status`, `git changed`, and `ctx changed`.

The parser:

- Reads the two-byte `XY` status and the path following `XY `.
- Uses NUL terminators rather than lines or ` -> ` delimiters.
- For rename/copy entries, treats the first path as the new path and the following NUL field as the old path, as defined by porcelain v1 `-z`.
- Derives staged, unstaged, and untracked counts from the parsed entries.
- Converts path bytes to lossy UTF-8 only at the public output boundary.

### Search discovery

Replace both `rg` candidate discovery and the `WalkDir` fallback with one `ignore::WalkBuilder` discovery path. The existing Rust content matcher remains unchanged.

The walker consistently respects hidden-file behavior, `.ignore`, `.gitignore`, `.git/info/exclude`, global Git ignores, symlink policy, and caller globs. Search JSON reports the stable backend value `ignore+rust`.

This intentionally favors deterministic results over an environment-dependent optional prefilter.

### GitLab GraphQL endpoint

Add the global `--graphql-url` option to the GitLab plugin. Endpoint precedence is:

1. Explicit `--graphql-url`.
2. Replace a known trailing `/api/v4` REST suffix with `/api/graphql`.
3. Use `<host>/api/graphql`.

An arbitrary custom REST root is never extended with `/graphql` because the relationship cannot be inferred safely.

### Stable GitLab JSON

Remove the unused flattened `extra` maps from issue, note, and design DTOs. Serde ignores unknown input fields by default, so newer GitLab responses remain readable while public output continues to contain only explicitly modeled fields.

`issue view --full` continues to return the explicit issue, comments, designs, counts, and warnings introduced by the current user changes.

## Public Contract Changes

The following changes are additive:

- HTTP commands accept `--max-response-bytes`.
- HTTP specs accept `max_response_bytes` at defaults and request levels.
- HTTP output exposes `body_truncated` where response metadata is returned.
- GitLab commands accept `--graphql-url`.

The following changes are intentional corrections:

- Oversized `--tail-lines` output is a bounded suffix instead of a prefix of an unbounded tail.
- Search backend is always `ignore+rust` and ignored/hidden traversal no longer varies with `rg` availability.
- Unknown GitLab fields no longer appear in public JSON.

Existing command names, stable error diagnostics, modeled JSON fields, and exit behavior remain unchanged.

## Error Handling

- Process-group setup and termination failures use the existing command-execution diagnostic path and retain the command label.
- A timeout still produces a successful `ah` invocation with `success=false` and `timed_out=true` for the checked command.
- HTTP body truncation is metadata for ordinary requests and an explicit assertion failure when a complete body is required.
- Malformed NUL porcelain data is rejected with a concise Git response error instead of being partially misreported.
- Invalid or empty `--graphql-url` values return `INVALID_ARGUMENT`.

## Testing

### Process and output

- A subprocess creates a grandchild that inherits stdout and attempts to write a marker after the timeout; the command returns near the deadline and the marker is never created.
- Oversized stdout and stderr are bounded independently.
- Prefix mode, tail mode, `tail-lines=0`, one giant line, and UTF-8 boundary cases are covered.

### HTTP

- Responses at the limit and at `limit + 1` bytes.
- Content-Length and chunked responses.
- Oversized JSON and body assertions fail explicitly.
- Status and header assertions still work on truncated bodies.
- CLI, spec-default, and per-request limits are covered.

### Git

- Ordinary paths with spaces, quotes, and the literal text ` -> `.
- Real rename and copy entries with correct new and old paths.
- Newline-containing paths on Unix.
- `git changed` and `ctx changed` produce matching path/status data.

### Search

- Hidden files, `.ignore`, `.gitignore`, nested negation, global/exclude behavior where isolated, caller globs, and symlink policy.
- Results do not depend on whether `rg` is installed.

### GitLab

- Default, standard custom `/api/v4`, arbitrary REST proxy, and explicit GraphQL URLs.
- Unknown issue, comment, and design fields deserialize successfully but are absent from public JSON.
- Existing full-view success and best-effort warning behavior remains covered.

### Verification order

1. Focused unit tests for bounded readers, porcelain parsing, and URL normalization.
2. Integration tests for run, HTTP, Git, ctx, and search.
3. GitLab plugin tests.
4. `cargo fmt --all -- --check` executed through `ah`.
5. `cargo test --workspace --all-targets --locked` executed through `ah`.

## Rollout and Compatibility

The changes ship together because the review findings are independent but share one compatibility goal: bounded and deterministic agent-facing behavior. No data migration is required. Documentation for run, HTTP, search, Git, ctx, and GitLab is updated in the same implementation.

The existing uncommitted GitLab implementation and documentation changes are preserved and extended rather than reverted.
