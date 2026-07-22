# Signed Release Pipeline Design

## Status

Approved on 2026-07-22. This design extends the accepted
[canonical release manifest decision](../decisions/2026-07-22-adopt-canonical-ed25519-release-manifest-v1.md)
without changing its wire contract.

## Context

AIHelper release jobs currently build, smoke-test, and publish one ZIP for each
supported platform. The repository now has a verify-only
`ah-release-manifest` crate, but the workflow does not create manifests or
detached signatures and no production release key has been provisioned.

The next block must make the pipeline implementation ready for signed assets
while failing closed until the maintainer configures real signing material. A
manual `workflow_dispatch` must remain useful without exposing or requiring the
production secret.

## Goals

- Create one canonical manifest and detached Ed25519 signature per release ZIP.
- Derive manifest inventory, sizes, and hashes from the final archive bytes.
- Keep every private-key parser and signing operation outside the shipped `ah`
  binary and the verify-only manifest crate.
- Expose the production seed to one protected GitHub Actions job only.
- Verify every archive, manifest, and signature triplet before publication.
- Require exactly three triplets and fail before asset upload on any mismatch.
- Preserve the current ZIP names and released CLI, JSON, MCP, and plugin ABI
  contracts.

## Non-Goals

- Generating, rotating, backing up, or otherwise provisioning a production key.
- Embedding the production public key in `ah`.
- Publishing with a placeholder or test key.
- Making the already-published GitHub Release page atomic with asset upload.
- Implementing installed-manifest persistence, downloading, extraction,
  candidate activation, or `ah upgrade`.
- Adding signing APIs or private-key formats to `ah-release-manifest`.

## Considered Approaches

### Separate release-only Rust tool and centralized signing job

Add an internal, unpublished `ah-release-tool` workspace binary. It reuses the
typed manifest model and canonical serializer, owns ZIP inspection and signing,
and is never a dependency of `aihelper`. One protected job signs all three
platform assets, then a separate job publishes the verified set without access
to the key.

This is the chosen approach. It keeps one Rust implementation of the signed
contract, limits secret exposure to one runner, and gives the pipeline a single
completeness checkpoint.

### Sign independently in matrix jobs

Each platform could generate and sign its own sidecars while the unpacked
`dist` directory is available. This is simpler locally, but exposes the key to
three operating systems, expands the logging and supply-chain surface, and
cannot prove the complete cross-platform set before individual signed artifacts
exist.

### External signer or OIDC-backed key service

An external service can keep the private key out of GitHub runners entirely.
It is the strongest long-term custody boundary, but requires a separately
operated Ed25519 service that implements the exact AIHelper signing preimage,
authentication, auditing, and recovery. That infrastructure is outside the
current single-maintainer v1 scope.

## Chosen Architecture

### Release-only tool boundary

Add `crates/ah-release-tool` as a `publish = false` binary package. It depends
on `ah-release-manifest`; neither the root `aihelper` package nor the future
update-helper depends on it.

The tool provides two narrow operations:

- `validate-archives` accepts an assets directory and requires the exact three
  supported ZIP names and layouts. It performs no signing and needs no secret.
- `sign-release` accepts the validated assets directory, repository identity,
  and release tag. It reads signing material from step-scoped environment
  variables, creates all manifests and signatures, verifies every triplet, and
  writes a separate output directory containing exactly nine publishable files.

The implementation is split into testable modules for target profiles, ZIP
inventory, signing material, release-set preparation, and the small CLI adapter.
The manifest contract crate gains no signing function. The release tool creates
the signing preimage from the existing public `SIGNING_DOMAIN` constant and
canonical manifest bytes, then uses the existing verifier for the final check.

### Closed target profiles

The workflow matrix and release tool use explicit profiles rather than deriving
target identity from mutable runner labels:

| Asset | Rust target | Architecture | Executable | Plugin suffix |
| --- | --- | --- | --- | --- |
| `ah-windows-x64.zip` | `x86_64-pc-windows-msvc` | `x86_64` | `ah.exe` | `.dll` |
| `ah-linux-x64.zip` | `x86_64-unknown-linux-gnu` | `x86_64` | `ah` | `.so` |
| `ah-macos-arm64.zip` | `aarch64-apple-darwin` | `aarch64` | `ah` | `.dylib` |

Each ZIP must contain exactly the executable and the four released dynamic
plugins. Directory entries may describe those paths but do not enter the
managed-file inventory. File paths are normalized to `/`, sorted, classified by
the closed profile, and passed through manifest semantic validation. Unknown,
missing, duplicate, case-colliding, linked, encrypted, or unsafe entries fail
the operation.

The tool hashes the complete ZIP bytes for `archive.sha256` and streams every
file entry for its managed-file size and digest. `minimum_updater_version` is
set to the release version, which is conservative until a later policy decision
allows an older updater to consume newer bundles.

### Signing material contract

The protected GitHub environment is named `release-signing`. Its secret and
non-secret variable are:

- `AIHELPER_RELEASE_ED25519_SEED_B64URL`: exactly one 32-byte Ed25519 seed encoded
  as 43 ASCII characters of unpadded Base64URL;
- `AIHELPER_RELEASE_ED25519_PUBLIC_KEY_B64URL`: exactly one 32-byte public key in
  the same 43-character encoding.

The tool rejects empty input, whitespace, padding, alternate alphabets, PEM,
PKCS#8, expanded 64-byte keys, and every other length. The public key and key ID
are derived from the seed and must match the separately configured public key.
The manifest key ID is never accepted as an independent input.

This comparison makes the workflow fail closed before the public key is
provisioned and prevents an accidentally replaced seed from silently defining
a new release identity. The same public-key bytes must later be embedded in the
runtime trust registry to complete the separate roadmap item.

The secret is supplied only as step-scoped environment to an already-built
internal binary. It is never placed in argv, a file, an artifact, command
output, error details, or workflow outputs. Decoded secret bytes are zeroized.

### Workflow data flow

1. The existing no-secret matrix builds explicit target profiles, packages and
   smoke-tests the three ZIP files, then uploads one artifact per ZIP.
2. A no-secret validation job downloads the three artifacts, runs
   `validate-archives`, and uploads one normalized archive-set artifact.
3. On `workflow_dispatch`, the workflow ends successfully after validation. It
   does not request the protected environment, sign, or publish anything.
4. On `release: published`, one `release-signing` environment job checks out the
   exact release ref, builds `ah-release-tool` before exposing the secret, and
   runs `sign-release` with step-scoped signing environment variables.
5. The signing operation requires a canonical `v<SemVer>` tag matching the
   workspace package version, constructs final HTTPS GitHub download URLs,
   creates all three sidecar pairs, and verifies every triplet with the derived
   public key.
6. The signing job uploads one explicit verified artifact containing exactly
   the nine expected files. No workspace glob or temporary directory is used.
7. A separate publish job has `contents: write` but no signing environment or
   secret. It checks the exact filenames again and uploads the ZIP, manifest,
   and signature assets to the existing release.

The release event occurs after the GitHub Release page is published. Therefore
this design guarantees that unverified assets do not reach the upload step, but
it cannot retract the page or guarantee atomicity if GitHub fails during a
multi-asset upload. A future tag-to-draft orchestration may strengthen that
boundary without changing the manifest contract.

## Error Handling

- Missing or malformed signing configuration returns a generic deterministic
  error that never includes the provided value.
- An archive-set mismatch names only expected and observed asset filenames.
- ZIP validation errors identify the archive and safe member name when known.
- Any tag, version, target, URL, digest, canonicalization, signature, or
  reconciliation failure prevents creation of the verified release-set artifact.
- Existing released command behavior and diagnostics are unchanged.

## Testing

- Unit tests cover strict seed/public-key decoding, deterministic public-key and
  key-ID derivation, mismatch handling, exact 86-byte signature encoding, and
  absence of a trailing newline.
- Synthetic ZIP tests cover all three profiles and deterministic inventory,
  purpose, required-file, size, digest, URL, version, and canonical ordering.
- Negative ZIP tests cover missing and extra files, unsafe paths, links,
  encryption, duplicates, case collisions, and malformed archives.
- End-to-end tests use a deterministic test-only seed for
  ZIP -> manifest -> signature -> existing verifier -> ZIP reconciliation.
- CLI tests prove that malformed or missing secret input does not appear in
  stdout, stderr, output files, or error text.
- Workflow structure tests prove that dispatch does not reference signing or
  publishing, release signing precedes publication, and the publish input is an
  explicit nine-file set.

## Roadmap and Completion Boundary

After local tests and workflow validation pass, the release-pipeline roadmap
item can be removed. The production-public-key item remains until real public
key bytes are provisioned and embedded. No test key or empty configuration
counts as completion. All installed-manifest and updater items remain open.
