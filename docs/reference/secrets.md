# `ah secrets`

Manage the encrypted local secret vault.

```bash
ah secrets init
ah secrets list [--kind <postgres|http-basic|ssh-key>]
ah secrets add <id> --kind <postgres|http-basic|ssh-key> [--label TEXT] [--description TEXT]
ah secrets edit <id> [--label TEXT] [--description TEXT]
ah secrets remove <id>
```

Use global `--json` for machine-readable output. List and mutation output contains only `id`, `kind`, `label`, and nullable `description`; values and field-state information are never returned.

## Secret fields

`add` and `edit` read values from hidden terminal prompts. Values are never accepted through argv.

- `postgres`: `password`
- `http-basic`: `username`, `password`
- `ssh-key`: `private_key`, optional `passphrase`

During `edit`, submit an empty field to retain its stored value.

## Agent discovery

Agents should call the read-only, low-risk MCP tool `ah.secrets.list`, optionally with `{"kind":"postgres"}`, to discover IDs. The tool returns redacted metadata only. Secret resolution is not exposed by this command.

## Storage and test isolation

The encrypted vault is stored as `secrets.v1.json` in the AIHelper config directory and uses the platform keyring by default. Set `AH_CONFIG_DIR` for an isolated config directory. `AH_VAULT_MASTER_KEY` accepts a 64-character hexadecimal test key for controlled test environments; do not pass that key in command arguments.

Stable error codes include:

- `VAULT_NOT_INITIALIZED`
- `VAULT_KEY_UNAVAILABLE`
- `VAULT_LOCKED`
- `VAULT_INVALID_SECRET`
- `VAULT_SECRET_EXISTS`
- `VAULT_SECRET_NOT_FOUND`
- `VAULT_IO`
