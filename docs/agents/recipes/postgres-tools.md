# Inspect and manage PostgreSQL tools

Inspect the selected client toolchain before running database commands:

```bash
ah --json postgres tool status
```

Managed downloads support Windows x64. The default is PostgreSQL 18.6; archives
are verified against a pinned SHA256 before extraction:

```bash
ah postgres tool download
```

Explicit `--version 18.4` downloads remain supported. An existing 18.4 managed
cache remains usable when no 18.6 cache is present. Database commands do not
download tools unless `--ensure-tool` is requested.

Use `ah postgres tool use --path PATH` to select an existing installation.
Tool updates manage client binaries; they do not update a PostgreSQL server.
