# Check for a Trusted AIHelper Update

Use the built-in read-only updater check:

```text
ah upgrade --check --json
```

Read these stable fields:

- `status`: `up_to_date`, `update_available`, or `current_newer`;
- `current_version`: the running AIHelper SemVer;
- `selected_version`: the highest trusted stable release;
- `target`: `x86_64-pc-windows-msvc` in updater v1;
- `source`: `github_release`.

Treat any `UPDATER_*` diagnostic as a failed check, not as evidence that the
current version is up to date. In particular, do not fall back to an older
release when the highest stable release has missing or invalid signed assets.

The check is safe to run while managed MCP is active: it does not download the
archive, modify files or service state, acquire lifecycle locks, or stop a
process.

To install after a successful check, run `ah upgrade --json`, or use
`ah upgrade --version <VERSION> --json` for an exact stable non-downgrade
release. Treat `activation_launched` as successful helper handoff, not as proof
that the asynchronous activation has already finished; the next `ah` startup
performs any required durable recovery before loading configuration or plugins.

After a successful update, `ah upgrade --rollback --json` restores and consumes
the one verified permanent backup without network access. If rollback fails, the
pre-rollback installation and permanent backup remain available for recovery or
a retry.

Source builds without the externally provisioned production trust anchor return
`UPDATER_TRUST` before network access. Do not replace that anchor with a test key.
