# `ah secrets`

Manage the encrypted local secret vault.

```bash
ah secrets init
ah secrets list [--kind <postgres|http-basic|ssh-key>]
ah secrets add <id> --kind <postgres|http-basic|ssh-key> [--label TEXT] [--description TEXT] [--open]
ah secrets edit <id> [--label TEXT] [--description TEXT] [--open]
ah secrets remove <id>
```

Use global `--json` for machine-readable output. List and mutation output contains only `id`, `kind`, `label`, and nullable `description`; values and field-state information are never returned.

## Secret fields

`add` and `edit` read values from hidden terminal prompts. Values are never accepted through argv.

- `postgres`: `password`
- `http-basic`: `username`, `password`
- `ssh-key`: `private_key`, optional `passphrase`

During `edit`, submit an empty field to retain its stored value.

## Protected browser setup

Start the local HTTP MCP server, then add or edit with `--open`:

```text
ah mcp serve --transport http --port 8787
ah secrets add billing --kind postgres --open
ah secrets edit billing --open
```

`--open` asks the loopback server to mint a random 256-bit capability, prints
the returned setup URL, and makes a best-effort attempt to open it in the
default browser. Copy the printed URL into a browser when running headless or
when automatic opening is unavailable. The capability expires after ten
minutes, is accepted only by the matching create/edit form, and is consumed only
after a successful POST. The page never reads or pre-fills an existing value;
an empty edit field retains its stored value. SSH private keys use a multiline
textarea; all other fields remain password inputs. Responses contain only
redacted metadata.

The default server origin is `http://127.0.0.1:8787`. For a custom loopback
port, set `AH_MCP_HTTP_URL` to an exact `http://127.0.0.1:PORT` origin before
running `ah secrets ... --open`. AIHelper rejects remote, TLS, path, query, and
fragment values. It also rejects any returned setup URL that does not have the
same origin, the exact `/secrets/setup` path, and exactly one non-empty
`capability` query parameter. Capability tokens and form bodies are excluded
from AIHelper logs. Treat the printed setup URL as sensitive because it contains
the one-time capability.

## Agent discovery

Agents should call the read-only, low-risk MCP tool `ah.secrets.list`, optionally with `{"kind":"postgres"}`, to discover IDs. The tool returns redacted metadata only. Secret resolution is not exposed by this command.

Tools with credential slots name every accepted kind in their generated
description and direct agents to the matching `secrets.list` kind filter. Pass
only the selected ID, for example `{"credentials":{"database":"billing"}}` or
`{"credentials":{"basic":"internal-api"}}`. `tools/list` never embeds live
record IDs, and `/mcp` never accepts or returns secret values.

For compatibility, PostgreSQL `--password-env` remains available to direct CLI
commands. HTTP MCP callers must use `credentials.database`; a server process
cannot safely inherit a per-call password environment variable.

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
- `VAULT_SETUP_CAPABILITY_INVALID`
- `VAULT_SETUP_UNAVAILABLE`
