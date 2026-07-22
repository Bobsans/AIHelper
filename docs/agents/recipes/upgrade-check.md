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

Source builds without the externally provisioned production trust anchor return
`UPDATER_TRUST` before network access. Do not replace that anchor with a test key.
