# Offline Candidate Smoke Design

## Status

Approved in conversation on 2026-07-23. This design resolves a roadmap ordering
conflict: Stage 4 can validate the staged `ah.exe` and plugin catalog now, while
the `ah-update-helper.exe` protocol smoke remains in Stage 6, where that binary
is introduced.

This is a refinement of
[`2026-07-22-managed-mcp-self-update-completion-design.md`](2026-07-22-managed-mcp-self-update-completion-design.md).
It does not weaken the requirement that a future complete Windows candidate
must pass both executable and helper smoke checks before activation.

## Goal

Prove that a fully verified, extracted candidate can execute offline and is
internally coherent before any installation or managed MCP state changes:

- the staged `ah.exe` starts and reports the signed release version;
- built-in and staged dynamic plugins load using the supported plugin ABI;
- every loaded dynamic plugin has a valid typed command catalog;
- the process reads and writes only an isolated temporary configuration;
- execution, output, and diagnostics remain bounded and deterministic.

## Chosen Approach

Add an offline smoke layer after candidate extraction and before installation
classification. It consumes a `PreparedCandidate` and returns a distinct
smoke-checked candidate type. Later mutation code therefore cannot accidentally
accept a candidate that only passed archive verification.

The smoke layer runs the staged executable twice:

1. `ah.exe --version` must exit successfully and report exactly the version from
   the verified manifest.
2. `ah.exe --json plugins list` must exit successfully and return the expected
   plugin catalog.

Both processes run with the candidate root as their working directory and a
private `AH_CONFIG_DIR` inside the candidate staging area. The second command
exercises the normal configuration, built-in registration, dynamic plugin
loading, ABI validation, and command-catalog path. The first command separately
proves the version fast path.

The existing bounded process runner remains the single process-control
implementation. It gains child-only environment overrides so smoke execution
does not mutate the parent environment and retains timeout, output limit, and
Windows Job Object termination behavior.

## Catalog Validation

The smoke layer compares the JSON plugin list with the signed manifest:

- the number of `dynamic` entries equals the number of manifest files whose
  purpose is `plugin`;
- dynamic domains and plugin names are unique;
- every dynamic entry is enabled;
- every dynamic entry reports `AH_PLUGIN_ABI_VERSION`;
- every dynamic entry is MCP-exposed and has no catalog omission reason.

Generating the plugin list already requests the typed command catalog for every
registered plugin. A missing or invalid built-in or dynamic catalog therefore
causes the smoke command to fail before validation succeeds.

Additive JSON fields are ignored, while required fields and values are checked.
This preserves compatibility with future non-breaking catalog extensions.

## Failure Contract

The smoke fails closed on launch failure, timeout, cancellation, non-zero exit,
truncated output, unexpected stderr, malformed JSON, version mismatch, missing
or duplicate plugins, ABI mismatch, disabled plugins, or invalid catalogs.

Failures use the updater `candidate` error category. Diagnostics do not include
untrusted child output or filesystem paths. Dropping the prepared candidate
removes its private staging directory and leaves the installation, user
configuration, and managed MCP untouched.

## Alternatives Considered

### Hidden self-test CLI command

A dedicated hidden command could return a smaller purpose-built payload, but it
would add a new CLI contract and could bypass the public plugin catalog path
that must remain functional. It is rejected for this slice.

### In-process plugin loading

Loading candidate DLLs into the running updater would avoid a child process, but
it would not prove that the staged executable starts and risks mixing candidate
plugins with the active process. It is rejected.

### Dedicated unbounded process launcher

Using `std::process::Command` directly would simplify environment isolation but
would duplicate timeout and output handling and weaken process-tree cleanup.
It is rejected in favor of extending the existing bounded runner.

## Verification

Unit tests use an injectable smoke process adapter and cover:

- successful version and catalog validation;
- child working directory and isolated `AH_CONFIG_DIR`;
- version mismatch and malformed output;
- launch, timeout, truncation, stderr, and exit failures;
- missing, duplicate, disabled, ABI-incompatible, and catalog-invalid plugins;
- preservation of a sentinel user configuration.

The applicable workspace format, test, debug build, and release build checks
must pass before the completed Stage 4 roadmap item is removed and committed.
Stage 6 must explicitly retain helper protocol smoke coverage until the signed
update helper exists.
