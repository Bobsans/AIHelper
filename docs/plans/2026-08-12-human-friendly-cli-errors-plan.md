# Human-friendly CLI errors implementation plan

## Objective

Replace terse default console errors with actionable, Docker-like diagnostics for human users while preserving all existing JSON fields, MCP behavior, plugin responses, and ABI contracts described in the approved design.

## Workstreams

### Stream 1 – Console diagnostic model and renderer

- Introduce a text-only diagnostic view derived from the existing `AppError`/`RenderedError` data. Definition of done: it can represent a primary explanation, optional correction, usage, scoped help, and recovery hints without changing serialized diagnostics.
- Preserve complete multiline parser messages and normalize Clap sections into the common `ah` layout. Definition of done: missing argument names, spelling suggestions, usage, and help targets are visible in text mode.
- Add targeted unknown-domain guidance, including `ah version` → `ah --version` and root help. Definition of done: the former `DOMAIN_NOT_FOUND: version` output is fully human-readable and contains no internal error code in text mode.

### Stream 2 – Tests and command documentation

- Update unit tests for plain and colored console rendering. Definition of done: labels are styled deterministically while content remains unchanged without color.
- Add integration tests for unknown domains, misspelled subcommands, missing required arguments, operational errors, and JSON compatibility. Definition of done: each approved UX example is covered by an executable assertion.
- Update relevant user and agent documentation if command-error behavior is described there. Definition of done: documentation matches the new text/JSON split and no unrelated pages change.

### Stream 3 – Validation

- Run focused tests during implementation.
- Run `ah run check cargo fmt --all -- --check`, `ah run check cargo test --workspace --all-targets --locked`, and `ah run check cargo build --locked` before handoff.
- Manually exercise representative invalid commands and compare their stderr with the approved examples.

## Risks and mitigations

- Parsing rendered Clap text can be version-sensitive. Keep parsing narrow, retain unknown lines instead of dropping them, and cover representative messages with tests.
- Rich text output could accidentally alter JSON. Keep `wants_json_error_output()` as an early branch and add an integration compatibility assertion.
- Over-eager suggestions can mislead users. Provide only Clap suggestions, explicit conventional aliases, or high-confidence catalog matches; otherwise show help without a guess.
- Existing consumers may inspect plain stderr. Limit the breaking presentation change to non-JSON human mode, which is the explicitly requested behavior, while keeping deterministic output and exit status.
