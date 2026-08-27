# 07 — Update and Release Subsystem

**Severity: Medium.** The design here is the most carefully engineered part of the
project (signed manifests, journalled transactions, rollback, recovery). The problem
is not correctness — it is that the same subsystem is spread across five locations
with overlapping names and a circular dependency on the MCP service.

## Findings

### 7.1 Five homes, unclear ownership

| Location | Lines | Role |
|---|---|---|
| `src/updater/` (11 modules) | ~5 000 | orchestration: check, candidate, activate, recovery, smoke, handoff, installation, github, trust, command |
| `crates/ah-updater-core/` | ~1 900 | plan/journal model, release parsing, trust, errors |
| `crates/ah-update-helper/` | ~4 700 | privileged execution: transaction apply, restart manager, bounded process |
| `crates/ah-release-manifest/` | ~600 | canonical manifest, signature, validation |
| `crates/ah-release-tool/` | ~1 000 | build-time release packaging |

`src/updater/` is the third layer, living in the application crate. It is the layer
most likely to need reuse (by the helper, by tests, by a future daemon) and the one
least able to provide it.

### 7.2 Two `transaction.rs` files

`crates/ah-updater-core/src/transaction.rs` (736 lines — plan, journal, state
machine) and `crates/ah-update-helper/src/transaction.rs` (2 260 lines — apply,
activate, commit, rollback, recover, permanent backup). The *split* is right
(model vs. execution); the *naming* guarantees confusion in reviews, stack traces
and imports.

### 7.3 Filesystem hardening primitives are duplicated

[`src/updater/installation.rs`](../../src/updater/installation.rs) implements
`hash_file:475`, `has_single_hard_link:439`, `is_reparse_point:461`,
`read_bounded_file:524`, `ensure_direct_file:409`, `paths_equal:569`,
`encode_digest:588`. Equivalent logic exists in the helper crate
(`crates/ah-update-helper/src/transaction.rs` verification paths) and
`encode_digest` also exists in `ah-updater-core/src/transaction.rs:429`.

These are security-critical checks (hard-link and reparse-point rejection defeat
symlink-swap attacks). Duplicated security checks are the ones that drift.

### 7.4 The updater and the MCP service know about each other

`src/updater` calls into `mcp_service::lifecycle`:

- `stop_for_update_while_locked` (`lifecycle.rs:116`)
- `capture_for_update_while_locked` (`:129`)
- `restore_for_update_while_locked` (`:152`)

and hands off across processes via the `AH_UPDATER_MCP_RESTORE` environment variable
(`src/runtime_flow.rs:147`), which encodes an exact 5-element argv. The updater
cannot be tested without a scheduler, and the service cannot be reasoned about
without reading the updater.

### 7.5 Recovery runs before parsing, silently

`recover_before_startup` (`src/runtime_flow.rs:40`) runs on essentially every
invocation, before argument parsing, and on the recovery path emits a warning and
returns `Ok(())` — so `ah git status` can exit successfully having done something
entirely different. The behavior is defensible; its invisibility is not.

## Why it hurts

- Ownership questions ("where does a new verification step go?") have no clear
  answer, so code lands wherever the current task started.
- The root crate cannot shrink (group 09) while it holds 5 000 lines of updater.
- Duplicated hardening primitives are a real security-drift risk.
- The bidirectional coupling with the service blocks group 06: making the service
  cross-platform requires touching updater code.

## Target design

### A. Four crates, one role each

| Crate | Role | Contains |
|---|---|---|
| `ah-release-manifest` | wire format | canonical form, signature, validation *(unchanged)* |
| `ah-updater-core` | model | `plan`, `journal`, `release`, `trust`, `error` — rename `transaction.rs` to `plan.rs` + `journal.rs` |
| `ah-updater` **(new)** | orchestration | everything currently in `src/updater/`: check, candidate selection, download, smoke, activation, recovery |
| `ah-update-helper` | privileged execution | rename `transaction.rs` to `apply.rs`; keep `restart_manager`, `bounded_process` |

`src/updater/command.rs` shrinks to CLI wiring in the root crate.

### B. One `fsverify` module

All hardening primitives — digest, bounded read, direct-file check, hard-link
check, reparse-point check, atomic replace — live in one place (`ah-updater-core::fsverify`,
or `platform::fs` from group 06 if the primitives are generally useful). Every
verification path calls it. One implementation, one test suite, one fuzz target.

### C. Invert the service dependency

```rust
pub trait ServiceGuard {
    fn capture(&self) -> Result<ServiceState, GuardError>;
    fn stop_for_update(&self) -> Result<bool, GuardError>;
    fn restore(&self, state: ServiceState) -> Result<(), GuardError>;
}
```

`ah-updater` depends on the trait; `mcp_service` implements it; the root crate wires
them together. Consequences:

- the updater is testable with a fake guard, with no scheduler and no Windows;
- `mcp_service` no longer exports six `pub(crate)` update-specific helpers;
- a `NoServiceGuard` covers platforms and configurations without a managed service,
  replacing the current `cfg(windows)` gates.

The cross-process handoff (`AH_UPDATER_MCP_RESTORE`) becomes a hidden subcommand
per group 03, so it is parsed, validated, redacted and tested like any other entry
point.

### D. Make recovery observable

`recover_before_startup` should return a structured outcome that is always logged
(`EventLogger`) and always reported in `--json` mode, not only as a stderr warning.
A user or an agent must be able to tell that an invocation was consumed by recovery.

## Migration

1. Rename the two `transaction.rs` files (`plan.rs`/`journal.rs` in core,
   `apply.rs` in helper). Mechanical, zero behavior change, immediate clarity gain.
2. Extract `fsverify`; point all three duplicate sites at it; keep every existing
   test and add the missing symmetric ones.
3. Introduce `ServiceGuard`; move the six helpers out of
   `mcp_service::lifecycle` into an implementation module; delete the
   `cfg(windows)` gates on those helpers.
4. Move `src/updater/` into a new `ah-updater` crate. Do this after step 3, so the
   move does not have to drag a `mcp_service` dependency with it.
5. Convert `AH_UPDATER_MCP_RESTORE` / `AH_UPDATER_INSTALLED_SMOKE` into hidden
   subcommands, with a one-release deprecation window (see group 03 risks).
6. Add structured recovery reporting.

## Risks and invariants

- **Cross-version handoff.** An installed v1.4 helper may hand off to a newly
  installed v1.5 `ah`, and a v1.5 helper may need to recover a v1.4 transaction.
  Any change to the handoff protocol or journal format needs a compatibility window
  and explicit tests for both directions. This is the highest-risk item in the whole
  program; treat step 5 as a separate release with its own acceptance run.

  **Half tested now, and the untested half is named.** The helper validates its
  arguments *by position*, so the contract is an exact 14-element command line
  and not a set of flag names.
  `crates/ah-update-helper/tests/handoff_contract.rs` freezes it: all three
  operations parse it, a renamed flag at any of the six positions is refused, a
  swapped pair is refused, and the three values the helper acts on rather than
  copies - the inherited handle, the event name, the parent pid - are each
  refused when malformed.

  **Both directions are covered now.** The order lives in
  `ah_updater_core::HANDOFF_FLAGS`, and the command line is produced by two
  builders there - `handoff_paths_arguments` for the roots, which `ah` knows
  before the launch, and `handoff_lease_arguments` for the inherited lease and
  the acknowledgement event, which only the launcher knows. The parser validates
  against the same list instead of against literals at hard-coded indices, and
  the test puts the two halves together and parses them, which is the only place
  in the repository where that pairing can be checked at all.

  What is still untested is the Windows launcher's own handle plumbing -
  duplicating the lease into the child, the attribute list, the acknowledgement
  wait. No in-process test reaches it; `scripts/release_smoke.py` is its cover.
- ~~**The journal is on-disk state that survives across versions.** Renaming Rust
  modules must not change serialized field names or the `TransactionStateV1`
  discriminants. Add a fixture-based compatibility test before step 1.~~
  **(done, late rather than before step 1.)**
  `crates/ah-updater-core/tests/wire_compatibility.rs` freezes the v1 wire form
  of the plan, the journal and the helper self-check as fixtures, and covers the
  two directions differently because they are not the same kind of claim:

  - *Reading what an older release wrote* is a real test - the fixtures are
    parsed and validated by today's types.
  - *Writing what an older release can read* cannot run the old code, so it is a
    byte comparison. Both types carry `deny_unknown_fields`, so an older reader
    **rejects** a document with a field it does not know. The rule that follows
    is strict, and the test states it: a new field has to be
    `skip_serializing_if` so it is absent when unset. The upgrade fixture proves
    the three fields added since v1 - `operation`, `managed_mcp_was_running`,
    `managed_mcp_previous_instance_id` - are all absent from an ordinary plan,
    which is why v1 readers still accept one.

  Every `TransactionStateV1` discriminant and every `ManagedFileOperationV1` tag
  has its spelling pinned, because recovery dispatches on them and a rename
  would strand a transaction rather than fail loudly.
- **Do not weaken verification while deduplicating it.** The union of all three
  implementations is the required behavior, not the intersection; enumerate the
  checks each site performs before merging them.
- The existing acceptance work for managed MCP self-update
  and `scripts/release_smoke.py` is the safety net — run it at every step.

## Acceptance criteria

- The root crate contains no updater logic beyond CLI wiring.
- One implementation of each filesystem hardening primitive.
- `ah-updater` builds and its tests pass without `mcp_service` in the dependency graph.
- No module named `transaction.rs` in two crates.
- Every recovery-consumed invocation is visible in the event log and in JSON output.
