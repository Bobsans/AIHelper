# Manage and discover secret references

Initialize the local encrypted vault once:

```bash
ah secrets init
```

Add or edit secrets interactively. Values are entered only through hidden prompts, never through argv:

```bash
ah secrets add billing --kind postgres --label Billing
ah secrets edit billing --description "Production billing database"
```

Agents discover references through the read-only typed/MCP command:

```json
{
  "name": "ah.secrets.list",
  "arguments": {"kind": "postgres"}
}
```

The result contains only `id`, `kind`, `label`, and `description`. Keep the selected ID as a reference; this discovery command does not resolve or expose secret values.

Use the reference directly from supported CLI commands without placing plaintext
credentials in argv:

```bash
ah postgres ping --database billing --credential database=billing
ah http get https://api.example.test/private --credential basic=service-api
```

The slot names are command contracts: PostgreSQL uses `database`; HTTP
`request|get|post|replay` use `basic`. Do not combine a vault mapping with the
corresponding legacy plaintext or environment-backed authentication option.

Remove an obsolete reference explicitly:

```bash
ah secrets remove billing
```
