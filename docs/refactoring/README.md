# AIHelper Refactoring Program

Architectural review of the AIHelper workspace, written as if the project had
been handed over for an unconstrained optimization pass. The goal is **long-term
extensibility and maintainability**, not short-term feature delivery.

## Scope of the review

- ~87k lines of Rust across 12 workspace members (`src/` 47k, `crates/` + `plugins/` 40k).
- Reviewed: workspace layout, plugin contract, CLI/MCP dispatch, command
  implementation pattern, output and error model, secrets, logging/redaction,
  managed MCP service, updater/release subsystem, tests, build and CI.
- Not reviewed: business correctness of individual commands, protocol-level MCP
  conformance, cryptographic design of the release manifest (assumed sound).

## What is already good

Findings below are about the parts that will hurt as the project grows. For balance,
these decisions are sound and should be preserved through any refactor:

- The plugin boundary is real: a stable C ABI (`crates/ah-plugin-api`), version
  and capability negotiation, and a loader that validates metadata and symbol
  completeness before trusting a library.
- Typed commands are schema-validated on both input and output, so a plugin cannot
  silently return a shape the host did not advertise.
- The catalog is compiled once per definition revision instead of per request.
- `LifecycleService<S, R>` is generic over scheduler and readiness probe — the right
  shape, even though it currently has one implementation.
- Atomic, locked persistence (`src/persistence.rs`) is used consistently for state files.
- Secret redaction exists at three layers and is tested for the obvious cases.

## Finding groups

| # | Group | Core problem | Severity |
|---|-------|--------------|----------|
| [01](01-command-contract-duplication.md) | Command contract duplication | One command is defined 4–5 times by hand | **Critical** |
| [02](02-plugin-duplication-and-sdk.md) | Plugin duplication / missing SDK | ~7.8k lines of near-parallel plugin code, cross-cutting concerns copy-pasted | **Critical** |
| [03](03-composition-and-configuration.md) | Composition, bootstrap, configuration | Ad-hoc argv pre-parsers, hidden env-var control flow, process-global state | **High** |
| [04](04-output-and-error-model.md) | Output and error model | Direct `println!`, three overlapping error taxonomies, hand-written mapping tables | **High** |
| [05](05-module-boundaries.md) | Module boundaries / god files | 2k–4k-line modules mixing four responsibilities each | **High** |
| [06](06-platform-portability.md) | Platform portability | 173 `cfg` sites; managed service is Windows-only by construction | **Medium** |
| [07](07-update-and-release-subsystem.md) | Update / release subsystem | Three-layer split with unclear ownership and duplicated primitives | **Medium** |
| [08](08-testing-strategy.md) | Testing strategy | Process-level tests substitute for missing seams; test hooks shipped in production | **High** |
| [09](09-workspace-build-and-ci.md) | Workspace, build, CI | No workspace dependency management, monolithic root crate, thin CI | **Medium** |
| [10](10-roadmap.md) | Roadmap | Sequencing, invariants, acceptance gates | — |

## Method

Each document follows the same shape:

1. **Findings** — what is wrong, with `file:line` evidence and a measured cost.
2. **Why it hurts** — the concrete failure mode as the project grows.
3. **Target design** — what it should look like instead.
4. **Migration** — ordered, individually shippable steps.
5. **Risks and invariants** — what must not change.
6. **Acceptance criteria** — how to know the work is done.

## Non-negotiable invariants

Every proposal in this program is constrained by the rules already stated in
`AGENTS.md`, and no step may violate them:

- Text and JSON output stay deterministic.
- Released JSON field names do not change without an explicit breaking-change decision.
- Plugin ABI compatibility is preserved; ABI changes go through version negotiation.
- Behavior changes ship with tests for both the success and the failure path.

The practical consequence: **every refactor in this program must be preceded by a
golden-snapshot test of the artifact it touches** (command catalog, manual, rendered
text output, JSON payloads). Refactors then become provably behavior-preserving
instead of hopefully behavior-preserving.
