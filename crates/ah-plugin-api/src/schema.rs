//! Derive command schemas from the Rust types that produce and consume them.
//!
//! Hand-writing a JSON Schema next to the struct it describes means the two can
//! disagree, and today the only thing that notices is response validation at
//! run time. Deriving the schema removes that class of bug entirely.
//!
//! `schemars` output is not directly usable as an AIHelper command schema, so
//! everything here goes through [`normalize_output`] or [`normalize_input`],
//! which reshape it into the form the command catalog has always published:
//!
//! | `schemars` emits | AIHelper publishes | Why |
//! |---|---|---|
//! | `$defs` + `$ref` | fully inlined subschemas | MCP clients consume these directly; a self-contained schema needs no resolver |
//! | `$schema`, `title` | removed | the runtime strips `$schema` anyway, and the title is a Rust type name |
//! | `format: "uint"` | removed | `minimum: 0` already carries the constraint |
//! | `anyOf: [T, null]` | `oneOf: [T, null]` | the two are equivalent here, and `oneOf` is what the catalog has always used |
//! | `Option<T>` omitted from `required` | listed in `required` | output structs always serialize every field, including `null` ones |
//!
//! The last rule applies to **outputs only**. Input schemas keep the `required`
//! list `schemars` derives, because an optional argument really is optional.

use schemars::JsonSchema;
use serde_json::{Map, Value};

/// Maximum number of inlining passes before a `$ref` graph is treated as cyclic.
const MAX_INLINE_PASSES: usize = 16;

/// Schema for a command's output payload, with `command` pinned to `command_id`.
///
/// Every output type carries a `command` discriminator whose value is the command
/// id; deriving cannot know that value, so it is applied here.
pub fn output_schema_for<T: JsonSchema>(command_id: &str) -> Value {
    let mut schema = normalize_output(raw_schema::<T>());
    pin_command_discriminator(&mut schema, command_id);
    schema
}

/// Schema for a command's argument type.
pub fn input_schema_for<T: JsonSchema>() -> Value {
    normalize_input(raw_schema::<T>())
}

/// An input schema for a command that takes no arguments.
pub fn empty_input_schema() -> Value {
    serde_json::json!({
        "type": "object",
        "properties": {},
        "additionalProperties": false
    })
}

fn raw_schema<T: JsonSchema>() -> Value {
    serde_json::to_value(schemars::schema_for!(T))
        .expect("a generated JSON Schema is always representable as a value")
}

/// Reshape a derived schema into catalog form, requiring every property.
pub fn normalize_output(schema: Value) -> Value {
    let mut schema = inline_definitions(schema);
    strip_annotations(&mut schema);
    prefer_one_of_for_nullable(&mut schema);
    require_all_properties(&mut schema);
    schema
}

/// Reshape a derived schema into catalog form, keeping the derived `required` list.
pub fn normalize_input(schema: Value) -> Value {
    let mut schema = inline_definitions(schema);
    strip_annotations(&mut schema);
    prefer_one_of_for_nullable(&mut schema);
    drop_null_from_optional_arguments(&mut schema);
    ensure_object_properties(&mut schema);
    schema
}

/// An omitted argument and an explicit `null` are different things on the wire.
///
/// `Option<T>` means "may be omitted" for a command argument, and the catalog has
/// always rejected an explicit `null` for one. `schemars` cannot tell the two
/// apart, so the null branch is removed from every property that is already
/// optional by virtue of not being required.
fn drop_null_from_optional_arguments(schema: &mut Value) {
    let Some(root) = schema.as_object_mut() else {
        return;
    };
    let required = root
        .get("required")
        .and_then(Value::as_array)
        .map(|names| {
            names
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let Some(properties) = root.get_mut("properties").and_then(Value::as_object_mut) else {
        return;
    };
    for (name, property) in properties.iter_mut() {
        if required.iter().any(|required_name| required_name == name) {
            continue;
        }
        drop_null_branch(property);
    }
}

fn drop_null_branch(property: &mut Value) {
    let Some(map) = property.as_object_mut() else {
        return;
    };
    if let Some(kinds) = map.get("type").and_then(Value::as_array) {
        let remaining = kinds
            .iter()
            .filter(|kind| kind.as_str() != Some("null"))
            .cloned()
            .collect::<Vec<_>>();
        if remaining.len() == 1 && remaining.len() < kinds.len() {
            let only = remaining.into_iter().next().expect("checked above");
            map.insert("type".to_owned(), only);
        }
        return;
    }
    let Some(variants) = map.get("oneOf").and_then(Value::as_array) else {
        return;
    };
    let remaining = variants
        .iter()
        .filter(|variant| !is_null_schema(variant))
        .cloned()
        .collect::<Vec<_>>();
    if remaining.len() != 1 || remaining.len() == variants.len() {
        return;
    }
    map.remove("oneOf");
    let Some(Value::Object(only)) = remaining.into_iter().next() else {
        return;
    };
    for (key, value) in only {
        map.entry(key).or_insert(value);
    }
}

fn pin_command_discriminator(schema: &mut Value, command_id: &str) {
    let Some(command) = schema
        .get_mut("properties")
        .and_then(Value::as_object_mut)
        .and_then(|properties| properties.get_mut("command"))
        .and_then(Value::as_object_mut)
    else {
        return;
    };
    command.insert("const".to_owned(), Value::String(command_id.to_owned()));
}

/// Replace every `$ref` with the subschema it points at and drop `$defs`.
///
/// Runs to a fixpoint so nested definitions resolve too. A `$ref` cycle would
/// never converge, so after [`MAX_INLINE_PASSES`] the definitions are left in
/// place rather than looping; no current type is cyclic, and a type that became
/// cyclic would show up as a `$defs` block in the catalog snapshot.
fn inline_definitions(mut schema: Value) -> Value {
    let Some(mut definitions) = schema
        .as_object_mut()
        .and_then(|root| root.remove("$defs"))
        .and_then(|defs| match defs {
            Value::Object(map) => Some(map),
            _ => None,
        })
    else {
        return schema;
    };

    for _ in 0..MAX_INLINE_PASSES {
        if !definitions.values().any(contains_ref) {
            break;
        }
        let snapshot = definitions.clone();
        for value in definitions.values_mut() {
            substitute_refs(value, &snapshot);
        }
    }

    substitute_refs(&mut schema, &definitions);
    if contains_ref(&schema)
        && let Some(root) = schema.as_object_mut()
    {
        root.insert("$defs".to_owned(), Value::Object(definitions));
    }
    schema
}

fn contains_ref(value: &Value) -> bool {
    match value {
        Value::Object(map) => map.contains_key("$ref") || map.values().any(contains_ref),
        Value::Array(values) => values.iter().any(contains_ref),
        _ => false,
    }
}

fn substitute_refs(value: &mut Value, definitions: &Map<String, Value>) {
    match value {
        Value::Object(map) => {
            if let Some(target) = map
                .get("$ref")
                .and_then(Value::as_str)
                .and_then(|reference| reference.strip_prefix("#/$defs/"))
                .and_then(|name| definitions.get(name))
            {
                *value = target.clone();
                return;
            }
            for nested in map.values_mut() {
                substitute_refs(nested, definitions);
            }
        }
        Value::Array(values) => {
            for nested in values {
                substitute_refs(nested, definitions);
            }
        }
        _ => {}
    }
}

/// Remove keys that describe the Rust type rather than the JSON contract.
fn strip_annotations(value: &mut Value) {
    match value {
        Value::Object(map) => {
            map.remove("$schema");
            map.remove("title");
            map.remove("format");
            for nested in map.values_mut() {
                strip_annotations(nested);
            }
        }
        Value::Array(values) => {
            for nested in values {
                strip_annotations(nested);
            }
        }
        _ => {}
    }
}

/// Rewrite the nullable-wrapper `anyOf` that `schemars` emits for `Option<T>`.
fn prefer_one_of_for_nullable(value: &mut Value) {
    match value {
        Value::Object(map) => {
            if let Some(variants) = map.get("anyOf").and_then(Value::as_array)
                && variants.len() == 2
                && variants.iter().any(is_null_schema)
            {
                let variants = map.remove("anyOf").expect("checked above");
                map.insert("oneOf".to_owned(), variants);
            }
            for nested in map.values_mut() {
                prefer_one_of_for_nullable(nested);
            }
        }
        Value::Array(values) => {
            for nested in values {
                prefer_one_of_for_nullable(nested);
            }
        }
        _ => {}
    }
}

fn is_null_schema(value: &Value) -> bool {
    value
        .get("type")
        .and_then(Value::as_str)
        .is_some_and(|kind| kind == "null")
}

/// Output payloads serialize every field, so every property is required.
fn require_all_properties(value: &mut Value) {
    match value {
        Value::Object(map) => {
            let names = map
                .get("properties")
                .and_then(Value::as_object)
                .map(|properties| properties.keys().cloned().collect::<Vec<_>>());
            if let Some(names) = names
                && !names.is_empty()
            {
                map.insert(
                    "required".to_owned(),
                    Value::Array(names.into_iter().map(Value::String).collect()),
                );
                map.entry("additionalProperties")
                    .or_insert(Value::Bool(false));
            }
            for nested in map.values_mut() {
                require_all_properties(nested);
            }
        }
        Value::Array(values) => {
            for nested in values {
                require_all_properties(nested);
            }
        }
        _ => {}
    }
}

/// An argument object with no declared properties still needs the empty map, so
/// the published schema shape does not depend on whether a command takes
/// arguments.
fn ensure_object_properties(value: &mut Value) {
    let Some(map) = value.as_object_mut() else {
        return;
    };
    if map.get("type").and_then(Value::as_str) == Some("object") && !map.contains_key("properties")
    {
        map.insert("properties".to_owned(), Value::Object(Map::new()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[derive(JsonSchema)]
    #[serde(deny_unknown_fields)]
    #[allow(dead_code)]
    struct CommitSummary {
        hash: String,
        short_hash: String,
        subject: String,
    }

    #[derive(JsonSchema)]
    #[serde(deny_unknown_fields)]
    #[allow(dead_code)]
    struct StatusOutput {
        command: String,
        in_git_repo: bool,
        branch: Option<String>,
        ahead: Option<usize>,
        clean: bool,
        staged_count: usize,
        latest_commit: Option<CommitSummary>,
    }

    #[test]
    fn output_schema_matches_the_hand_written_catalog_form() {
        let derived = output_schema_for::<StatusOutput>("git.status");

        assert_eq!(
            derived,
            json!({
                "type": "object",
                "properties": {
                    "command": {"type": "string", "const": "git.status"},
                    "in_git_repo": {"type": "boolean"},
                    "branch": {"type": ["string", "null"]},
                    "ahead": {"type": ["integer", "null"], "minimum": 0},
                    "clean": {"type": "boolean"},
                    "staged_count": {"type": "integer", "minimum": 0},
                    "latest_commit": {
                        "oneOf": [
                            {
                                "type": "object",
                                "properties": {
                                    "hash": {"type": "string"},
                                    "short_hash": {"type": "string"},
                                    "subject": {"type": "string"}
                                },
                                "required": ["hash", "short_hash", "subject"],
                                "additionalProperties": false
                            },
                            {"type": "null"}
                        ]
                    }
                },
                "required": [
                    "ahead",
                    "branch",
                    "clean",
                    "command",
                    "in_git_repo",
                    "latest_commit",
                    "staged_count"
                ],
                "additionalProperties": false
            })
        );
    }

    #[derive(JsonSchema)]
    #[serde(deny_unknown_fields)]
    #[allow(dead_code)]
    struct TagsArgs {
        latest: bool,
        name: Option<String>,
    }

    #[test]
    fn input_schema_keeps_optional_arguments_optional() {
        let derived = input_schema_for::<TagsArgs>();

        assert_eq!(derived["required"], json!(["latest"]));
        assert_eq!(derived["additionalProperties"], json!(false));
        assert_eq!(
            derived["properties"]["name"],
            json!({"type": "string"}),
            "an optional argument may be omitted, but not sent as an explicit null"
        );
    }

    #[test]
    fn nested_definitions_are_inlined() {
        let derived = output_schema_for::<StatusOutput>("git.status");
        assert!(
            !contains_ref(&derived),
            "no $ref should survive normalization: {derived}"
        );
        assert!(derived.get("$defs").is_none());
    }
}
