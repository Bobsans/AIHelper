---
status: accepted
date: 2026-07-22
decision-makers: AIHelper maintainer
consulted: Codex design review
informed: AIHelper contributors
---

# Adopt canonical Ed25519 release manifest v1

## Context and Problem Statement

AIHelper needs a signed release contract before it can safely download,
validate, install, or roll back a release bundle. The current release workflow
publishes one ZIP archive per target but has no machine-verifiable inventory or
authenticity metadata. The self-update roadmap requires each archive to have a
versioned manifest containing its complete managed-file inventory, archive and
file digests, updater compatibility, target identity, and signing-key identity.

The decision is how to encode and authenticate that manifest so the main
`ah` executable, a future update-helper, and release tooling interpret exactly
the same bytes without expanding the released CLI, JSON, MCP, or plugin ABI
contracts.

The roadmap and design specification this decision was drawn from were the
planning artefacts for work that has since shipped; they were removed with the
rest of that archive and remain in the Git history.

## Decision Drivers

- Verification must fail closed for unknown schema versions, fields,
  algorithms, and key identifiers.
- The signed representation must have exactly one valid byte encoding.
- Manifest contents must stay human-readable and easy to inspect in release
  assets.
- One contract must be reusable by `ah`, release tooling, and the future
  update-helper without widening the root crate's public surface.
- The runtime must contain verification material only. Private signing keys and
  production signing APIs must not enter the repository or release binary.
- Version 1 needs additive key rotation but does not need channels, online
  revocation, threshold signing, or general metadata delegation.

## Considered Options

- AIHelper-specific canonical JSON signed as exact bytes.
- RFC 8785 JSON Canonicalization Scheme.
- A TUF-like or separate binary signing representation.

## Decision Outcome

Chosen option: **AIHelper-specific canonical JSON signed as exact bytes**.

Create a focused internal workspace crate named `ah-release-manifest`. Schema
v1 uses only strict Serde structs, closed enums, strings, integer sizes, and
ordered arrays. It contains no free-form maps, floating-point values, optional
field omission, or unknown-field tolerance.

The canonical representation is compact UTF-8 JSON without a byte-order mark,
insignificant whitespace, or a trailing newline. Struct declaration order is
normative. Managed files and required-path arrays must already be sorted and
unique. A verifier parses the typed value, serializes it with the v1 canonical
serializer, and rejects the input unless those bytes are identical.

Each platform archive has independent assets:

```text
ah-<platform>-<architecture>.zip
ah-<platform>-<architecture>.manifest.json
ah-<platform>-<architecture>.manifest.sig
```

The detached signature file is exactly one unpadded Base64URL Ed25519
signature: 86 ASCII bytes, with no whitespace or newline. The signing preimage
is:

```text
"AIHELPER-RELEASE-MANIFEST-V1\0" || canonical_manifest_bytes
```

Verification uses strict Ed25519 verification. The signed manifest contains
`signing.algorithm = "ed25519"` and a key ID formatted as
`ed25519-sha256-<lowercase public-key SHA-256>`. A trust registry supports more
than one public key so a new key can be distributed before the release signer
switches to it.

No production release key exists yet. This decision therefore implements the
registry and verification boundary with deterministic test keys only. A test
key is not trusted production material and does not complete the roadmap item
for embedding the production public key.

### Scope and non-goals

This decision includes schema v1, semantic validation, canonical encoding,
detached signature parsing, injected trust registries, strict verification,
golden fixtures, and negative tests.

It excludes production key provisioning, release-pipeline signing, installed
manifest persistence, HTTPS downloading, archive extraction, candidate bundle
verification, smoke checks, installation discovery, transaction recovery, and
all `ah upgrade` commands.

### Consequences

- Good, because a manifest accepted by one consumer has one reproducible signed
  representation.
- Good, because the focused crate can be reused by the main executable and the
  future helper without exposing updater internals through `aihelper`.
- Good, because schema and algorithm changes require an explicit versioned
  contract rather than permissive parsing.
- Good, because an additive registry supports planned signing-key rotation.
- Bad, because changing field order or serializer behavior is a schema change;
  golden byte fixtures must guard against accidental drift.
- Bad, because an embedded-key-only design cannot retroactively revoke a
  compromised key in already released binaries. Recovery then requires a new
  trusted binary or a separately designed trust-update mechanism.
- Neutral, because release signing cannot become operational until the
  maintainer provisions a production key outside the repository.

## Implementation Plan

- **Affected paths**: add `crates/ah-release-manifest/Cargo.toml`,
  `crates/ah-release-manifest/src/`, and
  `crates/ah-release-manifest/tests/fixtures/`; add the crate to the workspace
  in `Cargo.toml`; update `Cargo.lock`; remove only the completed schema,
  inventory, and signature-format items from the self-update roadmap.
- **Dependencies**: use compatible requirements `ed25519-dalek = "3"`,
  `sha2 = "0.11"`, `base64 = "0.22"`, `semver = "1"`, `url = "2"`,
  `serde = "1"`, `serde_json = "1"`, and `thiserror = "2"`. Do not enable
  signing-key generation or PKCS#8 features in production dependencies.
- **Patterns to follow**: use `#[serde(deny_unknown_fields)]` like the durable
  managed-service models; use typed validation errors; keep text deterministic;
  include one code-to-ADR reference in the new crate root.
- **Patterns to avoid**: no `serde_json::Map` in the signed schema, floats,
  optional omitted fields, input normalization, private-key constants, runtime
  signing functions, network access, archive access, or root CLI changes.
- **Configuration**: add no environment variables, feature flags, user files,
  or runtime configuration. Verification accepts an explicit trust registry.
- **Migration steps**: this is additive. The later production-key block adds a
  real public key to the consumer registry; the later pipeline block consumes
  the same canonical serializer and signs outside the runtime crate.

Implementation order:

1. Add the crate and typed schema with semantic validation.
2. Add canonical serialization and exact-byte parsing.
3. Add signature decoding, trust registry validation, and strict verification.
4. Add deterministic fixtures and the positive and negative test matrix.
5. Run focused and workspace checks.
6. Remove only roadmap items fully proven by the checks.

### Verification

- [x] `cargo test -p ah-release-manifest --locked` passes.
- [x] Golden manifests for Windows x64, Linux x64, and macOS arm64 serialize to
      byte-identical fixtures and verify with a deterministic test key.
- [x] Non-canonical JSON, unknown or duplicate fields, unsupported schemas,
      algorithms, and key IDs are rejected.
- [x] Tampered signed fields and malformed, padded, whitespace-bearing, or
      wrong-length signatures are rejected.
- [x] Invalid SemVer, HTTPS URL, target, digest, size, path, ordering,
      uniqueness, case-collision, purpose, and required-file relationships are
      rejected.
- [x] Production code contains no private signing key or signing API.
- [x] `cargo fmt --all -- --check`,
      `cargo test --workspace --all-targets --locked`, and
      `cargo build --locked` pass.
- [x] The production public-key, pipeline, installed-manifest, downloading,
      extraction, candidate verification, and broader update-test roadmap items
      remain open.

Implementation note (2026-07-22): schema and semantic validation landed in
`3363690`, canonical encoding and platform fixtures in `104ce40`, and the trust
registry plus strict Ed25519 verification in `08c5c3f`. The focused crate test
passed 29 tests, the complete workspace all-target test matrix passed, and the
locked workspace build completed successfully. Signing material is
deterministic and test-only; no production release key exists, and release
pipeline signing and updater behavior remain outside this implementation.

## Pros and Cons of the Options

### AIHelper-specific canonical JSON signed as exact bytes

- Good, because the producer and verifier share a small Rust contract.
- Good, because exact-byte checking makes formatting drift visible.
- Good, because the format needs no extra canonicalization dependency.
- Bad, because serializer and field-order changes must be treated as schema
  changes.

### RFC 8785 JSON Canonicalization Scheme

- Good, because it is a language-independent standard.
- Good, because property order and insignificant formatting do not affect the
  signing representation.
- Bad, because its Unicode and number rules add implementation and dependency
  surface that schema v1 otherwise excludes.
- Bad, because arrays still need explicit ordering and semantic validation.

### TUF-like or separate binary representation

- Good, because TUF-style metadata can support threshold keys, delegation, and
  richer compromise recovery.
- Good, because a binary representation can be structurally unambiguous.
- Bad, because either option introduces substantially more protocol and tooling
  than the current single-maintainer, single-channel v1 requires.
- Bad, because a separate human-readable JSON asset and signing representation
  can drift from each other.

## More Information

Revisit this decision if AIHelper needs online key revocation, threshold
signatures, delegated release roles, update channels, a non-Rust manifest
producer, or managed paths that cannot fit the v1 canonical path alphabet.

When a production key is provisioned, distribute its public half in a release
signed by the currently trusted key before switching the release signer. Never
store, print, or commit the private half.
