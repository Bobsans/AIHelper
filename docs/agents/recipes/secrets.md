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

Remove an obsolete reference explicitly:

```bash
ah secrets remove billing
```
