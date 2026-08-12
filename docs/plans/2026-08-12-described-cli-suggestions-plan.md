# Described CLI Suggestions Implementation Plan

## Objective

Extend the approved human-friendly CLI error renderer so every available correction includes the canonical command description, while preserving text fallbacks and all JSON, MCP, logging, exit-code, and ABI contracts.

## Workstreams

### Stream 1 – Suggestion metadata

- Introduce a structured suggestion value containing the corrected command and optional description.
- Resolve top-level descriptions from host metadata and enabled plugin metadata.
- Resolve plugin subcommand descriptions from the runtime command catalog.
- Definition of done: suggestions never invent descriptions and dynamic plugin catalogs are supported.

### Stream 2 – Console rendering

- Render command and description as a readable two-column `Did you mean` entry.
- Add the scoped AIHelper-version alternative when `ah <domain> version` is suggested.
- Preserve command-only rendering when no description is available.
- Definition of done: ordinary text is self-explanatory and JSON output is byte-contract compatible.

### Stream 3 – Tests and documentation

- Update unit and integration assertions for described domain and subcommand corrections.
- Cover unrelated input, fallback suggestions, and unchanged JSON diagnostics.
- Update user-facing reference documentation.
- Validate with formatting, full workspace tests, build, and `git diff --check` through `ah run check`.

## Risks and mitigations

- Catalog descriptions may be unavailable for legacy plugins: keep the corrected command without a description.
- Description lookup may accidentally change machine-facing errors: keep suggestion metadata exclusively in `AppError` text rendering.
- Long descriptions may make console output noisy: use the catalog summary/title, not extended documentation.
