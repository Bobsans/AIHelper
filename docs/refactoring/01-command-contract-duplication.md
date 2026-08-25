# 01 — Command Contract Duplication

**Severity: Critical.** This is the single largest tax on extensibility in the
codebase. Everything else is cheaper to fix.

## Findings

### 1.1 One command is authored four to five times

Adding `git.tags` required writing, by hand, in four different notations:

| Representation | Location | Example |
|---|---|---|
| clap argument struct | [`src/commands/git.rs:40`](../../src/commands/git.rs) | `struct TagsArgs { latest: bool }` |
| JSON→struct mapper | [`src/commands/git.rs:145`](../../src/commands/git.rs) | `"git.tags" => GitCommand::Tags(TagsArgs { latest: typed_bool(...) })` |
| input JSON Schema | [`src/commands/git.rs:239`](../../src/commands/git.rs) | `json!({"type":"object","properties":{"latest":{...}}})` |
| output JSON Schema | [`src/commands/git.rs:250`](../../src/commands/git.rs) | 20 lines mirroring `GitTagsOutput` |
| plugin manual entry | [`src/plugins.rs:340`](../../src/plugins.rs) | `git_manual()` restates the same description |
| user documentation | `docs/reference/git.md` | a sixth, entirely manual copy |

Scale: 41 `CommandDescriptor::new` call sites and 500+ `json!({...})` literals
(142 in `plugins/ah-plugin-github/src/typed.rs` alone).

### 1.2 The output schema is a hand-maintained mirror of a Rust type *(resolved for built-in domains)*

`GitTagsOutput` already derives `Serialize`. The output schema next to it repeats
every field name, type, and `required` entry by hand. Nothing links them.

The only thing that notices divergence is
[`crates/ah-runtime/src/typed.rs:125`](../../crates/ah-runtime/src/typed.rs)
`validate_response`, at runtime, in the session of whoever ran the command, as
`OUTPUT_SCHEMA_VIOLATION`. A field renamed in the struct and forgotten in the
schema is a production error, not a compile error.

**Status:** every built-in domain now derives its output schema from the payload
type through `ah_plugin_api::schema::output_schema_for`, so the two cannot
disagree. ~850 lines of hand-written schema deleted. Host commands and dynamic
plugins still hand-write theirs.

### 1.3 Argument extraction is re-implemented per module

`required_string` appears 8 times, `optional_string` 6, `bool_or` 4, each with
slightly different error text and default handling
([`src/commands/git.rs:212`](../../src/commands/git.rs),
[`plugins/ah-plugin-github/src/typed.rs:548`](../../plugins/ah-plugin-github/src/typed.rs), …).
These are a worse `serde::Deserialize`, written by hand, per domain.

### 1.4 The manual is a third description of the same thing

[`src/plugins.rs:187–596`](../../src/plugins.rs) is ~400 lines of `PluginManual`
literals whose descriptions duplicate the clap `about` strings and the descriptor
summaries.

**Correction:** the original claim that nothing kept the three consistent was
wrong. `assert_examples_parse` already ran every manual example through the
domain's clap parser, for all eight domains — a stronger check than it looks,
because it proves the documented argv is actually accepted.

What genuinely had no mechanism was the rest of the entry: a documented command
that no longer exists, and a `usage` line naming a flag that had been renamed
away. Both are checked now, in the same helper.

**What cannot be derived**, and why the roadmap row was wrong to say "generate
from descriptors": the manual is CLI-shaped (`usage: "read <path> [-n] …"`,
`argv: ["read", "src/main.rs", "-n"]`) while a descriptor is MCP-shaped (JSON
arguments). Producing one from the other means synthesising CLI syntax from a
JSON Schema. The clap command *can* supply `name`, `summary` and `usage` — but
the manual summary and the clap `about` deliberately differ in wording
("Read file content with optional line range and numbering." against
"Read file content (supports line range and numbering)"), so generating them
would rewrite agent-facing text. That is a product decision, not a refactor.

### 1.5 Documentation has no coupling to the code at all

Searching the Rust sources for `docs/reference` returns nothing. There is no test,
no generator, no check. `docs/reference/*.md` and `docs/agents/recipes/*.md` are
drift by construction, and `AGENTS.md` correctly but expensively compensates by
*instructing humans to remember*.

## Why it hurts

- **Cost per command is ~5 coordinated edits in ≥4 files.** That cost is paid by
  every future contributor, forever, and is the main reason the plugin files grew
  to 3–4k lines.
- **Drift is detected late and in the wrong place** — at runtime, by the end user.
- **Refactoring is discouraged.** Renaming an output field means editing a struct,
  a schema, a manual, and a doc page; the rational move becomes "do not rename".
- **Third-party plugin authors inherit the same tax**, which caps the plugin
  ecosystem the architecture was built to enable.

## Target design

**One declarative definition per command; everything else is derived.**

```rust
// One source of truth, colocated with the domain type.
#[derive(Debug, clap::Args, serde::Deserialize, schemars::JsonSchema)]
#[command_meta(
    id = "git.tags",
    summary = "List Git tags",
    description = "List repository tags newest-first…",
    effects = read_only(risk = Low, reversible = Yes),
)]
pub struct TagsArgs {
    /// Return at most the newest tag.
    #[arg(long)]
    #[serde(default)]
    pub latest: bool,
}

#[derive(Serialize, schemars::JsonSchema)]
pub struct GitTagsOutput { /* … */ }
```

Derived artifacts:

| Artifact | Derived from | Mechanism |
|---|---|---|
| clap `Command` | `#[derive(Args)]` | already the case |
| typed argument decoding | `#[derive(Deserialize)]` | replaces hand-written `typed_args` |
| input JSON Schema | `#[derive(JsonSchema)]` | `schemars` |
| output JSON Schema | `#[derive(JsonSchema)]` on the output type | `schemars` |
| `CommandDescriptor` | `#[command_meta]` + the two schemas | one proc-macro or builder |
| manual entry | doc comments + `#[command_meta]` | generator |
| `docs/reference/<domain>.md` | the catalog | self-documenting generator + CI diff check |

Two structural consequences worth calling out:

- `fn typed_args(request) -> Result<Args, AppError>` (per domain, ~100 lines each)
  disappears entirely — it becomes `serde_json::from_value::<Args>(request.arguments)`.
- The schema is guaranteed to match the type, so `OUTPUT_SCHEMA_VIOLATION` becomes
  a genuine plugin-contract violation rather than a self-inflicted maintenance bug.

## Migration

Each step is independently shippable and behavior-preserving.

1. **Freeze current behavior.** Add a golden-snapshot test that serializes the full
   command catalog (all descriptors, both schemas, effects, examples) and all plugin
   manuals to a checked-in file. Any later step that changes a byte fails loudly.
   *This step is a prerequisite for every subsequent step in this document.*
2. **Introduce `schemars`** in `ah-plugin-api` behind a feature flag; add a helper
   that normalizes generated schemas to the current project style
   (`additionalProperties: false`, ordering, no `$schema` key — the runtime already
   strips `$schema`, see `crates/ah-runtime/src/typed.rs:10`).
3. **Convert output schemas first**, one domain at a time. Output schemas are pure
   description; converting them cannot change request handling. The golden snapshot
   proves each conversion is byte-identical or shows exactly what shifted.
4. **Convert input schemas and replace `typed_args`** with `serde` deserialization.
   Add `#[serde(deny_unknown_fields)]` to preserve the current `additionalProperties: false`.
5. **Collapse the argument helpers** (`required_string` and friends) as their call
   sites disappear; delete the duplicates.
6. **Generate the manual** from descriptors; delete the literals in `src/plugins.rs`.
7. **Generate `docs/reference/*.md`** from the catalog; add a CI job that regenerates
   and fails on diff. Hand-written prose moves to per-command doc comments so it lives
   next to the code.
8. **Repeat for dynamic plugins**, which benefit most (142 `json!` literals in the
   GitHub plugin alone).

## What the conversion has actually found

Deriving is not only cheaper to maintain; it surfaced defects that the
hand-written schemas had been hiding.

- **`gitlab.issues` and `gitlab.pipelines` published schemas that rejected their
  own payload.** Both listed four properties with `additionalProperties: false`
  while the payload type serialized ten and five. The runtime validates every
  typed response against the descriptor, so both commands returned
  `OUTPUT_SCHEMA_VIOLATION` on every call. Neither is covered by a test, because
  both need the network. This is finding 1.2 happening in production.
- **Two enums existed only in prose.** `file.stat.kind` and
  `plugins.list.source`/`state` were `&'static str` whose legal values lived in
  the schema alone. They are real enums now.
- **The normalizer itself had a bug of the same shape, twice.** Stripping
  `title`/`format` and stripping `default` both recursed into `properties`
  without knowing its keys are *property names*, so a property genuinely called
  `title` or `default` was deleted from the published schema while remaining in
  `required`. It hit `gitlab.issue.create.title` and
  `postgres.describe.columns[].default`. Both walks now share one traversal that
  knows which keywords are name-keyed maps, with regression tests.

The last one is worth stating plainly: a generator can be wrong in ways a
hand-written literal cannot. What makes it better is not that it cannot fail, but
that when it does, it fails the same way everywhere and one fix covers every
command — and the golden snapshots make the failure visible.

## Risks and invariants

- **Schema byte-stability.** `schemars` output ordering and phrasing will not match
  the hand-written schemas exactly. Mitigation: the normalization layer in step 2 plus
  the golden snapshot; treat every diff as a deliberate decision, not a rounding error.
- **Released JSON field names must not move.** Deriving schemas from types makes this
  *safer*, not riskier — but the snapshot test is what enforces it.
- **ABI.** Nothing here changes the C ABI: descriptors are still serialized JSON
  crossing the boundary. Only their authoring changes.
- **Proc-macro cost.** Prefer a builder plus `schemars` before writing a custom derive.
  A custom `#[command_meta]` macro is justified only once the builder is proven repetitive.

## Acceptance criteria

- Adding a new typed command requires editing **one** file and **one** struct.
- No `json!({...})` literal describes a type that also exists as a Rust struct.
- `docs/reference/*.md` is generated; CI fails if it is stale.
- `typed_args`-style hand mappers no longer exist in any domain or plugin.
- The catalog golden snapshot is unchanged by the whole migration, except for
  deliberately recorded diffs.
