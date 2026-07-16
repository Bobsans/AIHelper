# Dynamic Plugin Text Formatting Design

## Goal

Provide the same semantic terminal formatting to dynamic plugins that built-in
commands already use, then apply it to structured GitHub and GitLab output
without changing plugin wire contracts, the C ABI, JSON output, or raw content.

PostgreSQL formatting is intentionally deferred to a separate package.

## Shared Formatter Architecture

Move `TextFormatter` and `TextStyle` from the root CLI implementation into
`ah-plugin-api`.

The shared formatter provides:

- automatic stdout and stderr terminal detection;
- `NO_COLOR` support;
- the existing semantic styles: heading, key, success, warning, error, muted;
- an explicit color-enabled constructor for deterministic renderer tests.

The root CLI re-exports the shared formatter from `src/output.rs`, preserving
existing imports such as `crate::output::{TextFormatter, TextStyle}`. Built-in
renderers therefore continue to use one implementation without widespread
call-site changes.

Dynamic plugins import the formatter directly from `ah-plugin-api`. Because the
plugin executes in the host process, its stdout terminal detection and
environment access describe the same output destination as the host.

## Compatibility

This design does not change:

- `GlobalOptionsWire`;
- `InvocationRequest`;
- `InvocationResponse`;
- serialized JSON field names;
- exported C symbols;
- `AH_PLUGIN_ABI_VERSION`;
- plugin API major or minor contract versions.

The formatter is an additive Rust helper compiled into each plugin. Existing
plugin DLLs continue returning plain text and remain loadable. Newly built
plugins continue using the same invocation ABI and can also run under an older
host that prints successful response messages unchanged.

No semantic span protocol is added to `InvocationResponse`. Such a protocol
would centralize final rendering, but it would expand the wire contract and add
migration complexity without being required for the current formatting scope.

## GitHub Formatting

Apply semantic formatting to structured text renderers for:

- repository context;
- issue lists and single-issue summaries;
- issue comment summary metadata;
- release metadata and assets;
- workflows;
- workflow runs and wait results;
- jobs and artifacts;
- workflow dispatch and other concise success messages.

Formatting rules:

- repository names, issue numbers, IDs, tags, branches, workflow paths, asset
  names, and URLs use the key style;
- successful or active states use the success style;
- queued, pending, in-progress, draft, and prerelease states use the warning
  style;
- failed, cancelled, timed-out, or action-required states use the error style;
- closed, skipped, neutral, and unavailable metadata use the muted style;
- issue and workflow titles remain normal source text.

Run logs and extracted log lines remain raw text.

## GitLab Formatting

Apply the same semantic model to structured text renderers for:

- project context;
- issue lists and single-issue summaries;
- issue comment summary metadata;
- full issue metadata headings;
- releases;
- pipelines and wait results;
- pipeline jobs;
- concise mutation success output.

GitLab pipeline and job states map to semantic styles using the same categories
as GitHub. Project names, issue IDs, tags, refs, usernames, design filenames,
and URLs use the key style.

Full issue descriptions and comment bodies remain raw text. Only their
surrounding headings and metadata are formatted. Embedded partial-fetch
warnings use the warning style. Job traces and extracted trace lines remain raw
text.

## Output Boundaries

- ANSI sequences are emitted only for interactive terminal text output.
- `NO_COLOR` disables formatting for host and plugin renderers.
- Redirected, piped, and captured text output remains plain.
- JSON output remains deterministic and unchanged.
- Quiet mode remains silent.
- Plugin diagnostics continue through the host `AppError` renderer.

## Testing

- Move existing formatter tests to `ah-plugin-api` and retain root warning
  renderer coverage.
- Verify the root re-export preserves built-in renderer compilation and tests.
- Add plain-contract and forced-color tests for GitHub render helpers.
- Add plain-contract and forced-color tests for GitLab render helpers.
- Keep raw-body and raw-log assertions to prevent accidental formatting.
- Run formatting, workspace tests, locked debug build, and the relevant plugin
  builds.
- Demonstrate representative GitHub/GitLab formatting only when local command
  execution can produce output without mutating remote state.

## Documentation

Update:

- GitHub and GitLab command references;
- plugin development documentation describing the shared formatter;
- any relevant AI recipes that describe text output.
