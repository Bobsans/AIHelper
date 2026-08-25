# 04 — Output and Error Model

**Severity: High.** Presentation is fused to the process (`stdout`), and the same
error information is modelled three times with hand-written translation tables
between them.

## Findings

### 4.1 Rendering writes directly to the process, not to a sink *(resolved for every adapter)*

There were 177 print macro calls in `src/`. Most were correctly confined to
`*/output.rs` adapters — the convention was right — but the adapters printed to
the process rather than to an injected writer, so `--quiet` was re-checked by
hand at the top of every `emit` function and no output could be asserted without
spawning a subprocess.

**Status:** `Emitter` in [`src/output.rs`](../../src/output.rs) is the sink, and
every command adapter goes through it. 177 print calls are 23, and none of the
remainder is a command result:

| Holdout | Sites | Why it is still there |
|---|---|---|
| `src/ai/install.rs` | 13 | live terminal frames with cursor control — finding 4.2 |
| `src/output.rs` | 3 | `emit_warning`/`emit_muted_stderr`, process-level notices outside any command's output contract |
| `src/error.rs` | 3 | `AppError::print` — finding 4.3 |
| `src/runtime_flow.rs` | 2 | update-recovery notices, emitted before a command exists |
| `src/bin/ah-mcp-service.rs`, `src/ai/prompt.rs` | 2 | a service entry point and an interactive prompt |

`--quiet` is now a property of the sink: the ~20 hand-written
`if options.quiet { return Ok(()); }` guards in adapters are gone, and a new
adapter cannot forget the check because it never sees the flag. Output is
assertable in-process — see the `Emitter::capture` tests in
[`src/commands/git/output.rs`](../../src/commands/git/output.rs).

### 4.2 `src/ai.rs` and `src/ai/install.rs` bypass the layering

Unlike the `commands/*` domains, the `ai` domain prints from business logic:
`install.rs` renders live terminal frames (`render_live_lines:897`,
`write_live_frame:963`, `emit_live_status:973`) interleaved with installation work,
inside the same 1 367-line module that performs config-file mutation and HTTP
readiness probing. Cursor control and redraw logic sit next to registrar mutation.

### 4.3 The error type knows how to paint a terminal

[`src/error.rs:123`](../../src/error.rs) `AppError::print()` and
`console_diagnostic` (`:441`) put ANSI styling and layout inside the error enum.
An error value is data; how it is shown is a policy of the presentation layer.
This is why `error.rs` is 1 113 lines for 15 variants.

### 4.4 Three error taxonomies, two hand-written translation tables *(consolidated — see status)*

| Layer | Type | Variants |
|---|---|---|
| runtime | `RuntimeError` (`crates/ah-runtime/src/lib.rs:32`) | ~28 |
| host | `AppError` (`src/error.rs:9`) | 15 |
| wire | `CommandError` / `ErrorDiagnostic` (`crates/ah-plugin-api/src/lib.rs:607`, `:227`) | structured |

The translations are manual and duplicated:

- [`src/lib.rs:390`](../../src/lib.rs) `map_runtime_error` — 133 lines mapping 28
  variants into `AppError::external(code, message)`.
- [`crates/ah-mcp/src/server.rs:2453`](../../crates/ah-mcp/src/server.rs)
  `runtime_command_error` — maps the *same* 28 variants again, for MCP.
- [`crates/ah-mcp/src/server.rs:2553`](../../crates/ah-mcp/src/server.rs)
  `runtime_error_code` — a *third* pass over the same variants, for the code string.

Adding one `RuntimeError` variant means editing three match statements in two
crates, and the compiler only catches two of them if the matches are exhaustive.

`AppError::External { code: String, message: String }` is the escape hatch that
absorbs everything — which means the host error type has effectively degenerated
into a stringly-typed pair, while still carrying 14 structured variants for
filesystem errors.

**Status:** the three tables are one. `RuntimeError::describe` in `ah-runtime` is
now the only exhaustive match over the variants; `RuntimeError::diagnostic()` and
`RuntimeError::command_error()` project from it, exposed as
`From<RuntimeError> for ErrorDiagnostic` and `From<RuntimeError> for CommandError`.
`map_runtime_error` is four lines, `runtime_command_error` and `runtime_error_code`
are gone, and adding a variant now breaks exactly one match, in `ah-runtime`.

Two things §B below did not anticipate, and which the table records explicitly
rather than flattening:

- The two surfaces froze **different code strings** for the same five variants
  (`EXECUTION_CANCELLED`/`CANCELLED`, `EXECUTION_TIMEOUT`/`TIMEOUT`,
  `EXECUTION_HANDLER_PANIC`/`HANDLER_PANIC`, `TYPED_COMMAND_NOT_FOUND`/`COMMAND_NOT_FOUND`,
  `PLUGIN_RESPONSE_PARSE_FAILED`/`PLUGIN_RESPONSE_INVALID`). Both sets are public,
  so neither could move; the table carries the host code and an MCP override.
- **`retryable` is not a property of the error**, it is a property of the MCP
  transport, so it stays off `ErrorDiagnostic` and is applied when projecting to
  `CommandError`. `exit_code_hint` is likewise per-surface: the host reports 1
  because `AppError::exit_code` is always 1, MCP keeps its 2 for the
  not-found/disabled cases.

The consolidation also closed a leak: the old MCP fallback arm used
`RuntimeError`'s `Display` as the `cause`, so credential ids reached MCP clients
for `SecretNotFound`, `SecretKindMismatch`, `VaultLocked` and `VaultKeyUnavailable`.
Redaction was only ever implemented on the CLI arm; it now lives in the shared
table and covers both surfaces.

What remains: `AppError::External` is still the universal fallback for
non-runtime errors, and `AppError` still owns its own 15-variant rendering — see
4.3 and step 6 below.

### 4.5 Error values are large enough to be suppressed rather than fixed *(resolved)*

`#![allow(clippy::result_large_err)]` appears at the crate root of `src/lib.rs`,
`crates/ah-mcp/src/server.rs`, and all four plugins, plus six targeted `#[allow]`s
in `ah-plugin-api`. The lint is correct: `AppError` embeds `ErrorDiagnostic`
(five `String`s plus options) and `Box<AppError>`, and `InvocationResponse` is
returned by value from every plugin entry point. Every `Result` in the codebase
pays that size. The allow silences the signal instead of boxing the payload.

**Status:** all thirteen allows are gone. Measured with the lint enabled, 625
call sites were over the 128-byte threshold. Boxing the two payloads that were
genuinely oversized - `AppError::Diagnostic` and `InvocationResponse::diagnostic`,
each inlining a whole `ErrorDiagnostic` - removed 599 of them. The remaining 26
are `CommandError` and `McpAdapterError` at exactly 128 bytes, which have no
large element inside to box: they are flat wire structs of required strings.
Those get a documented 136-byte budget in `clippy.toml` instead, so growth beyond
today's shape is still caught.

### 4.6 Redaction is implemented three times *(engine extracted — see status)*

Secret redaction — the most security-sensitive cross-cutting concern here — has
three independent implementations:

- [`src/cli.rs`](../../src/cli.rs) `redact_secret_command_argv` (argv before logging)
- [`src/event_log.rs:448–1000`](../../src/event_log.rs) — ~550 lines: URL userinfo,
  header-like strings, curl `-u`/`--user`, embedded assignments, JSON values,
  sensitive-name heuristics, percent-decoding
- [`crates/ah-mcp/src/server.rs:1973`](../../crates/ah-mcp/src/server.rs)
  `redact_mcp_plaintext_auth` plus `curl_contains_auth:1921`,
  `url_contains_userinfo:1963`, `is_authorization_header:1915`

Three code paths, three sets of heuristics, one shared risk: a secret leaking
through whichever path was not updated.

**Status:** the engine and the detectors now live in `crates/ah-redact`, with the
event-log rewriters and the MCP detectors moved there verbatim and covered by a
dedicated test suite. What remains is the *policy* layer: the three callers still
decide independently which fields to look at. That is a semantic merge, not a
move, so it stays out of the safety-net phase — see step 1 below.

## Why it hurts

- Presentation cannot be tested without a process; the test suite is slow and
  coarse (group 08).
- The MCP transport and the CLI transport render errors through separate,
  divergent tables; the same failure can present differently in each.
- The redaction split is a security defect waiting to happen: any new sink
  (a future HTTP transport, a new log format) starts from zero.

## Target design

### A. One `Emitter` abstraction *(done)*

Shipped as `Emitter` rather than `Emitter<W>`: it needs two sinks, stdout and
stderr, which are different types, and boxing them costs nothing at terminal
output rates while keeping the type out of every adapter signature.

```rust
impl Emitter {
    pub fn stdio(options: &GlobalOptions) -> Self;
    pub fn capture(options: &GlobalOptions) -> (Self, Captured);

    pub fn value<T: Serialize + ?Sized>(&mut self, json: &T, text: impl FnOnce(TextFormatter) -> String) -> Result<(), AppError>;
    pub fn report(&mut self, render: impl FnOnce(TextFormatter) -> Result<String, AppError>) -> Result<(), AppError>;
    pub fn raw(&mut self, text: &str) -> Result<(), AppError>;
    pub fn raw_err(&mut self, text: &str);
    pub fn warning(&mut self, message: impl Display);
    pub fn text_warning(&mut self, message: impl Display);
    pub fn muted(&mut self, message: impl Display);
}
```

- `quiet` is enforced once, inside the emitter.
- Text and JSON rendering stay side by side, which is what keeps them consistent.
- Tests render into buffers; no subprocess required.
- `report` exists for output whose format the command chose itself — `--report
  junit` — which the global mode does not describe.
- `raw`/`raw_err` pass a child process's captured bytes through unchanged, since
  reformatting them would break whatever the caller pipes them into.
- `text_warning` is the one that accompanies text output only: in JSON mode the
  payload already carries `truncated: true`, so repeating it on stderr is noise a
  machine reader has to filter.
- An empty rendering writes nothing rather than a blank line, which is what the
  hand-written `if !items.is_empty()` guards used to do.
- The MCP path can reuse the same renderers for the human-readable `text` field of
  a typed response instead of maintaining separate `*_result_text` helpers.

### B. One diagnostic currency

`ErrorDiagnostic` already exists in `ah-plugin-api` and already crosses the ABI.
Make it the single carrier:

- `RuntimeError` and `AppError` keep their variants but each gains
  `fn diagnostic(&self) -> ErrorDiagnostic` (`AppError` already has one at
  `src/error.rs:423`) — and *that* is the only conversion anyone writes.
- Delete `map_runtime_error`, `runtime_command_error`, `runtime_error_code`;
  replace with `impl From<RuntimeError> for ErrorDiagnostic` in `ah-runtime`,
  owned by the crate that owns the variants. Adding a variant then breaks exactly
  one exhaustive match, in the right crate.
- `AppError::External` shrinks back to a genuine "foreign error" case rather than
  the universal fallback.

### C. Rendering moves out of the error type

`AppError::print` and `console_diagnostic` move to
`presentation::render_error(&ErrorDiagnostic, &mut Emitter)`. `error.rs` should
end up around 300 lines.

### D. Box the error payloads

Change `Result<T, AppError>` payloads to `Box`-ed inner data (or reduce
`ErrorDiagnostic` to `Box<DiagnosticInner>`), then delete every
`allow(clippy::result_large_err)` and enable the lint as a denial.

### E. One redaction engine, thin policies

`ah-redact` owns the engine and the detectors; each sink keeps only its own
policy — which fields it inspects and whether it rewrites or rejects. The three
policies should converge on a shared field vocabulary rather than three private
lists of names, and the crate is the right place for the property tests (already
added) and a future fuzz target (group 08).

## Migration

1. ~~Extract `ah-redact`~~ **(done)** — the engine, the detectors, the marker and
   the bounds moved as-is; `event_log.rs` went from 1 845 to 1 195 lines and the
   MCP adapter no longer defines its own credential detectors. Next: converge the
   three field-selection policies onto one vocabulary.
2. ~~Introduce `Emitter`; migrate one domain (`git`, the largest output surface) and
   convert its output tests from `assert_cmd` to in-process assertions.~~ **(done, and
   every other adapter with it — the shape repeated exactly, so stopping at one
   domain would have left the duplication in place for no gain)**
3. Migrate the remaining domains, then `src/ai.rs` and `mcp_service/output.rs`.
4. Separate `ai/install.rs` live rendering into a `progress` module that writes
   through the emitter; installation logic returns events, the renderer consumes them.
5. ~~Add `From<RuntimeError> for ErrorDiagnostic`; delete the three mapping functions~~
   **(done)** — see the status note under 4.4.
6. Move error rendering out of `error.rs`.
7. Box error payloads; remove the `result_large_err` allows; add `-D warnings` to CI
   (group 09).

## Risks and invariants

- **Text output must remain byte-identical**, including ANSI placement. The
  existing tests in `src/output.rs` and `src/lib.rs` (`plugins_table_applies_styles_after_padding`)
  show the project already treats this as a contract — extend that pattern to every
  renderer *before* migrating it.
- **Error codes are part of the public contract** (`DOMAIN_DISABLED`,
  `SECRET_NOT_FOUND`, `OUTPUT_SCHEMA_VIOLATION`, …). The consolidation must preserve
  every code string exactly; snapshot them in a test.
- **Redaction extraction must not weaken any heuristic.** Port tests first, then code.

## Acceptance criteria

- No `println!` outside the emitter implementation.
- `--quiet` is handled in exactly one place.
- One conversion path from each error enum to `ErrorDiagnostic`; no duplicated
  match statements across crates.
- `clippy::result_large_err` is enabled, not allowed.
- Exactly one redaction implementation, with property and fuzz coverage.
