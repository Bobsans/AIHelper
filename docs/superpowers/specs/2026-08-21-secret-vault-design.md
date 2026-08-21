# Secret Vault for CLI and HTTP MCP

## Goal

Provide one trusted AIHelper server with durable, encrypted secrets that can
be selected by identifier from CLI and MCP tools. Secret values must never be
part of a model-visible tool argument, tool response, command-line argument,
or application log.

The design supports PostgreSQL first and is extensible to HTTP Basic and SSH
credentials without plugin-specific vault implementations. It works on
Windows, macOS, and Linux.

## Non-goals

- No multi-user authorization, OAuth, or per-user isolation. The deployment is
  one trusted service account.
- No project-file scanning by a remote MCP server.
- No secret value in `secrets.list`, tool schemas, examples, errors, or logs.
- No replacement of existing `password_env` behaviour for direct CLI use.

## Storage

`VaultStore` is rooted at `ConfigContext::paths().config_dir` and owns a single
versioned encrypted document, `secrets.v1.json`. The document contains only a
format version, nonce, and AEAD ciphertext. Writes use the existing atomic
JSON-write pattern and an exclusive file lock.

The plaintext document contains records with this shape:

```json
{
  "id": "qa-lms",
  "kind": "postgres",
  "label": "LMS QA PostgreSQL",
  "description": "Read-only QA connection",
  "values": { "password": "..." }
}
```

`id`, `kind`, `label`, and `description` are metadata. `values` is encrypted
with the whole plaintext document. The write path creates a fresh nonce;
authentication failure returns `VAULT_LOCKED` and never attempts partial
recovery.

Use a randomly generated 256-bit data key and AES-256-GCM. The data key is
stored under a stable AIHelper service name in the platform credential store:
Windows Credential Manager, macOS Keychain, or Linux Secret Service. The
implementation uses one cross-platform keyring abstraction. On a headless
host without an available credential store, startup or vault access fails with
`VAULT_KEY_UNAVAILABLE` unless `AH_VAULT_MASTER_KEY` is explicitly supplied to
the server process. This fallback is intentionally operational configuration,
not a per-project environment variable.

## Management interface

CLI commands are host commands:

```text
ah secrets init
ah secrets list
ah secrets add <id> --kind <postgres|http-basic|ssh-key>
ah secrets edit <id>
ah secrets remove <id>
ah secrets add <id> --kind <kind> --open
```

`add` and `edit` prompt for secret fields with terminal echo disabled. The
kind registry defines required fields:

| Kind | Secret fields |
| --- | --- |
| `postgres` | `password` |
| `http-basic` | `username`, `password` |
| `ssh-key` | `private_key`, optional `passphrase` |

`--open` creates a single-use setup capability valid for ten minutes and
prints a URL served by the existing HTTP MCP process. Opening the default
browser is best-effort so the printed URL remains usable on a headless host.
The CLI accepts only the configured HTTP MCP origin, exact `/secrets/setup`
path, and one non-empty `capability` query parameter. The browser page only
creates or edits a specified secret; it never reads an existing value. SSH
private keys use a multiline textarea while all other secret fields use
password inputs. The capability is consumed on success or expiry. It is absent
from MCP discovery, and the capability and secret POST body are excluded from
request logging; the printed URL itself must be treated as sensitive.

MCP exposes only `secrets.list` as a read-only tool. It returns `id`, `kind`,
`label`, and `description`, optionally filtered by kind. It never indicates
whether optional fields have values.

## Plugin contract and resolution

Extend `CommandDescriptor` with `secret_slots`:

```text
SecretSlot {
  name: String,
  accepted_kinds: Vec<String>,
  required: bool,
  description: String,
}
```

The runtime augments schemas for commands declaring slots with:

```json
{
  "credentials": {
    "database": "qa-lms"
  }
}
```

Each key must be a declared slot and each value is a vault record id. Before a
handler runs, the runtime resolves those IDs, checks their kinds and required
fields, and passes resolved values in a dedicated private field of
`TypedInvocationRequest`. They are not inserted into `arguments`; adapters and
logging use a redacted request representation. Dynamic plugins receive the
private field as part of their typed invocation wire format, so this is an
additive, versioned plugin API change.

CLI uses repeatable mappings with the same names:

```text
ah postgres query --credential database=qa-lms --sql "select now()"
ah http get https://api.example --credential basic=company-api
```

PostgreSQL declares optional slot `database` accepting `postgres`. When set,
the plugin supplies the resolved password to `psql` internally as `PGPASSWORD`.
`--password-env` remains compatible for local/direct CLI operation; HTTP MCP
documentation recommends `credentials` instead.

The HTTP request command declares optional slot `basic` accepting `http-basic`
and builds the authorization header internally. A future SSH plugin declares
an `identity` slot accepting `ssh-key`; no SSH execution behaviour is added by
this change.

## Agent experience

`secrets.list` is documented as the discovery step when a credential ID is
unknown. The MCP description generated for a command with slots includes its
slot names, accepted kinds, and this instruction. Examples use only record
IDs. For example:

```json
{"credentials":{"database":"qa-lms"},"sql":"select now()"}
```

This lets an agent list available safe metadata, select a compatible record,
and invoke the command without ever receiving a password.

## Errors and observability

Use deterministic errors:

- `SECRET_REQUIRED`
- `SECRET_NOT_FOUND`
- `SECRET_KIND_MISMATCH`
- `VAULT_LOCKED`
- `VAULT_KEY_UNAVAILABLE`
- `VAULT_SETUP_CAPABILITY_INVALID`

Logs can include command, slot, record ID, and kind, but never values,
authorization headers, ciphertext, private-key paths, or setup capability
tokens.

## Validation

- Vault encryption round trip, tamper rejection, atomic write, and concurrent
  writers.
- Keyring and explicit master-key backends behind a testable abstraction.
- No-value checks for CLI output, MCP output, error text, and structured logs.
- Descriptor/schema validation rejects unknown slots and kind mismatches.
- Postgres resolves a vault password without exposing it in arguments.
- HTTP Basic builds a request from a vault record without exposing its header.
- Existing `password_env` tests remain valid.
- MCP tool descriptions and `secrets.list` guide an agent to a compatible ID.
