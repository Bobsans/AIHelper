# Signed Release Manifest Foundation Implementation Plan

## Status

The design and governing ADR were approved on 2026-07-22. This plan implements
only the release-manifest contract and verifier described in:

- `docs/plans/2026-07-22-signed-release-manifest-foundation-design.md`;
- `docs/decisions/2026-07-22-adopt-canonical-ed25519-release-manifest-v1.md`.

The production public key, release pipeline, installed manifest, downloads,
archive handling, candidate verification, and upgrade commands remain out of
scope.

## Delivery rules

- Inspect `ah git status` and `ah git changed` before every edit block.
- Preserve `.agents/skills/release/SKILL.md` and any other user change that
  appears while work is in progress.
- Use `ah run check` for focused and workspace commands.
- Keep code and comments in English.
- Keep output deterministic and do not add CLI, JSON, MCP, plugin ABI, user
  configuration, network, or filesystem behavior.
- Commit each task only after its focused checks pass.
- Stage only the paths named by the task.
- Do not edit the roadmap until the complete automated matrix passes.

## Task 1: Add the strict manifest schema and semantic validation

### Files

Create or modify:

```text
Cargo.toml
Cargo.lock
crates/ah-release-manifest/Cargo.toml
crates/ah-release-manifest/src/lib.rs
crates/ah-release-manifest/src/error.rs
crates/ah-release-manifest/src/model.rs
crates/ah-release-manifest/src/validation.rs
crates/ah-release-manifest/tests/schema_validation.rs
```

### Workspace package

Add `crates/ah-release-manifest` to the root workspace member list. The package
uses version `1.1.0`, edition `2024`, and `publish = false` so the workspace
version contract remains uniform without creating a crates.io release surface.

Add compatible dependency requirements:

```toml
semver = "1"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
thiserror = "2"
url = "2"
```

Resolve and update `Cargo.lock` once without `--locked`, then use `--locked` for
all later checks. Do not update unrelated dependency requirements in existing
manifests.

### Public model

Define these schema-v1 types with `Debug`, `Clone`, `PartialEq`, `Eq`,
`Serialize`, `Deserialize`, and `#[serde(deny_unknown_fields)]` on every object:

```text
ReleaseManifest
ReleaseMetadata
ArchiveMetadata
SigningMetadata
ManagedFile
RequiredFiles
FilePurpose
SignatureAlgorithm
```

Use the exact field order and snake_case values from the approved design. Keep
all fields required. Do not add maps, floats, nullable fields, default values,
flattened extension bags, or permissive aliases.

Expose constants for schema version, manifest byte limit, file count limit,
path byte limit, supported target pairs, and signing algorithm. Keep target
identity as signed strings but validate only the three approved target and
architecture pairs.

### Typed errors

Add an internal-contract `ManifestError` with deterministic variants for:

- input too large;
- malformed JSON;
- unsupported schema;
- non-canonical encoding;
- invalid field value or relationship;
- unsupported signature algorithm;
- unknown key;
- invalid trust registry;
- malformed signature;
- invalid signature.

Do not convert these errors into `AppError` or expose them through current CLI
output.

### Validation

Implement `ReleaseManifest::validate()` as a rejecting validator. It must not
trim, normalize, sort, rewrite, or infer input.

Validate:

- schema version exactly `1`;
- canonical Cargo-style SemVer strings for release and minimum updater version;
- the three target and architecture pairs;
- absolute HTTPS archive URL with host and without credentials or fragment;
- nonzero archive size;
- lowercase 64-character hexadecimal SHA-256 values;
- `ed25519` algorithm and key-ID syntax;
- nonempty inventory with at most 4096 entries;
- ASCII managed paths from 1 through 512 bytes;
- allowed path characters, relative structure, and safe segments;
- strict byte ordering, exact uniqueness, and ASCII case-fold uniqueness;
- sorted and unique required executable and plugin paths;
- required paths present in the inventory with compatible purposes;
- at least one required executable.

Use small pure helpers for digest, path, ordered-list, URL, SemVer, target, and
required-reference validation. Each error identifies one stable field path.

### Tests

`schema_validation.rs` builds a valid Windows manifest through typed values and
uses table-driven mutations for every validation rule. Add raw JSON cases for
unknown, duplicate, missing, null, and incorrectly typed fields.

Cover zero-byte managed files as valid and zero-byte archives as invalid. Cover
exact and case-only path collisions separately. Prove invalid values are
rejected rather than normalized.

### Checks

Run:

```text
ah run check cargo fmt --all -- --check
ah run check cargo test -p ah-release-manifest --test schema_validation --locked
ah run check cargo test -p ah-release-manifest --lib --locked
```

Review the diff and commit:

```text
feat: add release manifest schema
```

## Task 2: Add canonical encoding and platform fixtures

### Files

Create or modify:

```text
crates/ah-release-manifest/src/canonical.rs
crates/ah-release-manifest/src/lib.rs
crates/ah-release-manifest/tests/canonical_contract.rs
crates/ah-release-manifest/tests/fixtures/linux-x64.manifest.json
crates/ah-release-manifest/tests/fixtures/macos-arm64.manifest.json
crates/ah-release-manifest/tests/fixtures/windows-x64.manifest.json
```

### Producer encoding

Implement `ReleaseManifest::to_canonical_bytes()`:

1. Run complete semantic validation.
2. Serialize the strict struct with compact `serde_json` output.
3. Return UTF-8 bytes without BOM, whitespace, or trailing newline.

Struct declaration order is the schema-v1 object order. Array order comes from
the already validated typed input. Do not serialize through
`serde_json::Value` or a map.

### Untrusted canonical decoding

Implement a crate-private `decode_canonical_untrusted()` that:

1. Rejects input above 1 MiB.
2. Strictly deserializes the complete typed schema.
3. Serializes the typed value without running semantic validation.
4. Compares the result to the original bytes.
5. Returns an untrusted parsed value only to the signature verifier.

This function must reject BOM, leading or trailing whitespace, property-order
changes, alternate escaping, a trailing newline, and array-order changes. It
must not inspect or trust URL, target, digest, inventory, or required-file
semantics.

### Fixtures

Commit one canonical manifest fixture for every current release asset:

- Windows x64 with `ah.exe` and four DLL plugins;
- Linux x64 with `ah` and four `.so` plugins;
- macOS arm64 with `ah` and four `.dylib` plugins.

Use fixed versions, URLs, sizes, digests, and a syntactically valid all-zero
non-production key-ID sentinel. Task 3 replaces that sentinel atomically in all
three manifests with the deterministic test public-key fingerprint before it
adds signatures. Inventory and required arrays are sorted by exact path bytes.
The Task 2 fixtures contain no signature or private key material.

### Tests

For every fixture:

- parse typed data and reproduce byte-identical fixture contents;
- assert no BOM, insignificant whitespace, or final newline;
- assert schema field and array order;
- reject each non-canonical textual variant;
- prove `to_canonical_bytes()` rejects a semantically invalid typed value.

Keep crate-private decoder cases in `canonical.rs` unit tests. They prove the
decoder can preserve an invalid semantic value for later signature-first
processing without exposing the function or parsed value outside the crate.

### Checks

Run:

```text
ah run check cargo fmt --all -- --check
ah run check cargo test -p ah-release-manifest --test canonical_contract --locked
ah run check cargo test -p ah-release-manifest --locked
```

Review the diff and commit:

```text
feat: canonicalize release manifests
```

## Task 3: Add the trust registry and strict Ed25519 verification

### Files

Create or modify:

```text
crates/ah-release-manifest/Cargo.toml
Cargo.lock
crates/ah-release-manifest/src/lib.rs
crates/ah-release-manifest/src/signature.rs
crates/ah-release-manifest/src/trust.rs
crates/ah-release-manifest/tests/signature_contract.rs
crates/ah-release-manifest/tests/support/mod.rs
crates/ah-release-manifest/tests/fixtures/linux-x64.manifest.json
crates/ah-release-manifest/tests/fixtures/linux-x64.manifest.sig
crates/ah-release-manifest/tests/fixtures/macos-arm64.manifest.json
crates/ah-release-manifest/tests/fixtures/macos-arm64.manifest.sig
crates/ah-release-manifest/tests/fixtures/windows-x64.manifest.json
crates/ah-release-manifest/tests/fixtures/windows-x64.manifest.sig
```

### Dependencies

Add compatible requirements:

```toml
base64 = "0.22"
ed25519-dalek = "3"
sha2 = "0.11"
```

Do not enable random key generation, PKCS#8, PEM, or signing-specific features
for production dependencies. Integration tests may use `SigningKey` from the
same dependency to create deterministic test fixtures in test-only code.

### Trust registry

Implement:

```text
TrustedKey
TrustedKeyRegistry
key_id_for_public_key
```

`TrustedKeyRegistry::new` owns a small ordered collection of verified public
keys. It rejects duplicate IDs, malformed Ed25519 public keys, unsupported
algorithms, and IDs that do not equal
`ed25519-sha256-<sha256(public_key_bytes)>`. Lookup is exact and deterministic.
The registry supports two or more keys without defining online revocation or
version policies.

Do not add a default production registry or public-key constant. No production
key exists yet.

### Detached signature contract

The signature parser accepts exactly 86 ASCII bytes using unpadded Base64URL.
It rejects padding, whitespace, alternate alphabet characters, short or long
input, and decoded values other than 64 bytes.

Build the signed message as:

```text
b"AIHELPER-RELEASE-MANIFEST-V1\0" || canonical_manifest_bytes
```

Use `VerifyingKey::verify_strict` for verification.

### Verification API

Implement:

```text
VerifiedManifest
verify_manifest(manifest_bytes, signature_bytes, &TrustedKeyRegistry)
```

The function must preserve this error and trust order:

1. Bound manifest and signature inputs.
2. Read untrusted schema, algorithm, and key-ID selectors.
3. Reject unsupported schema or algorithm.
4. Strictly parse and require canonical bytes without semantic validation.
5. Resolve the exact trusted key.
6. Parse the detached signature.
7. Verify the domain-separated message.
8. Run complete semantic validation.
9. Return `VerifiedManifest` that owns the validated manifest.

Do not expose the crate-private untrusted decoder. Future trust-sensitive code
must consume `VerifiedManifest`, not raw parsed JSON.

### Test-only signing support

Place one fixed 32-byte test seed and all `SigningKey` usage under
`tests/support/`. Label it explicitly as non-production. Use it to produce the
committed signature fixtures and positive test registry. Replace the Task 2
all-zero key-ID sentinel in every manifest fixture with the computed public-key
fingerprint in the same commit. Do not add a fixture generator binary, example,
build script, runtime signing function, or private key to `src/`.

### Tests

Verify all three platform fixture pairs. Add negative cases for:

- any changed signed byte or typed field;
- another signing key;
- unknown key ID and unsupported algorithm;
- malformed public key, fingerprint mismatch, and duplicate registry ID;
- padding, whitespace, invalid alphabet, truncated, and oversized signature;
- non-canonical manifest with an otherwise matching semantic value;
- invalid semantic content with an invalid signature, proving signature failure
  occurs before semantic validation;
- invalid semantic content signed by the test key, proving validation occurs
  after successful verification;
- a two-key registry and exact key selection.

Search production paths and require no `SigningKey`, private seed, PKCS#8, PEM,
signing function, network request, archive access, or root command change.

### Checks

Run:

```text
ah run check cargo fmt --all -- --check
ah run check cargo test -p ah-release-manifest --test signature_contract --locked
ah run check cargo test -p ah-release-manifest --locked
```

Review the diff and commit:

```text
feat: verify signed release manifests
```

## Task 4: Validate the workspace and update durable records

### Automated checks

Run the complete required validation:

```text
ah run check cargo fmt --all -- --check
ah run check cargo test -p ah-release-manifest --locked
ah run check cargo test --workspace --all-targets --locked
ah run check cargo build --locked
```

If the `ah` transport reaches its request deadline, rerun the exact child
command with a larger process timeout and report the fallback.

### Final audit

Review the complete implementation range and verify:

- no CLI, text, JSON, MCP, or plugin ABI change;
- no `.github/workflows/release.yml` change;
- no production public or private key was invented;
- no `SigningKey`, private seed, PKCS#8, PEM, or signing function occurs below
  `crates/ah-release-manifest/src/`;
- no network, archive, extraction, installation, or updater behavior was added;
- exact canonical bytes and deterministic error ordering are covered;
- `git diff --check` passes;
- `.agents/skills/release/SKILL.md` remains unstaged and untouched.

### ADR completion record

In
`docs/decisions/2026-07-22-adopt-canonical-ed25519-release-manifest-v1.md`,
mark only proven verification checkboxes complete and append a dated
implementation note with commit hashes and validation results. Do not rewrite
the accepted decision or claim production signing is operational.

### Roadmap update

After every check passes, remove exactly these completed items from
`roadmap/managed-mcp-and-self-update.md`:

```text
- [ ] Определить versioned schema подписанного release manifest.
- [ ] Добавить полный список managed-файлов с относительными путями, размерами,
      SHA-256 и назначением.
- [ ] Добавить key ID и формат подписи с каноническим представлением manifest.
```

Keep the production public-key, release-pipeline, installed-manifest, HTTPS,
extraction, candidate verification, smoke check, installation, helper,
transaction, and combined test items open.

### Durable memory

Record a Basic Memory milestone in project `aihelper` that states:

- the completed layer is schema/canonicalization/verification only;
- test signing material is deterministic and non-production;
- no production release key exists;
- the exact automated checks that passed;
- the remaining roadmap boundary.

### Commit

Stage only the ADR and roadmap changes after reviewing their diff. Commit:

```text
docs: update release manifest roadmap
```

## Completion boundary

This plan is complete only when all four commits exist, the required workspace
checks pass, and the roadmap retains the production public-key and pipeline
work. It must not claim a publishable signed release, candidate bundle
verification, or functioning self-update command.
