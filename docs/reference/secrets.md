# `ah secrets`

Manage the encrypted local secret vault.

```bash
ah secrets init
ah secrets list [--kind <postgres|http-basic|ssh-key|github-token|gitlab-token>]
ah secrets add <id> --kind <postgres|http-basic|ssh-key|github-token|gitlab-token> [--label TEXT] [--description TEXT] [--open]
ah secrets edit <id> [--label TEXT] [--description TEXT] [--open]
ah secrets remove <id>
```

Use global `--json` for machine-readable output. List and mutation output contains only `id`, `kind`, `label`, and nullable `description`; values and field-state information are never returned.

## Secret fields

`add` and `edit` read values from hidden terminal prompts. Values are never accepted through argv.

- `postgres`: `password`
- `http-basic`: `username`, `password`
- `ssh-key`: `private_key`, optional `passphrase`
- `github-token`: `token`
- `gitlab-token`: `token`

GitHub and GitLab get separate kinds on purpose. A GitHub PAT is meaningless to
GitLab, so a shared kind would buy no reuse while letting an agent pick the wrong
service's token out of one `secrets.list` result; separate kinds make that a
`SECRET_KIND_MISMATCH` instead.

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

A successful browser submission renders a confirmation page with the saved
`id`, `kind`, `label`, and `description`, plus a Close button. The button calls
`window.close()` and falls back to a "This tab can be closed now." hint when the
browser refuses to close a tab it did not open. Callers that do not send an
`Accept` header containing `text/html` keep receiving the redacted-metadata JSON
response. Reloading the confirmation page re-posts a spent capability and fails
with `VAULT_SETUP_CAPABILITY_INVALID`; mint a new one with `--open`.

Both pages are self-contained: styles and the close script are inline, and the
response carries `Content-Security-Policy: default-src 'none'` with a per-response
nonce for that one style block and script, so nothing on a secret entry page can
load or reach anything else.

The pages are served with `Referrer-Policy: same-origin` rather than
`no-referrer`. Under `no-referrer` a browser sends `Origin: null` when the form
posts, and the loopback Host and Origin policy rejects that with
`LOCAL_REQUEST_REJECTED`. The pages load no third-party resources, so
`same-origin` keeps the capability out of every referrer that leaves the server
while preserving the strict Origin check on the submission.

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

The declared slots are:

| Slot | Accepted kind | Commands |
| --- | --- | --- |
| `database` | `postgres` | every operational `postgres.*` command |
| `basic` | `http-basic` | `http.request`, `http.get`, `http.post`, `http.replay` |
| `token` | `github-token` | every `github.*` command |
| `token` | `gitlab-token` | every `gitlab.*` command |

`ssh-key` secrets are stored but no command declares a slot for them yet.

`--credential SLOT=ID` works on the direct CLI for every domain in that table:

```bash
ah postgres query --database app --credential database=billing --sql "select 1"
ah github issues --credential token=work-github
```

The host resolves the mapping and hands the plugin only the resolved values, so
no secret enters AIHelper argv, plugin argv, or invocation logs. A domain that
declares no secret slot rejects `--credential` outright.

For compatibility, PostgreSQL `--password-env` remains available to direct CLI
commands. HTTP MCP callers must use `credentials.basic`; a server process
cannot safely inherit a per-call password environment variable.

GitHub and GitLab MCP callers must use `credentials.token`. An inline `token`
argument is rejected with `INVALID_ARGUMENT`, the same way inline HTTP
credentials are. The `--token` flag and the `GITHUB_TOKEN`, `GH_TOKEN`,
`GITLAB_TOKEN`, and `GL_TOKEN` environment variables keep working for direct CLI
use; see `github.md` and `gitlab.md` for how ambient credentials are bound to the
destination host.

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
