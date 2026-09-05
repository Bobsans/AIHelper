# Manage and discover secret references

Initialize the local encrypted vault once:

```bash
ah secrets init
```

Add or edit secrets interactively. Values are never accepted through argv; password-like values use hidden prompts:

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
ah postgres ping --credential database=billing
ah http get https://api.example.test/private --credential basic=service-api
```

The slot names are command contracts: PostgreSQL uses `database`; HTTP
`request|get|post|replay` use `basic`. Do not combine a vault mapping with the
corresponding legacy plaintext or environment-backed authentication option.

New PostgreSQL entries require `host`, `user`, and `password`. They may also
store `port`, `database`, and `sslmode`. Explicit connection flags override
stored values. Existing password-only entries remain valid for compatibility.

Terminal SSH private-key entry reads hidden lines until a line containing only
`.`. The protected browser form provides a multiline textarea instead.

Remove an obsolete reference explicitly:

```bash
ah secrets remove billing
```
