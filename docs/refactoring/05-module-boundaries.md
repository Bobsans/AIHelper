# 05 — Module Boundaries and God Files

**Severity: High.** Several modules hold three or four unrelated responsibilities.
Each one is a merge-conflict magnet and a place where a reviewer cannot hold the
whole file in their head.

## Findings

### 5.1 `crates/ah-mcp/src/server.rs` — 3 991 lines, at least seven jobs *(done)*

| Responsibility | Evidence |
|---|---|
| MCP protocol handler | `impl ServerHandler for McpServer:840` |
| Streamable HTTP transport + lifecycle | `HttpLifecycleState:914`, `HttpLifecycleController:955` |
| Shutdown protocol and tracker | `ShutdownTracker:907`, `ShutdownReader:1056` |
| Local-request authentication | `validate_local_headers:1680` |
| ~~**A full HTML/CSS web UI for secret setup**~~ | *moved to `ah-setup-ui`* |
| Plaintext-credential detection and redaction | `validate_mcp_plaintext_auth:1834`, `redact_curl_user_options:1743` |
| Job registry glue, catalog snapshots, error mapping | `job_tools:1726`, `build_catalog_snapshot:2610`, `runtime_command_error:2453` |

A browser-facing HTML form living inside the MCP protocol adapter is the clearest
boundary violation in the codebase. It also means the CSP/nonce/`no-store` security
work is reviewed as part of protocol changes.

**Status: the UI is out.** `ah-setup-ui` owns the pages, the CSP and nonce, the
`no-store`/referrer headers, the HTML escaping, the `Accept` negotiation, and the
contract types the vault implements — with the four tests that cover them. The
adapter keeps routing and the local-request policy, which is transport, and
re-exports the contract types so callers still see one surface.

`axum` moved to `[workspace.dependencies]`, since two members need it now.

**Status: split.** Six modules where there was one file:

| Module | Production lines | Owns |
|---|---|---|
| `server` | 833 | `McpServer`, the `ServerHandler` impl, tool dispatch, catalog snapshots |
| `transport` | 570 | stdio and HTTP serving, readiness, control and setup routes, the local-request policy |
| `mapping` | 503 | catalog commands, typed responses and errors into MCP types |
| `job_tools` | 184 | the `ah.job.*` tool schemas, arguments and results |
| `shutdown` | 144 | the shutdown request, the tracker, the stdin reader |
| `plaintext_auth` | 122 | refusing and scrubbing credentials pasted into tool arguments |

`server.rs` is 833 production lines, marginally over this phase's criterion.

**Not done: the test module.** 1 305 lines of tests still sit in `server.rs` and
reach into all five modules, which is why several fields are `pub(crate)` rather
than private. They share one fixture set, so splitting them is its own job rather
than a tail of this one.

### 5.2 `src/commands/http/domain.rs` — 1 983 lines, a test framework in disguise *(done)*

It contains, in one file: an HTTP client with retry and deadline handling
(`send_with_retry:309`), **a curl command-line parser** (`parse_curl_replay:1086`),
**a JSONPath implementation** (`parse_json_path_tokens:918`, `resolve_json_path:897`),
**an assertion DSL** (`parse_status_expectation:775`,
`parse_json_expectation_expression:828`, `evaluate_assertions:662`), **a spec-file
test runner** with variable interpolation (`interpolate_string:1721`,
`build_case_request:1274`), and **JUnit XML emission** (`xml_escape:1808`).

These are four separately valuable, separately testable libraries. As one module
they cannot be fuzzed independently, and the parsers — which consume untrusted
input — are buried where they get no focused review.

**Status: split.** `domain/{assert,curl,jsonpath,spec}.rs` sit beside a `domain.rs`
that keeps the client and the request-building helpers the others share. The
modules nest under `domain` rather than under `http` so the domain/adapter line
stays where it is; finding 5.8 reorganises `commands/*` layout as a whole.

`run_assert` moved into `spec` with the runner it drives — leaving it in
`domain.rs` would have meant publishing most of the spec types.

The parsers now have generated-input coverage: a fixed-seed corpus over an
alphabet built from the delimiters each one slices on, asserting no panic and
bounded output. It earned itself on first run by finding that `curl ''` parsed
into an empty URL, which surfaced downstream as "url must not be empty" — a
message that does not say which argument was wrong. The parser rejects it now.

`cargo-fuzz` is still the right tool and is still deferred for the reason the
roadmap gives: nightly plus a CI lane of its own. What changed is that the
parsers are now reachable as units, which was the blocker.

`spec.rs` is 869 lines, over this phase's criterion. It is the runner, the case
format, interpolation, extraction and the JUnit reporter; the reporter is the
obvious next split.

Consider whether `jsonpath` should be a dependency rather than an implementation.

### 5.3 `src/event_log.rs` — logging plus a redaction engine *(done)*

The redaction engine (URL/userinfo/header/curl/JSON/percent-decode heuristics)
moved to `crates/ah-redact` first, taking the file from 1 845 to 1 195 lines. The
three concerns that were left are now three modules:

| Module | Production lines | Owns |
|---|---|---|
| `event_log` | 430 | the logger, the record shape, the `EventSink` impl |
| `event_log::record` | 130 | bounding a record to its line budget, compaction, the minimal fallback |
| `event_log::rotation` | 97 | the day's file name, the advisory lock with bounded retry, cleanup |

The proposed name was `event_log::sink`; `record` says what it actually does,
since the sink is the logger and what moved is record shaping.

Most of the test module stays with the logger: it is dominated by redaction and
end-to-end cases that go through `EventLogger`, including the two cleanup tests,
which build a real log directory through it. Only the one test that touches
nothing but `minimal_record` moved.

### 5.4 `src/mcp_service/lifecycle.rs` — 2 029 lines

`LifecycleService<S, R>` (`:191`) is well-designed, but the module also holds the
CLI entry point (`execute:44`), six update-integration helpers
(`capture_for_update_while_locked:129`, `restore_for_update_while_locked:152`, …),
drift reduction (`reduce_runtime:1929`), and status projection
(`apply_scheduler_section:1895`). The update-integration helpers are the coupling
described in group 07 and should move behind a trait.

### 5.5 `src/commands/ctx_symbols.rs` — 889 lines of hardcoded language support *(done)*

31 per-language extractor functions plus 61 `OnceLock<Regex>` accessors, one
function per regex. Adding a language meant editing a dispatch match, writing an
extractor, and adding two to four regex functions.

**Status:** `LANGUAGES` is now a static table of 28 specs and 62 patterns, and
one generic extractor walks lines against it. Adding a language is one entry.

The shape needed two things the sketch did not have. A pattern's *kind* is
sometimes a fixed label and sometimes a capture — one row covers
`struct`/`enum`/`trait` rather than three — so it is `Kind::Fixed` or
`Kind::Captured`. A pattern's *name* is usually one group but is assembled from
optional groups for Terraform (`resource "aws_s3_bucket" "logs"` is one name)
and Dockerfile (a stage is named by its `AS` alias or else its image), so it is
`Name::Capture`, `Name::Dotted` or `Name::Preferred`. Both enums exist because
current behavior requires them, not in anticipation.

Two extractors stay functions: Markdown counts heading depth rather than
capturing it, and the fallback for an unrecognised file has no regex at all.

Pattern order is user-visible and the table states it explicitly; the generic
extractor keeps first-match-wins per line.

Regexes are compiled once into a `LazyLock`, preserving the caching the
per-regex `OnceLock` accessors used to provide.

891 lines to 788 - a smaller cut than it looks, because the regexes themselves
are most of the file and did not go anywhere. The win is that they are now data
in one place instead of 61 accessor functions and 31 dispatch bodies. The
natural next step, swapping the regex engine for tree-sitter grammars, touches
one function.

**The safety net came first.** The module had no unit tests, so the conversion
is backed by a golden snapshot of 92 symbols across a corpus with one fixture
per dispatch arm, taken before any edit. Coverage of that corpus was checked
mechanically: every one of the 61 regexes matches at least one fixture line, and
a unit test now fails if a table row is added that no fixture reaches.

### 5.6 `src/commands/project/rules.rs` — a 550-line `match` that is really a table *(done)*

`classify_file` was one match over 95 file names followed by 26 `if` blocks over
suffixes and path fragments - pure data expressed as control flow.

**Status:** 121 `rule(when, detection)` entries plus a small matcher. Conditions
are a closed set - name, name prefix, name suffix, path, path prefix, path
fragment, `Any`, `All` - which is exactly what the old code tested and nothing
more. Adding a file type is one line.

**Caveat, stated rather than hidden:** the file is now 1 068 lines, over this
phase's ~800 line criterion. It grew because rustfmt gives a rule one to four
lines where the old `match` arm took one, not because anything was added: there
is no control flow left in it at all. Closing the gap for real means the other
half of this finding - moving the table to an embedded asset validated at build
time - which also gets the "reviewable without reading Rust" property that a
Rust table only approximates.

### 5.7 `src/ai/install.rs` — 1 367 lines mixing four layers *(done)*

Business logic (install/uninstall/status), a hand-rolled thread pool, a
live-updating terminal UI, and network probing, in one file. Now three:

| Module | Production lines | Owns |
|---|---|---|
| `ai::install` | ~1 260 | install, uninstall, status, path resolution, probing |
| `ai::progress` | ~400 | the live screen: cursor control, frame clipping, column widths |
| `ai::output` | ~180 | rendering a finished report, and the action label/style vocabulary |

`StatusProgress` — the event the logic emits and the screen consumes — stays
with the producer. The renderers are the consumers.

Two corrections to this finding. There is no shared parallel helper to reuse:
`parallel_map` is the only one in the workspace and has one caller, so it stays
where it is. And the "four layers" included the report renderers, which were
still calling `println!` directly; they now go through `Emitter`, which closes
the last thirteen print sites outside finding 4.2's live frames and 4.3's error
rendering.

### 5.8 Inconsistent and ceremonial layering in `src/commands/`

The intended pattern is `domain.rs` (pure) + `io.rs` (effects) + `output.rs`
(rendering). Actual state:

- `ctx`, `file`, `git`, `run`, `search`, `task` use flat `io.rs`/`output.rs`.
- `http`, `project` use an `adapters/` subdirectory for the same thing.
- `secrets.rs` (489 lines) and `ctx_symbols.rs` (889) do not follow the pattern at all.
- Eight modules contain a no-op alias module:
  ```rust
  mod adapters {
      pub(crate) use super::io;
      pub(crate) use super::output;
  }
  ```
  (`ctx.rs:109`, `file.rs:104`, `git.rs:95`, `http.rs:18`, `project.rs:15`,
  `run.rs:54`, `search.rs:74`, `task.rs:66`) — pure ceremony that makes two layouts
  look identical instead of making them identical.

**Target:** one layout, enforced. Either everything uses `adapters/{io,output}.rs`
or everything uses flat files; delete the alias shims; bring `secrets` and
`ctx_symbols` into the pattern. Document the chosen layout in
`docs/developers/architecture.md` and add a directory-shape test if it keeps drifting.

## Why it hurts

- Reviewers cannot reason about a 4 000-line file; security-relevant code hides in it.
- Parallel work on unrelated features collides in the same file.
- The inconsistent layering means every contributor has to learn the exception list.
- Language and file-type support — the parts most likely to receive outside
  contributions — are the parts that require the most Rust knowledge to extend.

## Migration

Ordering is chosen so that each split is mechanical and low-risk:

1. ~~`ctx_symbols` → table-driven (self-contained, well covered by existing tests).~~
   **(done)** — and it was *not* covered: the module had no unit tests, and the
   integration tests assert only that a few expected symbols are present for
   seven of the twenty-eight languages. A characterization snapshot was taken
   first.
2. ~~`project/rules` → table-driven (same).~~ **(done)** — and, as with
   `ctx_symbols`, it was not covered either: a characterization snapshot of 208
   paths was taken first.
3. ~~`event_log` → extract `ah-redact`~~ **(done)**; still to do: split sink/rotation.
4. ~~`http/domain` → extract `curl`, `jsonpath`, `assert`, `spec` as sibling modules;
   add focused unit tests and a fuzz target for the two parsers.~~ **(done, with
   generated-input tests standing in for the fuzz target)**
5. `ah-mcp/server` → move the setup UI out first (largest, most orthogonal chunk),
   then split transport/shutdown/protocol.
6. `ai/install` → separate progress rendering from logic.
7. `mcp_service/lifecycle` → move update integration behind the trait from group 07.
8. Unify `commands/*` layout; delete the eight `mod adapters` shims.

## Risks and invariants

- **Splitting must not move behavior.** Each step should be a pure move plus
  `pub(crate)` visibility adjustments; assert with `cargo test --workspace` and the
  golden snapshots from group 01/04.
- **The setup UI has security properties** (nonce, CSP, `no-store`, referrer policy).
  Move the tests with it and keep them passing at every commit.
- ~~**`ctx_symbols` output ordering is user-visible.**~~ **(held)** — the table
  encodes pattern order explicitly and the extractor keeps first-match-wins; the
  golden snapshot is byte-identical across the conversion.

## Acceptance criteria

- No source file in the workspace exceeds ~800 lines excluding tests.
- Adding a language to `ctx_symbols` or a file type to `project/rules` is a
  one-entry data change.
- No HTML or CSS in `crates/ah-mcp`.
- One documented module layout under `src/commands/`, with no alias shims.
