# 05 — Module Boundaries and God Files

**Severity: High.** Several modules hold three or four unrelated responsibilities.
Each one is a merge-conflict magnet and a place where a reviewer cannot hold the
whole file in their head.

## Findings

### 5.1 `crates/ah-mcp/src/server.rs` — 3 991 lines, at least seven jobs

| Responsibility | Evidence |
|---|---|
| MCP protocol handler | `impl ServerHandler for McpServer:840` |
| Streamable HTTP transport + lifecycle | `HttpLifecycleState:914`, `HttpLifecycleController:955` |
| Shutdown protocol and tracker | `ShutdownTracker:907`, `ShutdownReader:1056` |
| Local-request authentication | `validate_local_headers:1680` |
| **A full HTML/CSS web UI for secret setup** | `SETUP_PAGE_STYLE` (~30 lines of CSS), `setup_page:1519`, `render_secret_setup_form:1530`, `render_secret_setup_success:1569`, `html_escape:1611` |
| Plaintext-credential detection and redaction | `validate_mcp_plaintext_auth:1834`, `redact_curl_user_options:1743` |
| Job registry glue, catalog snapshots, error mapping | `job_tools:1726`, `build_catalog_snapshot:2610`, `runtime_command_error:2453` |

A browser-facing HTML form living inside the MCP protocol adapter is the clearest
boundary violation in the codebase. It also means the CSP/nonce/`no-store` security
work (`no_store:1443`, `page_nonce:1467`) is reviewed as part of protocol changes.

**Split into:** `mcp::protocol`, `mcp::transport::{stdio,http}`, `mcp::shutdown`,
`mcp::jobs`, `mcp::mapping` — and move the setup UI out entirely, into
`ah-setup-ui` (or serve it from a dedicated small crate), since it is a web
application, not an MCP concern.

### 5.2 `src/commands/http/domain.rs` — 1 983 lines, a test framework in disguise

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

**Split into:** `http::client`, `http::curl` (parser), `http::jsonpath`,
`http::assert` (DSL + evaluation), `http::spec` (runner + reporters).
Consider whether `jsonpath` should be a dependency rather than an implementation.

### 5.3 `src/event_log.rs` — logging plus a redaction engine *(engine extracted)*

The redaction engine (URL/userinfo/header/curl/JSON/percent-decode heuristics) has
moved to `crates/ah-redact`, taking the file from 1 845 to 1 195 lines. What is
left still mixes three concerns — the logger, record bounding/compaction, and
rotation with file locking — and splits cleanly into `event_log::sink` and
`event_log::rotation`.

### 5.4 `src/mcp_service/lifecycle.rs` — 2 029 lines

`LifecycleService<S, R>` (`:191`) is well-designed, but the module also holds the
CLI entry point (`execute:44`), six update-integration helpers
(`capture_for_update_while_locked:129`, `restore_for_update_while_locked:152`, …),
drift reduction (`reduce_runtime:1929`), and status projection
(`apply_scheduler_section:1895`). The update-integration helpers are the coupling
described in group 07 and should move behind a trait.

### 5.5 `src/commands/ctx_symbols.rs` — 889 lines of hardcoded language support

31 per-language extractor functions (`extract_rust_symbols:73` …
`extract_taskfile_symbols:505`) plus ~60 `OnceLock<Regex>` accessors, one function
per regex. Adding a language means editing a dispatch match, writing an extractor,
and adding two to four regex functions.

**Target:** a static table.

```rust
struct LanguageSpec {
    extensions: &'static [&'static str],
    filenames: &'static [&'static str],
    patterns: &'static [(&'static str, &'static str)], // (regex, symbol kind)
}
static LANGUAGES: &[LanguageSpec] = &[ /* … */ ];
```

One generic extractor walks lines against the specs. Adding a language becomes one
table entry. The natural next step — swapping the regex engine for tree-sitter
grammars — then touches one function instead of thirty-one.

### 5.6 `src/commands/project/rules.rs` — a 550-line `match` that is really a table

`classify_file:24` is a single match over filenames and path fragments producing
`FileRuleDetection` records. It is pure data expressed as control flow. Move it to
a static table (or an embedded TOML/JSON asset validated at build time) plus a small
matcher. Community contributions ("detect Bazel", "detect uv") then become data
changes, reviewable without reading Rust.

### 5.7 `src/ai/install.rs` — 1 367 lines mixing four layers

Business logic (install/uninstall/status), a hand-rolled thread pool
(`parallel_map:818`, `std::thread::scope`), a live-updating terminal UI
(`render_live_lines:897`, `write_live_frame:963`, `emit_live_status:973`), and
network probing (`probe_readiness:406`). Split: `ai::install` (logic, returns
events), `ai::progress` (renderer consuming events), and reuse a shared parallel
helper rather than a local one.

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

1. `ctx_symbols` → table-driven (self-contained, well covered by existing tests).
2. `project/rules` → table-driven (same).
3. ~~`event_log` → extract `ah-redact`~~ **(done)**; still to do: split sink/rotation.
4. `http/domain` → extract `curl`, `jsonpath`, `assert`, `spec` as sibling modules;
   add focused unit tests and a fuzz target for the two parsers.
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
- **`ctx_symbols` output ordering is user-visible.** The generic extractor must
  preserve the current per-language pattern order, which the table encodes explicitly.

## Acceptance criteria

- No source file in the workspace exceeds ~800 lines excluding tests.
- Adding a language to `ctx_symbols` or a file type to `project/rules` is a
  one-entry data change.
- No HTML or CSS in `crates/ah-mcp`.
- One documented module layout under `src/commands/`, with no alias shims.
