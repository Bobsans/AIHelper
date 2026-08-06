# Production Updater Trust Anchor Design

## Goal

Activate the externally provisioned Ed25519 release key for production updater
verification without exposing signing material or adding runtime configuration.

## Design

Embed one compile-time `ReleaseTrustAnchor` containing the public key bytes and
its canonical `ed25519-sha256-<sha256(public-key)>` identifier. Continue using
the existing `TrustedKeyRegistry` validation so a mismatched identifier, invalid
key, or duplicate future rotation entry fails closed during construction.

The 32-byte seed remains only in the protected GitHub `release-signing`
environment. The runtime receives no secret and performs no Base64 decoding,
file lookup, environment lookup, or network access to establish trust.

## Validation

- Replace the temporary empty-registry test with a test that constructs the
  production trust registry and requires exactly one key.
- Run updater-core tests followed by the required workspace format, test, debug
  build, and release build checks.
- Update updater documentation to distinguish completed key activation from the
  still-open real-release and Windows VM acceptance matrices.
