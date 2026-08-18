# HTTP Retries and Cross-Case Extraction Implementation Plan

## Objective

Implement the approved HTTP retry and cross-case extraction design from
`docs/plans/2026-08-13-http-retries-and-extraction-design.md`. The change must
keep existing text and JSON output contracts stable, preserve plugin ABI
compatibility, expose retry options consistently through CLI and typed MCP, and
publish extracted variables only after a completely successful assertion case.

## Workstreams

### Stream 1 – CLI and Typed MCP Contract

- Add `retry` and `retry_delay_ms` fields to `RequestOptionsArgs` in
  `src/commands/http.rs`, expose them as `--retry` and `--retry-delay-ms`, and
  include the same properties in every relevant typed MCP schema.
  - **Definition of Done:** one-off request commands, replay, assert, and run
    accept identical retry values through CLI and typed MCP.
  - **Validation:** focused argument/schema tests plus existing typed MCP tests.
- Forward the MCP remaining timeout as an execution deadline without changing
  released response field names.
  - **Definition of Done:** no retry attempt starts after the remaining tool
    budget expires.
  - **Validation:** deterministic domain test using a short deadline and a mock
    endpoint that remains retryable.

### Stream 2 – Shared Retry Execution

- Add a small internal retry policy/executor in
  `src/commands/http/domain.rs`; keep
  `src/commands/http/adapters/io.rs::send_request` as a single attempt.
  - **Definition of Done:** transport/read errors, timeouts, and `5xx` are
    retried up to `retry + 1` total attempts with a fixed delay; `4xx`, request
    construction errors, and assertion mismatches are not retried.
  - **Validation:** integration server counters for `503 -> 200`, `400`, and
    exhausted retries; focused unit tests for policy classification and deadline
    accounting.
- Route one-off requests, replay, and each assertion case through the executor.
  - **Definition of Done:** all entry points share one retry implementation and
    omission of retry flags preserves current behavior.
  - **Validation:** existing HTTP integration suite plus CLI coverage for a
    direct request and an assertion spec.

### Stream 3 – Atomic Assertion Extraction

- Extend the assertion spec model in `src/commands/http/domain.rs` with an
  `extract` map and strict JSON/header/text-regex selector types.
  - **Definition of Done:** each variable accepts exactly one selector; invalid
    selector shapes, regexes, or capture groups are rejected deterministically.
  - **Validation:** YAML deserialization and validation tests for valid and
    invalid selector forms.
- Evaluate extractors against the final `ResponseSnapshot`, reusing the current
  JSON-path resolver and normalized headers.
  - **Definition of Done:** JSON strings, compact non-string JSON, headers, and
    regex groups produce deterministic string values; JSON/text extraction
    rejects truncated bodies.
  - **Validation:** focused unit tests and integration cases for all sources,
    missing values, capture groups, and truncation.
- Treat assertions plus extraction as one case transaction.
  - **Definition of Done:** extracted values merge into the run variable map
    only when the case has no failures; a later successful value can replace an
    earlier variable; reports never expose extracted values.
  - **Validation:** multi-case integration tests for interpolation, overwrite,
    atomic rollback, and secret absence in text/JSON/JUnit reports.

### Stream 4 – Documentation and Backlog Reconciliation

- Update `docs/reference/http.md` with retry semantics, mutating-method warning,
  extraction syntax, and examples; update the relevant HTTP AI recipe under
  `docs/agents/recipes/`.
  - **Definition of Done:** CLI, MCP, YAML syntax, failure semantics, and safety
    caveats are documented consistently.
  - **Validation:** examples match tested command/spec syntax and exact flag
    names.
- Remove completed retry/extraction backlog statements from
  `docs/reference/http.md` and `src/plugins.rs`, preserving the user's existing
  status-cleanup edits in those files.
  - **Definition of Done:** repository status surfaces list only genuinely
    remaining work.
  - **Validation:** exact search for obsolete "planned"/"unimplemented" HTTP
    statements and review of the final diff.

### Stream 5 – Repository Validation

- Run the smallest focused tests during implementation, then the required
  project checks from `AGENTS.md`:
  - `ah run check cargo fmt --all -- --check`
  - `ah run check cargo test --workspace --all-targets --locked`
  - `ah run check cargo build --locked`
  - **Definition of Done:** all checks pass, or any environmental failure is
    reported with the exact command and evidence.
- Review `git diff --check`, the complete diff, and working-tree status.
  - **Definition of Done:** no unrelated user changes are overwritten or staged;
    output/ABI compatibility claims match the diff.

## Risks & Mitigations

- **Repeated remote mutation:** retries can duplicate POST/PUT/PATCH/DELETE side
  effects. Keep retries opt-in and document the ambiguity prominently.
- **Late MCP side effects:** a blocking worker can outlive the tool response.
  Check an absolute deadline before every sleep and request, and cap each
  attempt timeout to the remaining budget.
- **Secret leakage:** extracted values commonly contain credentials. Never add
  extracted maps or values to result/report structs or failure text.
- **Partial workflow state:** publishing some values before another extractor
  fails would make later cases nondeterministic. Collect into a temporary map
  and commit only after all assertions and extractors succeed.
- **Schema drift:** CLI and MCP arguments are declared separately. Add parity
  tests and update both declarations in the same task.
- **User-owned dirty files:** documentation and plugin metadata already contain
  authoritative edits. Patch only the relevant backlog lines and review those
  files before and after the change; do not revert surrounding modifications.

## Rollback

The implementation is additive. A rollback removes the two retry argument
fields, the domain retry executor, the optional `extract` spec field and its
tests/docs. Existing specs remain valid throughout because both features are
opt-in and no released output or ABI fields are changed.
