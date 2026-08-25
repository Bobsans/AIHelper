# 09 — Workspace, Build and CI

**Severity: Medium**, but the cheapest wins in the whole program live here.

## Findings

### 9.1 No workspace-level dependency or package management

`Cargo.toml:8` declares `[workspace]` with 12 members and nothing else — no
`[workspace.package]`, no `[workspace.dependencies]`. Every member pins its own
versions independently:

- `serde` is declared 9 times, `serde_json` 10, `clap` 5, `reqwest` 4, `zip` 3.
- **`sha2` has drifted**: `0.10.9` in `plugins/ah-plugin-postgres/Cargo.toml`,
  `0.11` everywhere else. `Cargo.lock` therefore contains two `sha2` builds — and
  two `digest`, `block-buffer`, `crypto-common` trees behind them.
- 24 crate names appear at more than one version in `Cargo.lock`.
- `edition`, `license`, `repository`, `rust-version` are repeated per member (or
  absent).

There is also no `rust-toolchain.toml` and no `rust-version` (MSRV) anywhere, while
`edition = "2024"` requires a recent toolchain. CI uses `toolchain: stable`,
unpinned — so a toolchain release can break CI without a repository change.

### 9.2 The root crate is a monolith

`src/` is 47 270 lines in one crate holding: the CLI, eight built-in domains, the
secrets vault, the event log and redaction engine, the managed service, the updater,
and the plugin host wiring. It is a workspace with a monolith inside it.

Practical effects: any change recompiles everything; nothing in `src/` can be reused
by the helper binary or by plugins; `pub(crate)` is doing the job that crate
boundaries should do; and the group 05 god files have nowhere natural to split *to*.

### 9.3 CI is a smoke test, not a gate

`.github/workflows/ci.yml` runs, on Linux and Windows:

```
cargo fmt --all -- --check
cargo test --workspace --all-targets --locked
```

Missing:

| Check | Why it matters here |
|---|---|
| `cargo clippy --workspace --all-targets -- -D warnings` | six crate-level `allow(clippy::result_large_err)` are hiding a real issue (group 04) |
| `cargo build --release --locked` | `AGENTS.md` requires it for release-sensitive work; CI never does it |
| macOS runner | `keyring` is configured with `apple-native`; untested |
| `cargo deny` / `cargo audit` | the project downloads and executes signed releases and handles credentials |
| MSRV job | no MSRV is declared, so there is nothing to verify |
| docs freshness | `docs/reference` has no coupling to code (group 01) |
| `cargo doc --no-deps -D warnings` | public plugin API is the contract for third parties |
| supply-chain pinning for plugin builds | plugins are cdylibs loaded at runtime |

`fail-fast: false` and pinned action SHAs are good practice already in place.

### 9.4 Lints are suppressed at crate scope

`#![allow(clippy::result_large_err)]` in `src/lib.rs:1`, `crates/ah-mcp/src/server.rs:1`,
and all four plugin crates, plus six targeted allows in `ah-plugin-api`. A
crate-level allow suppresses the lint for code that has not been written yet.

### 9.5 Release packaging is bespoke and partly unverified

`crates/ah-release-tool` plus `scripts/release_smoke.py` and `scripts/benchmark.ps1`
(PowerShell — Windows-only) implement release packaging and verification. The
contract tests in `crates/ah-release-tool/tests/` are good; the smoke script is not
run by `ci.yml`.

## Target design

### A. Workspace-level dependency and package management

```toml
[workspace.package]
version = "1.4.0"
edition = "2024"
rust-version = "1.85"      # pick and enforce
license = "…"
repository = "…"

[workspace.dependencies]
serde = { version = "1.0.219", features = ["derive"] }
serde_json = "1.0.140"
sha2 = "0.11"
clap = { version = "4.5.39", features = ["derive"] }
# …
```

Every member switches to `serde = { workspace = true }`. Add `rust-toolchain.toml`
pinning the toolchain, and a CI job that verifies MSRV.

### B. Decompose the root crate

Target layout (aligning with groups 02, 05, 06, 07):

| Crate | Contents |
|---|---|
| `ah` (bin) | argv parsing, bootstrap, dispatch — thin |
| `ah-paths` | path and config resolution (group 06) |
| `ah-redact` | redaction engine (group 04) |
| `ah-observability` | event log, sinks, rotation |
| `ah-secrets` | vault, key providers, setup service |
| `ah-domains-*` | built-in domains (one crate, or grouped: `fs`, `vcs`, `net`, `proc`) |
| `ah-service` | managed service lifecycle + scheduler adapters |
| `ah-updater` | updater orchestration (group 07) |
| `ah-plugin-sdk` | shared plugin implementation support (group 02) |

Benefits: parallel compilation, real API boundaries, per-crate test suites, and
the ability for the helper binary and plugins to reuse host code without pulling in
the CLI.

### C. CI as a gate

```yaml
jobs:
  check:    # fmt, clippy -D warnings, cargo doc
  test:     # ubuntu / windows / macos, --workspace --all-targets --locked
  msrv:     # cargo check with the pinned MSRV toolchain
  release:  # cargo build --release --locked, then release_smoke
  supply:   # cargo deny check, cargo audit
  docs:     # regenerate docs/reference, fail on diff
```

Nightly: fuzz targets (group 08), plus a slow acceptance run of the managed-service
and updater lifecycles.

## Migration

1. Add `[workspace.package]` and `[workspace.dependencies]`; unify `sha2` to a single
   version; add `rust-toolchain.toml` and `rust-version`. One PR, mechanical,
   immediately removes duplicate builds from the lock.
2. Add `clippy -D warnings` to CI **with the existing allows still in place**, so the
   gate starts green; then remove allows one crate at a time (group 04 step 7).
3. Add macOS to the test matrix; fix what it surfaces.
4. Add `cargo deny`/`audit` and `cargo doc` jobs.
5. Add the release-build job and wire `scripts/release_smoke.py` into it. Port the
   PowerShell benchmark script to something cross-platform, or mark it explicitly
   Windows-only and exclude it from the portable workflow.
6. Extract crates in dependency order — `ah-paths`, `ah-redact`, `ah-secrets`,
   `ah-observability`, then `ah-updater`, `ah-service`, then domains last (they
   depend on the most). Each extraction is a move plus a `Cargo.toml`, verified by
   an unchanged test suite.

## Risks and invariants

- **Version unification can change behavior.** `sha2` 0.10 → 0.11 changes API
  surface; verify the digest usage in the postgres plugin explicitly rather than assuming
  a drop-in bump.
- **Crate extraction changes visibility.** Items currently `pub(crate)` become `pub`,
  which is a real API decision each time — do not blanket-`pub` a module to make a
  move compile; that would trade a monolith for an unowned public surface.
- **Do not extract domains before the SDK exists** (group 02), or each domain crate
  will carry its own copy of the shared helpers.
- **MSRV choice is a support commitment.** Pick it deliberately, document it in
  `CONTRIBUTING.md`, and only then enforce it.

## Acceptance criteria

- One version of every third-party dependency in `Cargo.lock` (excepting genuinely
  unavoidable transitive splits).
- `rust-toolchain.toml` present; MSRV declared and verified in CI.
- Clippy runs with `-D warnings` and no crate-level lint allows remain.
- CI covers Linux, Windows, macOS, release build, supply-chain audit, and docs freshness.
- No crate exceeds ~10k lines.
