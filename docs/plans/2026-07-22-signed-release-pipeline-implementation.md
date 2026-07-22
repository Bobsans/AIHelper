# Signed Release Pipeline Implementation Plan

## Goal

Implement the approved fail-closed signed release pipeline without provisioning
a production key or adding signing behavior to the shipped `ah` binary.

## Governing Documents

- `docs/plans/2026-07-22-signed-release-pipeline-design.md`
- `docs/decisions/2026-07-22-adopt-canonical-ed25519-release-manifest-v1.md`
- `roadmap/managed-mcp-and-self-update.md`

## Task 1: Add the release-tool package and closed target profiles

Affected files:

- `Cargo.toml`
- `Cargo.lock`
- `crates/ah-release-tool/Cargo.toml`
- `crates/ah-release-tool/src/lib.rs`
- `crates/ah-release-tool/src/error.rs`
- `crates/ah-release-tool/src/profile.rs`
- `crates/ah-release-tool/src/archive.rs`
- `crates/ah-release-tool/src/main.rs`
- `crates/ah-release-tool/tests/archive_contract.rs`

Steps:

1. Add an unpublished binary package that depends on `ah-release-manifest`.
2. Define the exact Windows x64, Linux x64, and macOS arm64 asset names,
   targets, architectures, executables, and plugin paths.
3. Implement bounded ZIP inspection and SHA-256 hashing from the final archive.
4. Reject missing, extra, duplicate, case-colliding, linked, encrypted, unsafe,
   or otherwise non-profile entries.
5. Expose `validate-archives` with deterministic output and errors.
6. Add positive profile fixtures generated in tests and negative archive cases.
7. Run formatting and `cargo test -p ah-release-tool --locked`.
8. Review the diff and commit only this task.

Expected commit: `feat: validate release archive sets`

## Task 2: Generate, sign, and reconcile release triplets

Affected files:

- `crates/ah-release-tool/Cargo.toml`
- `crates/ah-release-tool/src/lib.rs`
- `crates/ah-release-tool/src/error.rs`
- `crates/ah-release-tool/src/key.rs`
- `crates/ah-release-tool/src/release.rs`
- `crates/ah-release-tool/src/main.rs`
- `crates/ah-release-tool/tests/signing_contract.rs`
- `crates/ah-release-tool/tests/release_set.rs`

Steps:

1. Parse only exact 43-character unpadded Base64URL Ed25519 seed and public-key
   encodings.
2. Derive the public key and key ID from the seed and require an exact match
   with the configured public key.
3. Build canonical manifests from validated ZIPs using release tag/version,
   final HTTPS GitHub URLs, closed profiles, and full inventory hashes.
4. Sign the existing domain-separated preimage and write exact 86-byte detached
   signatures without trailing newlines.
5. Verify each generated manifest with a one-key trust registry and reconcile
   every manifest field with the ZIP before returning success.
6. Write results to a fresh output directory only after the complete set is
   valid, leaving no partial publishable set on failure.
7. Zeroize decoded signing material and keep secret values out of error text.
8. Cover deterministic test-only signing, tampering, key mismatch, malformed
   configuration, version/tag mismatch, and output-set completeness.
9. Run focused tests, formatting, and source searches proving that `aihelper`
   and `ah-release-manifest` contain no production signing API or key.
10. Review the diff and commit only this task.

Expected commit: `feat: sign release manifest sets`

## Task 3: Integrate the GitHub Actions pipeline

Affected files:

- `.github/workflows/release.yml`
- optional workflow contract test under `scripts/` or the release-tool tests

Steps:

1. Make matrix asset identity explicit instead of deriving it from
   `runner.arch`.
2. Keep build, package, and smoke-test jobs free of signing configuration.
3. Add a no-secret validation job that requires exactly the three ZIPs.
4. Make `workflow_dispatch` terminate after validation without signing or
   publication.
5. Add a release-only job using the protected `release-signing` environment.
6. Build the internal tool before the signing step; expose seed/public-key
   values only as step-scoped environment variables to the built binary.
7. Upload one explicit verified artifact containing the expected nine files.
8. Add a separate publish job with `contents: write` and no signing secret.
9. Require exact filenames immediately before release upload.
10. Validate workflow structure locally where available and review the diff.
11. Commit only the workflow integration.

Expected commit: `ci: publish signed release manifests`

## Task 4: Validate and close the completed roadmap item

Affected files:

- `docs/plans/2026-07-22-signed-release-pipeline-design.md`
- `roadmap/managed-mcp-and-self-update.md`

Steps:

1. Run:
   - `ah run check cargo fmt --all -- --check`
   - `ah run check cargo test -p ah-release-tool --locked`
   - `ah run check cargo test -p ah-release-manifest --locked`
   - `ah run check cargo test --workspace --all-targets --locked`
   - `ah run check cargo build --locked`
   - `ah run check cargo build --release --locked`
2. Run diff, workflow, secret-leak, and exact-output checks.
3. Record verification results and the known post-publication GitHub Release
   page limitation in the design document.
4. Remove only the completed release-pipeline item from the roadmap.
5. Keep the production-public-key item open until real key bytes are embedded.
6. Review all task commits and confirm the user-owned release skill change is
   still untouched.
7. Commit the documentation and roadmap closure.
8. Record the completed milestone in the `aihelper` Basic Memory project.

Expected commit: `docs: close signed release pipeline roadmap item`
