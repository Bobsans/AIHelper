# 02 — Plugin Duplication and the Missing Plugin SDK

**Severity: Critical.** `crates/ah-plugin-api` defines the *contract* but provides
almost no *implementation support*. Every plugin therefore rebuilds the same
infrastructure, and the two SCM plugins are near-clones of each other.

## Findings

### 2.1 GitHub and GitLab plugins are structurally identical

| | GitHub | GitLab |
|---|---|---|
| `src/lib.rs` | 3 870 lines | 3 885 lines |
| `src/typed.rs` | 1 924 lines | 1 425 lines |

They implement the same domain concept — a hosted forge with issues, comments,
releases, CI runs and logs — with parallel, separately written code:

| Concern | GitHub | GitLab |
|---|---|---|
| credential authority parsing | `lib.rs:1720` `credential_authority` | `lib.rs:1628` |
| HTTPS authority extraction | `lib.rs:1729` `https_authority` | `lib.rs:1633` |
| loopback detection | `lib.rs:1742` `is_loopback_authority` | `lib.rs:1646` |
| git remote authority | `lib.rs:1753` `remote_authority` | `lib.rs:1657` |
| env token lookup | `lib.rs:1767` `env_token` | `lib.rs:1671` |
| `git credential fill` driver | `lib.rs:1774` `git_credential_token` | `lib.rs:1678` |
| bounded credential child wait | `lib.rs:1805` `wait_for_credential_child` | `lib.rs:1709` |
| JSON request wrapper | `lib.rs:1833` `github_json` | `lib.rs:1737` `gitlab_json` |
| response/error mapping | `lib.rs:1861` `github_response` | `lib.rs:1787` |
| log/trace fetch with byte caps | `lib.rs:1902`, `lib.rs:2001` | `lib.rs:1827` |
| ANSI stripping | `lib.rs:2364` | `lib.rs:2475` |
| error truncation | `lib.rs:2357` | `lib.rs:2468` |
| warning-line heuristic | `lib.rs:2091` | `lib.rs:2072` |
| success rendering | `lib.rs:2099` `render_success` | `lib.rs:2087` |
| state/status styling | `lib.rs:2305–2349` | `lib.rs:2428–2460` |
| manual example builder | `lib.rs:2556` | `lib.rs:2620` |

Two independent copies means two places to fix every credential-handling bug —
and credential handling is exactly the code that must not diverge.

### 2.2 Cross-cutting concerns are copy-pasted, not provided

**Cancellation** *(resolved: one implementation in `ah_plugin_api::cancellation`)*
was implemented five times, each with its own process-global
`static Mutex<HashSet<String>>` plus a thread-local current-request marker:

- [`src/commands/run.rs:200`](../../src/commands/run.rs) `cancellation_requests`
- [`src/commands/search.rs:201`](../../src/commands/search.rs)
- [`src/commands/task.rs`](../../src/commands/task.rs) (same shape)
- [`plugins/ah-plugin-github/src/typed.rs:22`](../../plugins/ah-plugin-github/src/typed.rs) `CancellationState`
- and the matching `cancelled_response` / `current_request_cancelled` pairs
  (5 definitions of `current_request_cancelled`, 3 of `cancellation_requests`)

Cancellation is a *runtime* responsibility. Today the runtime exposes
`cancel_typed(request_id)` ([`crates/ah-runtime/src/lib.rs:160`](../../crates/ah-runtime/src/lib.rs))
and then every implementer has to invent the plumbing behind it.

**Other duplicated primitives across the workspace:** `manual_example` (9),
`normalize_path` (9), `truncate_for_error` (4), `render_success` (4),
`strip_ansi_sequences` (2), `paint_if_present` (2), `input_schema` (2).

### 2.3 Test hooks live in the production binary *(resolved, and the finding was overstated)*

Three environment-variable seams were listed. Reading them again, only one was real:

| Seam | Where it was read | Verdict |
|---|---|---|
| `AH_GITHUB_TEST_CREDENTIAL_SLEEP` | inside `#[cfg(test)] mod tests` | never compiled into the cdylib |
| `AH_GITLAB_TEST_CREDENTIAL_SLEEP` | inside `#[cfg(test)] mod tests` | never compiled into the cdylib |
| `AH_POSTGRES_TEST_SYSTEM_PATH` | `find_psql_in_path`, production | **shipped** |

The two credential seams are a test re-executing its own test binary and telling the
child to sleep, which is what makes the timeout observable. They do not ship and they
do not need an abstraction.

`AH_POSTGRES_TEST_SYSTEM_PATH` did ship, and it short-circuited `PATH` resolution for
`psql` — an untracked environment variable choosing which executable runs. It is
deleted. No injectable process runner was needed to delete it: **no test referenced
it**. The seam outlived whatever test it was cut for, and removing it removed a
`PATH`-override primitive from the shipped plugin.

### 2.4 Text rendering is re-invented per plugin

`ah-plugin-api` provides `TextFormatter`/`TextStyle` but no table or list renderer,
so every plugin writes its own `render_*_text` family (GitHub: 9 renderers,
GitLab: 11, Postgres: 15). The host has the same problem
(`render_plugins_table` in [`src/lib.rs:271`](../../src/lib.rs) hand-computes column
widths). Column alignment logic exists in at least four places.

## Why it hurts

- Fixing a credential bug requires finding all copies; the review burden scales with
  the number of plugins, defeating the point of having a plugin architecture.
- A third-party plugin author must reimplement HTTP retries, pagination, bounded
  bodies, credential resolution, cancellation and rendering before writing a single
  feature — so third-party plugins will be low quality or will not exist.
- Two of the copies (credentials, redaction) are security-relevant.

## Target design

### A. `ah-plugin-sdk` crate

New crate depending on `ah-plugin-api`, linked by every plugin (and by the built-in
domains, which have the same needs):

| Module | Provides |
|---|---|
| `sdk::http` | blocking JSON client: retry policy, deadline, pagination, bounded response bodies, uniform error→`CommandError` mapping |
| `sdk::credentials` | authority parsing, loopback checks, `git credential fill` with bounded child wait, env fallback, ambient-token policy |
| ~~`sdk::cancel`~~ | *done, as `ah_plugin_api::cancellation` — see B* |
| `sdk::render` | table/list/keyvalue renderers built on `TextFormatter`, ANSI stripping, truncation |
| `sdk::descriptor` | descriptor/manual builders (feeds group 01) |
| ~~`sdk::process`~~ | *dropped: the one shipped seam it was for is deleted, and nothing else asks for it* |

### B. Cancellation belongs to one module *(done)*

**Status:** the roadmap called for a token in the request context. That is not
reachable: the host cancels by request id, across a C ABI that carries JSON and
not pointers, so a handler cannot be handed anything. What it can have is one
implementation of the two pieces of state it needs, and that now lives in
`ah_plugin_api::cancellation` - `RequestScope`, `cancel`, `is_cancelled`, and
`wait_or_cancel`, the last kept because two of the five copies used a `Condvar`
so a poll interval wakes on cancellation instead of sleeping through it.

Five registries became one; 389 lines net removed.

### C. A `Forge` abstraction for GitHub/GitLab

The two plugins differ in endpoint shapes and payload names, not in behavior. Model
that difference as data, not as duplicated control flow:

```rust
trait Forge {
    fn issues(&self, q: &IssueQuery) -> Result<Vec<Issue>, ForgeError>;
    fn pipeline_status(&self, id: PipelineId) -> Result<PipelineStatus, ForgeError>;
    fn logs(&self, id: JobId, limits: LogLimits) -> Result<LogStream, ForgeError>;
    // …
}
```

with a shared `forge::core` implementing wait-loops, log limiting, warning
extraction and rendering once. Realistic target: ~7.8k lines of plugin code
collapsing to roughly 3–3.5k plus a shared core, with a single code path for
credentials.

Keep the two plugins as separate cdylibs and separate domains — the goal is shared
implementation, not a merged product surface.

## Migration

1. Create `ah-plugin-sdk` with `sdk::render` and `sdk::http` first; migrate the
   Ollama plugin (smallest, 1 040 lines) as the pilot.
2. Move credential resolution into `sdk::credentials`; migrate GitHub, then GitLab.
   Add a shared test suite that runs against both.
3. Add the runtime cancellation token; migrate the three built-in domains and the
   GitHub plugin; delete the globals.
4. ~~Add `sdk::process`; delete `AH_*_TEST_*` environment seams and rewrite those tests
   against the injected runner.~~ **(done by deletion; the seam had no tests)**
5. Introduce `Forge` and migrate GitHub and GitLab behavior into `forge::core`,
   one command family at a time (issues → releases → pipelines/runs → logs).
6. Adopt `sdk::render` in the host too; delete `render_plugins_table` column logic.

## Risks and invariants

- **Output must stay byte-identical.** Every renderer migration needs a golden test
  of the current text output first (see group 08).
- **ABI stability.** The SDK is a Rust-side convenience; the C ABI is unchanged.
  Plugins built against the old API keep loading.
- **Do not over-unify the forge model.** GitLab designs/GraphQL and GitHub artifacts
  have no counterpart on the other side; keep them as adapter-specific commands
  rather than forcing a lowest-common-denominator trait.
- ~~**Deleting the test env vars is a behavior change for the test suite only**~~ — it
  was a production behavior change: `AH_POSTGRES_TEST_SYSTEM_PATH` no longer overrides
  `psql` resolution. Nothing in the repository set it.

## Acceptance criteria

- No credential-handling code exists in more than one place.
- No plugin defines its own cancellation registry.
- No `AH_*_TEST_*` environment variable is read by production code paths.
- A new plugin can be written without copying code from an existing plugin; the
  Ollama plugin serves as the reference and stays under ~400 lines.
