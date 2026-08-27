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
| ~~credential authority parsing~~ | *forge policy; kept, now three lines each* | |
| ~~HTTPS authority extraction~~ | `ah_plugin_sdk::credentials` | |
| ~~loopback detection~~ | `ah_plugin_sdk::credentials` | |
| ~~git remote authority~~ | `ah_plugin_sdk::credentials` | |
| ~~env token lookup~~ | `ah_plugin_sdk::credentials` | |
| ~~`git credential fill` driver~~ | `ah_plugin_sdk::credentials` | |
| ~~bounded credential child wait~~ | `ah_plugin_sdk::credentials` | |
| ~~ambient-token policy~~ | `credentials::TokenPolicy`, per-forge data only | |
| ~~JSON request wrapper~~ | `ah_plugin_sdk::http::JsonApi`, shared with Ollama too | |
| ~~response/error mapping~~ | `ah_plugin_sdk::http::JsonApi` | |
| ~~log/trace fetch with byte caps~~ | `ah_plugin_sdk::logs` | |
| ~~ANSI stripping~~ | `ah_plugin_sdk::render` — **the two copies had diverged** | |
| ~~error truncation~~ | `ah_plugin_sdk::render` | |
| ~~warning-line heuristic~~ | `ah_plugin_sdk::logs`, byte-identical in both | |
| ~~wait-for-completion loop~~ | `ah_plugin_sdk::poll` — **the two copies had diverged** | |
| ~~success rendering~~ | `ah_plugin_sdk::render`, shared by all four plugins | |
| state/status styling — **kept separate, deliberately** | per-forge vocabulary | |
| ~~manual example builder~~ | `ManualExample::new` in `ah-plugin-api`, shared with the host | |

Two independent copies means two places to fix every credential-handling bug —
and credential handling is exactly the code that must not diverge.

**The state/status style tables are not duplication and were not merged.** The
two `match` arms have the same shape but different *vocabularies*: GitHub says
`open`, GitLab says `opened`; GitHub `cancelled`, GitLab `canceled`; their
pending sets do not overlap. Sharing them would mean either a union table that
lies about both forges or a narrower one that loses styling, and the shared part
is a six-line `match` — not the cost. This is the same call as
`credentials::TokenPolicy`: shared mechanism, per-forge data.

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

**Other duplicated primitives across the workspace:** ~~`manual_example` (5)~~,
`normalize_path` (9), ~~`truncate_for_error` (4)~~, ~~`render_success` (4)~~,
~~`strip_ansi_sequences` (2)~~, ~~`paint_if_present` (2)~~, ~~`input_schema` (2)~~.

`normalize_path` is the one left, and it is not one duplicate but nine *different*
functions sharing a name: some take a `&Path`, some a `&str`, and they disagree on
trailing separators. Unifying them is a semantic decision, so it belongs to group
05, not here.

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

### 2.4 Text rendering is re-invented per plugin *(partly resolved, partly not a finding)*

Every plugin writes its own `render_*_text` family (GitHub: 9 renderers, GitLab: 11,
Postgres: 15). Those are not duplication: each renders a different payload, and the
prose inside them is the product surface. What *was* duplicated is the scaffolding
under them, and that is now `ah_plugin_sdk::render`.

The claim that "column alignment logic exists in at least four places" is wrong.
`column_width`/`pad_column` exist once, in the host
([`src/lib.rs`](../../src/lib.rs)); no plugin aligns columns at all. A shared table
renderer would be speculative until something else needs one.

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
| ~~`sdk::http`~~ *(done)* | blocking JSON client: bounded error bodies, authority-bound credentials, uniform error mapping. No retry policy or pagination: neither existed to share |
| ~~`sdk::credentials`~~ *(done)* | authority parsing, loopback checks, `git credential fill` with bounded child wait, env fallback, ambient-token policy |
| ~~`sdk::cancel`~~ | *done, as `ah_plugin_api::cancellation` — see B* |
| ~~`sdk::render`~~ *(done)* | success rendering, ANSI stripping, truncation, styling an optional value. No table renderer: see 2.4 |
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

1. ~~Create `ah-plugin-sdk` with `sdk::render` and `sdk::http` first; migrate the
   Ollama plugin (smallest, 1 040 lines) as the pilot.~~ **(done; all four plugins
   migrated at once rather than piloting one, since the golden snapshots made the
   whole set safe to move together)**
2. ~~Move credential resolution into `sdk::credentials`; migrate GitHub, then GitLab.
   Add a shared test suite that runs against both.~~ **(done)** — the shared suite lives
   with the code it tests, in the SDK; each plugin keeps only the assertions about its
   own policy, and makes them against the policy value the plugin actually builds.
3. ~~Add the runtime cancellation token; migrate the three built-in domains and
   the GitHub plugin; delete the globals.~~ **(done)** The five registries
   became `ah_plugin_api::cancellation`, and the five `cancelled_response`
   copies that went with them are now one function there too - they differed
   only in the domain name and the one-line summary, both of which are
   user-visible and stay each domain's own.
4. ~~Add `sdk::process`; delete `AH_*_TEST_*` environment seams and rewrite those tests
   against the injected runner.~~ **(done by deletion; the seam had no tests)**
5. Introduce `Forge` and migrate GitHub and GitLab behavior into `forge::core`,
   one command family at a time (issues → releases → pipelines/runs → logs).

   **Started from the other end, and the sketch above needs correcting.** The
   `trait Forge` with `issues()`/`pipeline_status()`/`logs()` describes the
   *product surface*, which is where the two plugins genuinely differ - GitHub
   pages past pull requests and has a search API, GitLab has neither; GitHub
   unzips a log archive, GitLab streams a trace. Their `execute_issues`
   functions are thirty lines each and share almost no control flow. Writing
   that trait would have produced adapters that are longer than what they
   replace.

   What was actually duplicated, after phases 1-3 had already taken the
   credentials, HTTP and rendering out, was two loops:

   - **The log line scan** - read a stream against a byte budget, strip the
     runner's terminal control sequences, keep the lines `--grep` or
     `--warnings-only` selects, stop at the line limit and say so. Fifty-five
     lines, near-verbatim in both. Now `sdk::logs::scan_lines`, with the budget
     as a value so GitHub can spend one `--max-expanded-bytes` across every
     archive entry, and with the failures returned rather than rendered so each
     plugin keeps its own diagnostic codes and wording.
   - **The wait loop** - ask, answer if terminal, give up at the deadline, sleep
     until the next attempt or until cancelled. Now `sdk::poll::until_ready`.
     **The two copies had drifted:** GitLab re-checked the deadline after
     sleeping and GitHub did not, so `github.run.wait` could issue one more
     request after `--timeout` had already passed. The shared loop checks, which
     is what the flag promises.

   Together: 239 lines out of the two plugins, 161 back in, and the two loops
   have unit tests of their own for the first time - byte budgets, the line
   limit, non-UTF-8 lines, a budget spanning several streams, and each of the
   three wait outcomes. Releases and issues remain as they are; the honest
   reading is that the 7.8k → 3.5k figure above counted duplication that
   phases 1-3 have already removed.
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

- ~~No credential-handling code exists in more than one place.~~ **(met)**
- ~~No plugin defines its own cancellation registry.~~ **(met)**, and none
  defines its own cancellation *response* either.
- No `AH_*_TEST_*` environment variable is read by production code paths.
- A new plugin can be written without copying code from an existing plugin; the
  Ollama plugin serves as the reference and stays under ~400 lines.
