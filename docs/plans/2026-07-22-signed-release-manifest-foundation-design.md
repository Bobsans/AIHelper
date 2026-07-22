# Signed Release Manifest Foundation Design

## Status

The design direction and this written specification were approved on
2026-07-22. The governing architecture decision is the
[release manifest ADR](../decisions/2026-07-22-adopt-canonical-ed25519-release-manifest-v1.md).

## Goal

Build the typed, deterministic, and cryptographically verifiable release
metadata foundation needed by the future AIHelper updater. The result is a
small workspace crate that can validate and verify one signed manifest per
platform archive without changing current command behavior.

The completed layer is a **release-manifest contract and verifier**. It is not a
release signer, downloader, archive verifier, updater, or operational key
deployment.

## Scope

This block will:

- add the internal `ah-release-manifest` workspace crate;
- define strict release manifest schema v1;
- define the complete managed-file inventory and required-file relationships;
- validate versions, target identity, HTTPS archive metadata, digests, paths,
  ordering, uniqueness, purposes, and signing metadata;
- serialize one canonical compact JSON representation;
- define the detached Ed25519 signature format and signing preimage;
- verify canonical manifests against an injected public-key registry;
- cover supported targets with golden fixtures and negative tests;
- update only the roadmap items fully completed by this block.

This block will not:

- provision or embed a production public release key;
- include a private signing key, runtime signing API, or production signing
  tool;
- modify `.github/workflows/release.yml`;
- persist an installed manifest;
- download, extract, compare, install, replace, or roll back files;
- add `ah upgrade` commands or change current CLI, text, JSON, MCP, or plugin ABI
  contracts;
- claim that the complete candidate-bundle test item is finished.

## Package boundary

Create this internal workspace package:

```text
crates/ah-release-manifest/
  Cargo.toml
  src/
    lib.rs
    canonical.rs
    error.rs
    model.rs
    signature.rs
    trust.rs
    validation.rs
  tests/
    fixtures/
      linux-x64.manifest.json
      linux-x64.manifest.sig
      macos-arm64.manifest.json
      macos-arm64.manifest.sig
      windows-x64.manifest.json
      windows-x64.manifest.sig
```

The crate is not published independently. It owns the release wire contract,
canonical encoding, and verification types. The root `aihelper` crate does not
re-export it in this block. The future main updater and update-helper depend on
the crate directly.

The crate root contains one link to the governing ADR. No other production file
needs repeated ADR comments.

## Manifest schema v1

The canonical JSON shape is:

```json
{
  "schema_version": 1,
  "release": {
    "version": "1.2.0",
    "target": "x86_64-pc-windows-msvc",
    "architecture": "x86_64"
  },
  "archive": {
    "url": "https://github.com/example/aihelper/releases/download/v1.2.0/ah-windows-x64.zip",
    "size": 123456,
    "sha256": "0000000000000000000000000000000000000000000000000000000000000000"
  },
  "minimum_updater_version": "1.2.0",
  "signing": {
    "key_id": "ed25519-sha256-0000000000000000000000000000000000000000000000000000000000000000",
    "algorithm": "ed25519"
  },
  "files": [
    {
      "path": "ah.exe",
      "size": 123,
      "sha256": "0000000000000000000000000000000000000000000000000000000000000000",
      "purpose": "executable"
    }
  ],
  "required": {
    "executables": ["ah.exe"],
    "plugins": []
  }
}
```

Field order shown above is normative. Nested field order and file-entry field
order are also normative. Every object uses `deny_unknown_fields`; duplicate,
missing, or incorrectly typed fields fail parsing. Schema v1 contains no maps,
floats, null values, or omitted optional fields.

`FilePurpose` is a closed enum:

```text
executable
plugin
support
update_helper
```

The supported release targets in this delivery are:

| Target triple | Architecture | Asset label |
| --- | --- | --- |
| `x86_64-pc-windows-msvc` | `x86_64` | `windows-x64` |
| `x86_64-unknown-linux-gnu` | `x86_64` | `linux-x64` |
| `aarch64-apple-darwin` | `aarch64` | `macos-arm64` |

## Semantic validation

Validation is rejecting, not normalizing. A caller must construct the canonical
value explicitly.

- Manifest input is at most 1 MiB.
- `schema_version` is exactly `1`.
- `release.version` and `minimum_updater_version` are canonical SemVer strings.
- Target and architecture must be one supported pair from the table above.
- Archive URL is absolute HTTPS, has a host, and contains no credentials or
  fragment.
- Archive size is nonzero.
- Every SHA-256 value is exactly 64 lowercase hexadecimal characters.
- `signing.algorithm` is exactly `ed25519`.
- `signing.key_id` is `ed25519-sha256-` followed by 64 lowercase hex
  characters.
- `files` is nonempty, contains at most 4096 entries, and is strictly sorted by
  path bytes.
- Managed paths are 1 to 512 ASCII bytes, use `/`, are relative, and contain
  only letters, digits, `-`, `_`, `.`, and `/`.
- Paths contain no empty, `.` or `..` segment and no leading or trailing `/`.
- Files are unique both exactly and after ASCII case folding.
- Required executable and plugin arrays are strictly sorted, exactly unique,
  case-fold unique, and reference existing file entries.
- Required executable paths have purpose `executable` or `update_helper`;
  required plugin paths have purpose `plugin`.
- At least one required executable exists.

File sizes are unsigned 64-bit integers. Zero-byte managed files are allowed;
archive and extraction size ceilings belong to the later candidate-bundle
policy.

## Canonical encoding

`ReleaseManifest::to_canonical_bytes` validates the typed value and serializes
it with the crate's compact schema-v1 serializer. Output is UTF-8 JSON with no
BOM, insignificant whitespace, or trailing newline.

The internal `decode_canonical_untrusted` path applies the input-size limit,
strictly deserializes the typed schema, serializes it again, and compares the
result byte-for-byte. It does not perform semantic validation or expose the
parsed value as trusted. A semantic equivalent with different whitespace,
property order, escaping, or array order is non-canonical and rejected.

The serializer implementation is part of schema v1. Refactoring field order or
changing serialization output requires a new schema version unless every
golden byte fixture remains unchanged.

## Signature and trust contract

The detached `.manifest.sig` asset contains exactly 86 Base64URL characters
without `=` padding or trailing whitespace. Decoding must produce exactly 64
bytes.

The signed message is:

```text
b"AIHELPER-RELEASE-MANIFEST-V1\0" || canonical_manifest_bytes
```

`TrustedKey` contains a key ID, the `ed25519` algorithm, and a 32-byte public
key. Registry construction rejects duplicate key IDs, malformed public keys,
and IDs whose fingerprint does not match the public key bytes. The registry can
contain multiple keys for additive rotation.

The production crate exposes verification, not signing. Deterministic test
fixtures may use a fixed test signing key under test-only code. That key is
labelled test-only and must never appear in a production registry.

No default production registry is created because no production key has been
provisioned. The roadmap item for embedding a trusted public release key stays
open.

## Verification flow

`verify_manifest(manifest_bytes, signature_bytes, registry)` performs these
steps:

1. Enforce manifest and signature input bounds.
2. Read untrusted schema, algorithm, and key-ID selectors without acting on any
   URL, digest, target, or file path.
3. Parse the complete strict schema and require canonical input bytes.
4. Resolve the exact key ID and algorithm in the supplied registry.
5. Decode the unpadded Base64URL signature and require 64 bytes.
6. Run strict Ed25519 verification over the domain-separated message.
7. Complete semantic validation and return `VerifiedManifest`.

Failures return typed internal errors for malformed input, unsupported schema,
non-canonical encoding, semantic validation, unsupported algorithm, unknown
key, malformed signature, invalid registry, and invalid signature. Error text
is deterministic but is not a released CLI or JSON contract in this block.

`VerifiedManifest` encapsulates a validated manifest. Future updater code must
accept this type at trust-sensitive boundaries instead of a raw parsed
`ReleaseManifest`.

## Test matrix

Golden tests use a deterministic, explicitly test-only Ed25519 key and fixed
fixtures for all three release targets. They prove canonical bytes, signature
format, and successful verification.

Negative tests cover:

- whitespace, BOM, newline, property-order, escaping, and array-order changes;
- unknown, duplicate, missing, null, and incorrectly typed fields;
- unsupported schema and signing algorithm;
- unknown key ID, fingerprint mismatch, malformed public key, and duplicate
  registry entries;
- padded, invalid-alphabet, whitespace-bearing, truncated, and oversized
  signatures;
- a signature from another key and a signature over any changed manifest byte;
- invalid SemVer, target pair, URL, size, digest, key ID, path, ordering,
  duplicate, case-collision, purpose, and required-file relationship;
- a two-key registry demonstrating additive rotation.

Signing fixture helpers compile only for tests. A source search verifies that
production modules do not contain `SigningKey`, a private seed, PKCS#8 decoding,
or a signing function.

## Validation

Run checks in this order:

```text
ah run check cargo fmt --all -- --check
ah run check cargo test -p ah-release-manifest --locked
ah run check cargo test --workspace --all-targets --locked
ah run check cargo build --locked
```

Review the final diff for deterministic output, test-only private material,
released API changes, and accidental edits to the release workflow.

## Documentation and roadmap

After all checks pass, remove these completed roadmap items instead of checking
them off:

```text
- [ ] Определить versioned schema подписанного release manifest.
- [ ] Добавить полный список managed-файлов с относительными путями, размерами,
      SHA-256 и назначением.
- [ ] Добавить key ID и формат подписи с каноническим представлением manifest.
```

Keep these neighboring items open:

```text
- [ ] Встроить доверенный публичный release key в AIHelper.
- [ ] Обновить release pipeline: публиковать архив, manifest и подпись.
```

The broader test item for manifest, signature, extraction, legacy bootstrap,
locking, and transaction recovery also remains open because this block covers
only its manifest and signature subset.
