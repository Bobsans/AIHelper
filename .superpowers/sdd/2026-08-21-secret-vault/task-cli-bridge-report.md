# Direct CLI vault credential bridge report

## Status

Complete. Direct HTTP and PostgreSQL CLI commands now opt into the existing
host-side typed credential resolver when `--credential SLOT=ID` is present.
Invocations without the flag keep the legacy argv path unchanged.

## Implementation

- Added strict host parsing for repeatable `--credential SLOT=ID` and
  `--credential=SLOT=ID` mappings. Empty, malformed, and duplicate slots fail
  before plugin execution without echoing the ID.
- Added a backward-compatible optional dynamic-plugin symbol,
  `ah_plugin_argv_to_typed_json_v1`. Absence returns `None` and leaves legacy
  invocation available. The ABI version and Plugin API version remain unchanged.
- PostgreSQL exports the optional converter and reuses its own clap parser to
  create a stable command ID plus secret-free public JSON. The host adds the
  `database` credential reference and calls `PluginManager::invoke_typed`.
- HTTP reuses the existing concrete clap parser and a command-specific converter
  for `request`, `get`, `post`, and `replay`; no generic schema-to-argv parser was
  introduced.
- The host supplies cwd, limit, an unbounded direct-CLI deadline, and the existing
  text/JSON/quiet rendering behavior. HTTP typed data is routed through the
  existing request output adapter; PostgreSQL typed responses reuse its text
  renderers where applicable.
- Vault values remain inside the existing resolver/typed request boundary.
  PostgreSQL supplies the resolved password only to the child `psql` process as
  `PGPASSWORD`; neither plugin receives vault access.
- Credential IDs are always redacted, including with `AH_LOG_UNREDACTED=1`.
  Credentialed invocation diagnostics and other sensitive argv flags are also
  redacted to prevent an unexpected secret from reaching invocation logs.
- PostgreSQL rejects `database` plus `password_env`; HTTP retains its typed-path
  conflicts for legacy Basic/Bearer/Authorization/curl authentication.
- Updated HTTP, PostgreSQL, secret-agent, and plugin-development documentation.

## Tests

- RED first: both direct HTTP and PostgreSQL integration tests initially failed
  because plugin clap rejected `--credential`.
- End-to-end HTTP test uses an explicit test vault and local authenticated server;
  it verifies the resolved Basic header, normal body output, and stdout/stderr/log
  redaction.
- End-to-end PostgreSQL test uses an explicit test vault and compiled fake `psql`;
  it verifies exact text output and that only child `PGPASSWORD` contains the
  password.
- Added malformed/duplicate mapping, unredacted-log, unexpected-secret-log,
  PostgreSQL conflict, and optional-ABI compatibility coverage.

Fresh final checks:

```text
ah run check cargo fmt --all -- --check
ah run check --timeout-secs 1800 cargo test --workspace --all-targets --locked
ah run check --timeout-secs 900 cargo build --locked
ah run check --timeout-secs 1800 cargo build --release --locked
ah run check --timeout-secs 900 cargo build --release -p ah-plugin-postgres --locked
```

All checks passed. The first full workspace test exposed only the intentional API
version assertion mismatch from a provisional minor-version bump; the bump was
removed to preserve old-host compatibility, and the complete workspace suite then
passed.
