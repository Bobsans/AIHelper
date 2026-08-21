# Secret Vault Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a cross-platform encrypted secret vault usable safely by AIHelper CLI and HTTP MCP tools.

**Architecture:** A host-owned `VaultStore` encrypts all secret values with an AES-256-GCM data key held in the operating-system keyring (or an explicit server master key). Typed command descriptors declare credential slots; the runtime resolves public record IDs into private invocation values after input validation. Built-in and dynamic plugins use the same private field while MCP exposes only redacted discovery metadata.

**Tech Stack:** Rust 2024, `aes-gcm`, `keyring`, `fs2`, `serde_json`, `clap`, `axum`/`rmcp`, existing typed plugin ABI.

**Spec:** `docs/superpowers/specs/2026-08-21-secret-vault-design.md`

## Global Constraints

- Support Windows Credential Manager, macOS Keychain, and Linux Secret Service through one keyring abstraction.
- On keyring failure, only `AH_VAULT_MASTER_KEY` may supply the server data key; do not fall back to plaintext storage.
- Never put a secret value, authorization header, ciphertext, private-key path, or setup capability token in CLI output, MCP output, typed arguments, error detail, or logs.
- Preserve `postgres --password-env` compatibility for direct CLI operation.
- Keep `cwd` optional for non-file PostgreSQL operations.
- Do not add OAuth or multi-user authorization.

---

## File Structure

- `src/secrets/mod.rs` — vault types, redacted metadata, and public service interface.
- `src/secrets/store.rs` — encrypted document read/write, locks, and atomic persistence.
- `src/secrets/key_provider.rs` — keyring and explicit-master-key implementations.
- `src/secrets/kinds.rs` — built-in kinds and their required secret fields.
- `src/secrets/setup.rs` — expiring single-use browser setup capabilities.
- `src/commands/secrets.rs` — typed `secrets.list` host plugin and CLI-facing operations.
- `src/cli.rs`, `src/runtime_flow.rs`, `src/lib.rs`, `src/host_commands.rs` — CLI routing and host-plugin registration.
- `crates/ah-plugin-api/src/lib.rs` — `SecretSlot` and private resolved-secret wire field.
- `crates/ah-runtime/src/typed.rs`, `crates/ah-runtime/src/lib.rs` — schema augmentation and runtime resolution.
- `crates/ah-mcp/src/server.rs` — credential guidance plus protected browser setup routes.
- `plugins/ah-plugin-postgres/src/{lib.rs,typed.rs}` — `database` slot, `--credential`, and internal `PGPASSWORD` injection.
- `src/commands/http.rs`, `src/commands/http/domain.rs` — `basic` slot and internal Basic auth construction.
- `docs/reference/mcp.md`, `docs/agents/recipes/mcp-http.md`, `docs/reference/secrets.md` — user and agent reference.

### Task 1: Define the versioned credential contract

**Files:**
- Modify: `crates/ah-plugin-api/src/lib.rs:388-468`
- Modify: `crates/ah-runtime/src/typed.rs`
- Test: `crates/ah-plugin-api/src/lib.rs`
- Test: `crates/ah-runtime/src/typed.rs`

**Interfaces:**
- Produces: `SecretSlot`, `ResolvedSecret`, and `TypedInvocationRequest::resolved_secrets`.
- Produces: augmented `credentials` input schema only for descriptors with slots.

- [ ] **Step 1: Write failing API serialization and schema tests**

```rust
#[test]
fn descriptor_serializes_declared_secret_slots() {
    let descriptor = CommandDescriptor::new(/* existing fields */)
        .with_secret_slot(SecretSlot::optional("database", ["postgres"], "Database credential"));
    let raw = serde_json::to_value(&descriptor).unwrap();
    assert_eq!(raw["secret_slots"][0]["name"], "database");
}

#[test]
fn schema_adds_credentials_only_when_slots_are_declared() {
    let schema = schema_with_context(&descriptor_with_database_slot()).unwrap();
    assert_eq!(schema["properties"]["credentials"]["properties"]["database"]["type"], "string");
}
```

- [ ] **Step 2: Run the focused tests and verify failure**

Run: `ah run check cargo test -p ah-plugin-api -p ah-runtime secret_slot --locked`

Expected: failure because `SecretSlot` and `with_secret_slot` do not exist.

- [ ] **Step 3: Add additive descriptor and request fields**

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SecretSlot {
    pub name: String,
    pub accepted_kinds: Vec<String>,
    pub required: bool,
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ResolvedSecret {
    pub id: String,
    pub kind: String,
    pub values: BTreeMap<String, String>,
}
```

Add `#[serde(default)] pub secret_slots: Vec<SecretSlot>` to `CommandDescriptor` and `#[serde(default)] pub resolved_secrets: BTreeMap<String, ResolvedSecret>` to `TypedInvocationRequest`. Update its constructor to initialize an empty map and add `with_resolved_secrets` for the runtime.

- [ ] **Step 4: Augment typed schemas and validate slot declarations**

Reject duplicate slot names, empty kinds, and invalid slot names during catalog validation. Add a `credentials` object with only declared slot names and string ID values to the already context-augmented schema; it must be optional unless one or more slots are marked required.

- [ ] **Step 5: Run focused tests and format**

Run: `ah run check cargo fmt --all -- --check`

Run: `ah run check cargo test -p ah-plugin-api -p ah-runtime secret_slot --locked`

Expected: PASS.

- [ ] **Step 6: Commit**

```text
git add crates/ah-plugin-api/src/lib.rs crates/ah-runtime/src/typed.rs
git commit -m "feat: add typed credential slots"
```

### Task 2: Implement encrypted vault storage and key providers

**Files:**
- Modify: `Cargo.toml`, `Cargo.lock`, `src/lib.rs`
- Create: `src/secrets/mod.rs`, `src/secrets/store.rs`, `src/secrets/key_provider.rs`, `src/secrets/kinds.rs`
- Test: modules above

**Interfaces:**
- Consumes: `ConfigContext::paths().config_dir`.
- Produces: `VaultStore::{initialize,list_metadata,put,remove,resolve}` and `VaultError` codes.

- [ ] **Step 1: Write failing round-trip and tamper tests**

```rust
#[test]
fn vault_round_trip_returns_metadata_and_resolves_values() {
    let vault = test_vault();
    vault.put(NewSecret::postgres("qa-lms", "LMS QA", "password")).unwrap();
    assert_eq!(vault.list_metadata(None).unwrap()[0].id, "qa-lms");
    assert_eq!(vault.resolve("qa-lms").unwrap().values["password"], "password");
}

#[test]
fn modified_ciphertext_is_rejected_without_leaking_data() {
    let vault = test_vault();
    vault.put(NewSecret::postgres("qa-lms", "LMS QA", "password")).unwrap();
    corrupt_vault_file(vault.path());
    assert_eq!(vault.list_metadata(None).unwrap_err().code(), "VAULT_LOCKED");
}
```

- [ ] **Step 2: Run and verify the tests fail**

Run: `ah run check cargo test vault_round_trip --locked`

Expected: failure because `src/secrets` is absent.

- [ ] **Step 3: Add minimal secure storage dependencies and types**

Add `aes-gcm` with the `aes` feature and `keyring` to root dependencies. Define `SecretKind` validation for `postgres`, `http-basic`, and `ssh-key`, including exact required fields. Use `BTreeMap<String, String>` for values to keep deterministic serialization before encryption.

- [ ] **Step 4: Implement `KeyProvider` and `VaultStore`**

```rust
pub trait KeyProvider: Send + Sync {
    fn load_or_create(&self) -> Result<[u8; 32], VaultError>;
}

pub fn resolve_key_provider() -> Result<Box<dyn KeyProvider>, VaultError> {
    if let Ok(value) = std::env::var("AH_VAULT_MASTER_KEY") {
        return Ok(Box::new(ExplicitMasterKey::parse(value)?));
    }
    Ok(Box::new(SystemKeyring::new("aihelper", "vault-v1")))
}
```

Encrypt the complete serialized document using a new random 96-bit nonce on every write. Obtain an exclusive `fs2::FileExt` lock before read-modify-write; write via `atomic_write_json`. Map keyring failures to `VAULT_KEY_UNAVAILABLE` and authentication failures to `VAULT_LOCKED`.

- [ ] **Step 5: Add edge-case tests**

Test duplicate IDs, kind-required fields, empty labels, missing file before initialization, two concurrent write attempts, and redacted metadata serialization. Assert every error string lacks the sample password.

- [ ] **Step 6: Run focused tests and format**

Run: `ah run check cargo fmt --all -- --check`

Run: `ah run check cargo test secrets:: --locked`

Expected: PASS.

- [ ] **Step 7: Commit**

```text
git add Cargo.toml Cargo.lock src/lib.rs src/secrets
git commit -m "feat: add encrypted secret vault"
```

### Task 3: Add CLI management and host discovery command

**Files:**
- Modify: `Cargo.toml`, `Cargo.lock`, `src/cli.rs`, `src/runtime_flow.rs`, `src/host_commands.rs`, `src/ai.rs`
- Create: `src/commands/secrets.rs`
- Test: `src/cli.rs`, `src/commands/secrets.rs`, `tests/integration/secrets.rs`

**Interfaces:**
- Consumes: `VaultStore` from Task 2.
- Produces: CLI `ah secrets init|list|add|edit|remove` and typed MCP command `secrets.list`.

- [ ] **Step 1: Write CLI and typed discovery tests**

```rust
#[test]
fn secrets_list_mcp_response_has_no_value_fields() {
    let response = invoke_typed("secrets.list", json!({"kind":"postgres"}));
    assert_eq!(response.data["secrets"][0]["id"], "qa-lms");
    assert!(response.data.to_string().contains("password") == false);
}

#[test]
fn add_prompts_without_echo_and_returns_only_metadata() {
    let output = run_ah_with_tty("secrets add qa-lms --kind postgres");
    assert!(!output.contains("secret-password"));
}
```

- [ ] **Step 2: Run and verify failure**

Run: `ah run check cargo test secrets_list_mcp_response --locked`

Expected: failure because the `secrets` host command is not registered.

- [ ] **Step 3: Route host CLI commands**

Add `dialoguer` and use `dialoguer::Password` for `add`/`edit`, so values are read without terminal echo and never accepted as command arguments. Add `HOST_COMMAND_SECRETS`, `RuntimeCommand` variants, and Clap subcommands for `init`, `list`, `add`, `edit`, and `remove`. Add `--open` as a setup-capability request, not an inline password mode.

- [ ] **Step 4: Register `SecretsHostPlugin`**

Expose only descriptor `secrets.list` to typed/MCP dispatch. It accepts optional `kind` and returns a deterministic array of `id`, `kind`, `label`, and nullable `description`. Mark it read-only, low risk, and add an agent-facing example that filters by `postgres`.

- [ ] **Step 5: Verify CLI and MCP contract**

Run: `ah run check cargo test secrets --locked`

Run: `ah run check cargo test --test integration secrets --locked`

Expected: PASS; command output and serialized MCP response contain no supplied secret text.

- [ ] **Step 6: Commit**

```text
git add Cargo.toml Cargo.lock src/cli.rs src/runtime_flow.rs src/host_commands.rs src/ai.rs src/commands/secrets.rs tests/integration/secrets.rs
git commit -m "feat: add secret management commands"
```

### Task 4: Resolve credential IDs in the typed runtime

**Files:**
- Modify: `crates/ah-runtime/src/lib.rs`, `crates/ah-runtime/src/executor.rs`
- Modify: `src/runtime_flow.rs`, `src/host_commands.rs`
- Test: `crates/ah-runtime/src/lib.rs`, `tests/integration/mcp.rs`

**Interfaces:**
- Consumes: `SecretSlot`, public `arguments.credentials`, and `VaultStore::resolve`.
- Produces: `TypedInvocationRequest::resolved_secrets` for handlers, or a deterministic vault error.

- [ ] **Step 1: Write failing resolver tests**

```rust
#[test]
fn resolver_keeps_public_id_out_of_private_values_and_injects_secret() {
    let request = request_with(json!({"credentials":{"database":"qa-lms"}}));
    let resolved = resolve_credentials(&descriptor_with_database_slot(), request, &vault).unwrap();
    assert_eq!(resolved.arguments["credentials"]["database"], "qa-lms");
    assert_eq!(resolved.resolved_secrets["database"].kind, "postgres");
}

#[test]
fn resolver_rejects_unknown_slot_and_wrong_kind() {
    assert_code("SECRET_KIND_MISMATCH", request_with(json!({"credentials":{"database":"api-basic"}})));
}
```

- [ ] **Step 2: Run and verify failure**

Run: `ah run check cargo test -p ah-runtime resolver_rejects --locked`

Expected: failure because resolution is not part of runtime dispatch.

- [ ] **Step 3: Implement resolution after input validation**

Validate the schema first, parse the `credentials` map, require all mandatory slots, resolve each ID, and compare `record.kind` against `accepted_kinds`. Build a copied request using `with_resolved_secrets`; preserve only IDs in public arguments. Invoke this code in all three routes before builtin/host/dynamic handler dispatch.

- [ ] **Step 4: Wire the vault resolver into server and direct CLI runtime creation**

Construct one `Arc<VaultStore>` from `ConfigContext` during runtime bootstrap and pass an `Arc<dyn SecretResolver>` to `PluginManager`. Do not initialize or read the vault for commands with no slots.

- [ ] **Step 5: Add error and redaction tests**

Cover `SECRET_REQUIRED`, `SECRET_NOT_FOUND`, `SECRET_KIND_MISMATCH`, unavailable key provider, and a dynamic plugin round trip. Assert structured event data contains slot/id/kind only.

- [ ] **Step 6: Run focused checks**

Run: `ah run check cargo fmt --all -- --check`

Run: `ah run check cargo test -p ah-runtime --locked`

Run: `ah run check cargo test --test integration mcp --locked`

Expected: PASS.

- [ ] **Step 7: Commit**

```text
git add crates/ah-runtime/src/lib.rs crates/ah-runtime/src/executor.rs src/runtime_flow.rs src/host_commands.rs tests/integration/mcp.rs
git commit -m "feat: resolve vault credentials for typed commands"
```

### Task 5: Integrate PostgreSQL and HTTP Basic authentication

**Files:**
- Modify: `plugins/ah-plugin-postgres/src/lib.rs`, `plugins/ah-plugin-postgres/src/typed.rs`
- Modify: `src/commands/http.rs`, `src/commands/http/domain.rs`
- Test: `plugins/ah-plugin-postgres/src/{lib.rs,typed.rs}`, `src/commands/http.rs`, `src/commands/http/domain.rs`

**Interfaces:**
- Consumes: private `request.resolved_secrets["database"]` and `["basic"]`.
- Produces: internal `PGPASSWORD` or HTTP basic auth only at the executor boundary.

- [ ] **Step 1: Write failing plugin integration tests**

```rust
#[test]
fn postgres_database_slot_passes_password_only_to_psql_environment() {
    let invocation = typed_request_with_secret("database", postgres_secret("qa-lms", "pw"));
    let command = typed_cli(&invocation).unwrap();
    assert_eq!(command.connection.password_env, None);
    assert_eq!(command.connection.password, Some("pw".to_owned()));
}

#[test]
fn http_basic_slot_builds_auth_without_basic_argument() {
    let request = typed_request_with_secret("basic", basic_secret("api", "u", "p"));
    let config = request_config_from_typed(&request).unwrap();
    assert_eq!(config.auth, AuthConfig::Basic { username: "u".into(), password: "p".into() });
}
```

- [ ] **Step 2: Run and verify failure**

Run: `ah run check cargo test -p ah-plugin-postgres database_slot --locked`

Run: `ah run check cargo test http_basic_slot --locked`

Expected: failure because neither descriptor declares a slot.

- [ ] **Step 3: Add descriptor slots and CLI mappings**

Declare optional `database`/`postgres` slots for connection commands and optional `basic`/`http-basic` slots for `http.request`, `get`, `post`, and replay. Add repeatable `--credential SLOT=ID` to the relevant CLI parsers; map it to public typed `credentials`. Reject it for commands without declared slots. Retain `--password-env` and `--basic USER:PASS` for direct CLI compatibility.

- [ ] **Step 4: Use resolved values only at execution boundaries**

Add an internal non-serialized password field to PostgreSQL connection execution state and set `Command::env("PGPASSWORD", value)` only when it exists. In HTTP, build `AuthConfig::Basic` from the resolved record and retain existing conflict validation so `--basic` and slot `basic` cannot both be selected.

- [ ] **Step 5: Add negative and leakage tests**

Test missing/wrong slot, conflicting legacy and vault options, psql failure response, HTTP failure response, and event serialization. Assert sample `pw`/`p` are absent from stderr, JSON, and typed output.

- [ ] **Step 6: Run focused tests and commit**

Run: `ah run check cargo fmt --all -- --check`

Run: `ah run check cargo test -p ah-plugin-postgres --locked`

Run: `ah run check cargo test http --locked`

```text
git add plugins/ah-plugin-postgres/src/lib.rs plugins/ah-plugin-postgres/src/typed.rs src/commands/http.rs src/commands/http/domain.rs
git commit -m "feat: use vault credentials for postgres and http"
```

### Task 6: Add protected browser setup endpoint and MCP guidance

**Files:**
- Modify: `src/secrets/setup.rs`, `src/runtime_flow.rs`, `crates/ah-mcp/src/server.rs`
- Modify: `docs/reference/mcp.md`, `docs/agents/recipes/mcp-http.md`
- Create: `docs/reference/secrets.md`
- Test: `src/secrets/setup.rs`, `crates/ah-mcp/src/server.rs`, `tests/integration/mcp.rs`

**Interfaces:**
- Consumes: a setup capability minted by `ah secrets add|edit --open`.
- Produces: one-use browser form routes and MCP descriptions generated from `secret_slots`.

- [ ] **Step 1: Write failing setup-capability tests**

```rust
#[tokio::test]
async fn setup_capability_is_single_use_and_expires() {
    let token = capabilities.issue(SecretSetupTarget::Create { id: "qa-lms".into(), kind: Postgres }, Duration::from_secs(600));
    assert!(capabilities.consume(&token).is_ok());
    assert_code("VAULT_SETUP_CAPABILITY_INVALID", capabilities.consume(&token));
}

#[test]
fn generated_tool_description_names_slot_and_discovery_tool() {
    assert!(command_to_tool(&postgres_descriptor()).unwrap().description.unwrap().contains("secrets.list"));
}
```

- [ ] **Step 2: Run and verify failure**

Run: `ah run check cargo test setup_capability_is_single_use --locked`

Expected: failure because the setup capability store is absent.

- [ ] **Step 3: Implement capability store and routes**

Keep capabilities only in process memory in a mutex-protected map keyed by a random 256-bit token. Add GET form and POST submission routes to the existing HTTP router. Require the token for both methods, expire after ten minutes, consume on successful POST, and return only record metadata. Configure request tracing to redact the form body and capability query parameter.

- [ ] **Step 4: Add descriptor-derived agent instructions**

For descriptors with slots, append text equivalent to: `Credential slots: database accepts postgres. If the ID is unknown, call secrets.list with kind=postgres.` Keep examples ID-only. Do not put live vault record IDs in `tools/list`, because catalogs are cached and the list is a separate read-only tool.

- [ ] **Step 5: Document CLI, browser, MCP, and compatibility flows**

Document exact `secrets init`, CLI prompt, `--open`, `secrets.list`, Postgres `credentials.database`, HTTP `credentials.basic`, errors, and that direct `password_env` is not the HTTP-MCP solution. State that `/mcp` never returns or accepts a secret value.

- [ ] **Step 6: Run endpoint and guidance tests**

Run: `ah run check cargo test -p ah-mcp --locked`

Run: `ah run check cargo test --test integration mcp --locked`

Expected: PASS; expired/reused tokens fail and secret strings are absent from captured logs.

- [ ] **Step 7: Commit**

```text
git add src/secrets/setup.rs src/runtime_flow.rs crates/ah-mcp/src/server.rs docs/reference/mcp.md docs/agents/recipes/mcp-http.md docs/reference/secrets.md tests/integration/mcp.rs
git commit -m "feat: add secure vault setup and MCP guidance"
```

### Task 7: Run full regression validation and update agent manual

**Files:**
- Modify: `docs/agents/recipes/mcp-http.md`, `docs/reference/secrets.md`
- Test: `tests/integration/mcp.rs`, `tests/integration/secrets.rs`

**Interfaces:**
- Consumes: all previous tasks.
- Produces: documented, release-ready vault behaviour.

- [ ] **Step 1: Add end-to-end tests**

```rust
#[test]
fn mcp_agent_discovers_postgres_credential_then_queries_without_cwd() {
    let listed = call_tool("secrets_list", json!({"kind":"postgres"}));
    assert_eq!(listed["secrets"][0]["id"], "qa-lms");
    let query = call_tool("postgres_query", json!({
        "credentials":{"database":"qa-lms"},
        "sql":"select 1"
    }));
    assert_success(query);
}
```

- [ ] **Step 2: Run the end-to-end tests**

Run: `ah run check cargo test --test integration mcp --locked`

Run: `ah run check cargo test --test integration secrets --locked`

Expected: PASS.

- [ ] **Step 3: Run required workspace checks**

Run: `ah run check cargo fmt --all -- --check`

Run: `ah run check cargo test --workspace --all-targets --locked`

Run: `ah run check cargo build --locked`

Expected: all PASS.

- [ ] **Step 4: Review for secret exposure and commit documentation corrections**

Run: `rg -n -i "password|authorization|credential" docs src crates plugins tests`

Inspect all new output/assertions to ensure examples use IDs only. Stage only intended documentation/test corrections, then commit:

```text
git add docs tests
git commit -m "test: verify vault credential workflows"
```

## Self-review

Spec coverage:

- Encrypted, cross-platform storage and its explicit headless fallback are covered by Task 2.
- CLI, browser setup, and safe MCP discovery are covered by Tasks 3 and 6.
- Generic slot mapping and private plugin delivery are covered by Tasks 1 and 4.
- PostgreSQL and HTTP Basic consumers are covered by Task 5; SSH is represented by its kind and slot contract, without inventing an SSH plugin.
- Deterministic errors, log redaction, backward compatibility, and full validation are covered across Tasks 2, 4, 5, 6, and 7.

Placeholder scan: no deferred implementation markers or unspecified validation steps remain. Type consistency: all tasks use `SecretSlot`, `ResolvedSecret`, `VaultStore`, `credentials`, and `resolved_secrets` consistently.
