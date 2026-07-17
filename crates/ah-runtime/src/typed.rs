use std::collections::HashSet;

use ah_plugin_api::{CommandCatalog, CommandDescriptor, PluginMetadata, TypedInvocationResponse};

use crate::RuntimeError;

const MCP_TOOL_PREFIX_LEN: usize = 3;
const MCP_TOOL_NAME_MAX_LEN: usize = 128;
const ALLOWED_INPUT_ROOT_KEYWORDS: &[&str] = &[
    "$schema",
    "$id",
    "$defs",
    "definitions",
    "title",
    "description",
    "default",
    "examples",
    "deprecated",
    "readOnly",
    "writeOnly",
    "type",
    "properties",
    "required",
    "additionalProperties",
];

pub(crate) struct CompiledCommandValidators {
    pub(crate) command_id: String,
    pub(crate) input: jsonschema::Validator,
    pub(crate) output: jsonschema::Validator,
}

pub fn validate_catalog(
    metadata: &PluginMetadata,
    catalog: &CommandCatalog,
) -> Result<(), RuntimeError> {
    compile_catalog(metadata, catalog).map(|_| ())
}

pub(crate) fn compile_catalog(
    metadata: &PluginMetadata,
    catalog: &CommandCatalog,
) -> Result<Vec<CompiledCommandValidators>, RuntimeError> {
    if catalog.plugin_name != metadata.plugin_name {
        return invalid_catalog(
            &metadata.domain,
            format!(
                "catalog plugin_name '{}' does not match metadata plugin_name '{}'",
                catalog.plugin_name, metadata.plugin_name
            ),
        );
    }
    if !catalog.domain.eq_ignore_ascii_case(&metadata.domain) {
        return invalid_catalog(
            &metadata.domain,
            format!(
                "catalog domain '{}' does not match metadata domain '{}'",
                catalog.domain, metadata.domain
            ),
        );
    }

    let normalized_domain = metadata.domain.trim().to_ascii_lowercase();
    let mut command_ids = HashSet::new();
    let mut validators = Vec::with_capacity(catalog.commands.len());
    for descriptor in &catalog.commands {
        let compiled = validate_descriptor(&normalized_domain, descriptor)?;
        if !command_ids.insert(descriptor.id.clone()) {
            return invalid_catalog(
                &metadata.domain,
                format!("duplicate command id '{}'", descriptor.id),
            );
        }
        validators.push(compiled);
    }
    Ok(validators)
}

pub fn validate_arguments(
    descriptor: &CommandDescriptor,
    arguments: &serde_json::Value,
) -> Result<(), RuntimeError> {
    let validator = jsonschema::validator_for(&descriptor.input_schema).map_err(|error| {
        RuntimeError::TypedInvocation(format!(
            "input schema for '{}' is invalid: {error}",
            descriptor.id
        ))
    })?;
    validate_arguments_with(&descriptor.id, &validator, arguments)
}

pub(crate) fn validate_arguments_with(
    command: &str,
    validator: &jsonschema::Validator,
    arguments: &serde_json::Value,
) -> Result<(), RuntimeError> {
    if !arguments.is_object() {
        return Err(RuntimeError::TypedInvocation(format!(
            "arguments for '{}' must be a JSON object",
            command
        )));
    }
    validator.validate(arguments).map_err(|error| {
        RuntimeError::TypedInvocation(format!(
            "arguments for '{}' failed validation: {error}",
            command
        ))
    })
}

pub fn validate_response(
    descriptor: &CommandDescriptor,
    response: &TypedInvocationResponse,
) -> Result<(), RuntimeError> {
    let validator = jsonschema::validator_for(&descriptor.output_schema).map_err(|error| {
        RuntimeError::TypedResponseValidation {
            command: descriptor.id.clone(),
            reason: format!("output schema is invalid: {error}"),
        }
    })?;
    validate_response_with(&descriptor.id, &validator, response)
}

pub(crate) fn validate_response_with(
    command: &str,
    validator: &jsonschema::Validator,
    response: &TypedInvocationResponse,
) -> Result<(), RuntimeError> {
    if response.success {
        if response.error.is_some() {
            return invalid_response(command, "successful response must not contain an error");
        }
        let data = response
            .data
            .as_ref()
            .ok_or_else(|| RuntimeError::TypedResponseValidation {
                command: command.to_owned(),
                reason: "successful response must contain data".to_owned(),
            })?;
        if !data.is_object() {
            return invalid_response(command, "successful response data must be a JSON object");
        }
        validator
            .validate(data)
            .map_err(|error| RuntimeError::TypedResponseValidation {
                command: command.to_owned(),
                reason: error.to_string(),
            })
    } else {
        if response.data.is_some() {
            return invalid_response(command, "failed response must not contain success data");
        }
        if response.error.is_none() {
            return invalid_response(command, "failed response must contain an error");
        }
        Ok(())
    }
}

pub fn mcp_input_schema(
    descriptor: &CommandDescriptor,
) -> Result<serde_json::Value, RuntimeError> {
    let mut schema = descriptor.input_schema.clone();
    let schema_object = schema
        .as_object_mut()
        .ok_or_else(|| RuntimeError::InvalidCommandCatalog {
            domain: command_domain(&descriptor.id),
            reason: format!("command '{}' input schema must be an object", descriptor.id),
        })?;
    let properties = schema_object
        .entry("properties")
        .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()))
        .as_object_mut()
        .ok_or_else(|| RuntimeError::InvalidCommandCatalog {
            domain: command_domain(&descriptor.id),
            reason: format!(
                "command '{}' input schema properties must be an object",
                descriptor.id
            ),
        })?;
    properties.insert(
        "context".to_owned(),
        serde_json::json!({
            "type": "object",
            "description": "Optional execution context for this call.",
            "properties": {
                "cwd": {
                    "type": "string",
                    "minLength": 1,
                    "description": "Base directory for relative paths and child processes."
                },
                "limit": {
                    "type": "integer",
                    "minimum": 1,
                    "description": "Optional output item or line limit."
                },
                "timeout_ms": {
                    "type": "integer",
                    "minimum": 1,
                    "description": "Total queue and execution timeout in milliseconds."
                }
            },
            "additionalProperties": false
        }),
    );
    Ok(schema)
}

fn command_domain(command: &str) -> String {
    command
        .split_once('.')
        .map(|(domain, _)| domain.to_owned())
        .unwrap_or_else(|| command.to_owned())
}

fn validate_descriptor(
    normalized_domain: &str,
    descriptor: &CommandDescriptor,
) -> Result<CompiledCommandValidators, RuntimeError> {
    if descriptor.id.len() + MCP_TOOL_PREFIX_LEN > MCP_TOOL_NAME_MAX_LEN {
        return invalid_catalog(
            normalized_domain,
            format!(
                "command id '{}' is too long for MCP exposure",
                descriptor.id
            ),
        );
    }
    if descriptor.id.is_empty()
        || !descriptor
            .id
            .bytes()
            .all(|value| value.is_ascii_alphanumeric() || matches!(value, b'_' | b'-' | b'.'))
    {
        return invalid_catalog(
            normalized_domain,
            format!(
                "command id '{}' contains unsupported characters",
                descriptor.id
            ),
        );
    }
    if descriptor.id.split('.').any(str::is_empty) {
        return invalid_catalog(
            normalized_domain,
            format!("command id '{}' contains an empty segment", descriptor.id),
        );
    }
    let expected_prefix = format!("{normalized_domain}.");
    if !descriptor.id.starts_with(&expected_prefix) {
        return invalid_catalog(
            normalized_domain,
            format!(
                "command id '{}' must start with '{}'",
                descriptor.id, expected_prefix
            ),
        );
    }
    if descriptor.title.trim().is_empty() {
        return invalid_catalog(
            normalized_domain,
            format!("command '{}' has an empty title", descriptor.id),
        );
    }
    if descriptor.description.trim().is_empty() {
        return invalid_catalog(
            normalized_domain,
            format!("command '{}' has an empty description", descriptor.id),
        );
    }
    if descriptor.effects.impact.trim().is_empty() {
        return invalid_catalog(
            normalized_domain,
            format!(
                "command '{}' has an empty impact description",
                descriptor.id
            ),
        );
    }
    if descriptor.effects.effects.is_empty() {
        return invalid_catalog(
            normalized_domain,
            format!("command '{}' has no effect categories", descriptor.id),
        );
    }
    if descriptor.effects.read_only && descriptor.effects.destructive {
        return invalid_catalog(
            normalized_domain,
            format!(
                "command '{}' cannot be both read-only and destructive",
                descriptor.id
            ),
        );
    }

    validate_input_root_keywords(normalized_domain, descriptor)?;
    let input = validate_schema(
        normalized_domain,
        descriptor,
        "input",
        &descriptor.input_schema,
    )?;
    let output = validate_schema(
        normalized_domain,
        descriptor,
        "output",
        &descriptor.output_schema,
    )?;

    let properties = descriptor
        .input_schema
        .get("properties")
        .and_then(serde_json::Value::as_object);
    if properties.is_some_and(|value| value.contains_key("context")) {
        return invalid_catalog(
            normalized_domain,
            format!(
                "command '{}' uses reserved input property 'context'",
                descriptor.id
            ),
        );
    }
    if descriptor
        .input_schema
        .get("required")
        .and_then(serde_json::Value::as_array)
        .is_some_and(|required| required.iter().any(|item| item.as_str() == Some("context")))
    {
        return invalid_catalog(
            normalized_domain,
            format!(
                "command '{}' uses reserved required input property 'context'",
                descriptor.id
            ),
        );
    }

    for example in &descriptor.examples {
        if !example.arguments.is_object() {
            return invalid_catalog(
                normalized_domain,
                format!(
                    "command '{}' example '{}' arguments must be an object",
                    descriptor.id, example.description
                ),
            );
        }
        if let Err(error) = input.validate(&example.arguments) {
            return invalid_catalog(
                normalized_domain,
                format!(
                    "command '{}' example '{}' failed validation: {error}",
                    descriptor.id, example.description
                ),
            );
        }
    }

    let augmented = mcp_input_schema(descriptor)?;
    jsonschema::validator_for(&augmented).map_err(|error| RuntimeError::InvalidCommandCatalog {
        domain: normalized_domain.to_owned(),
        reason: format!(
            "command '{}' MCP input schema is invalid after context injection: {error}",
            descriptor.id
        ),
    })?;

    Ok(CompiledCommandValidators {
        command_id: descriptor.id.clone(),
        input,
        output,
    })
}

fn validate_input_root_keywords(
    domain: &str,
    descriptor: &CommandDescriptor,
) -> Result<(), RuntimeError> {
    let Some(schema) = descriptor.input_schema.as_object() else {
        return invalid_catalog(
            domain,
            format!("command '{}' input schema must be a JSON object", descriptor.id),
        );
    };
    for keyword in schema.keys() {
        if !ALLOWED_INPUT_ROOT_KEYWORDS.contains(&keyword.as_str()) {
            return invalid_catalog(
                domain,
                format!(
                    "command '{}' input schema root keyword '{}' is incompatible with MCP context injection",
                    descriptor.id, keyword
                ),
            );
        }
    }
    Ok(())
}

fn validate_schema(
    domain: &str,
    descriptor: &CommandDescriptor,
    kind: &str,
    schema: &serde_json::Value,
) -> Result<jsonschema::Validator, RuntimeError> {
    let schema_object = schema
        .as_object()
        .ok_or_else(|| RuntimeError::InvalidCommandCatalog {
            domain: domain.to_owned(),
            reason: format!(
                "command '{}' {kind} schema must be a JSON object",
                descriptor.id
            ),
        })?;
    if schema_object
        .get("type")
        .and_then(serde_json::Value::as_str)
        != Some("object")
    {
        return invalid_catalog(
            domain,
            format!(
                "command '{}' {kind} schema root type must be 'object'",
                descriptor.id
            ),
        );
    }
    jsonschema::validator_for(schema).map_err(|error| RuntimeError::InvalidCommandCatalog {
        domain: domain.to_owned(),
        reason: format!(
            "command '{}' {kind} schema is invalid: {error}",
            descriptor.id
        ),
    })
}

fn invalid_catalog<T>(domain: &str, reason: impl Into<String>) -> Result<T, RuntimeError> {
    Err(RuntimeError::InvalidCommandCatalog {
        domain: domain.to_owned(),
        reason: reason.into(),
    })
}

fn invalid_response<T>(command: &str, reason: impl Into<String>) -> Result<T, RuntimeError> {
    Err(RuntimeError::TypedResponseValidation {
        command: command.to_owned(),
        reason: reason.into(),
    })
}

#[cfg(test)]
mod tests {
    use ah_plugin_api::{
        AH_PLUGIN_ABI_VERSION, CommandEffect, CommandEffects, CommandError, CommandExample,
        PluginCompatibility, Reversibility, RiskLevel, TypedInvocationResponse,
    };
    use serde_json::json;

    use super::*;

    fn metadata() -> PluginMetadata {
        PluginMetadata {
            plugin_name: "test-plugin".to_owned(),
            domain: "test".to_owned(),
            description: "test plugin".to_owned(),
            abi_version: AH_PLUGIN_ABI_VERSION,
            required_tools: Vec::new(),
            compatibility: PluginCompatibility::current(),
        }
    }

    fn descriptor() -> CommandDescriptor {
        CommandDescriptor::new(
            "test.inspect",
            "Inspect test data",
            "Inspect test data. Impact: reads local test data.",
            json!({
                "$schema": "https://json-schema.org/draft/2020-12/schema",
                "type": "object",
                "properties": {
                    "value": { "type": "string" }
                },
                "required": ["value"],
                "additionalProperties": false
            }),
            json!({
                "$schema": "https://json-schema.org/draft/2020-12/schema",
                "type": "object",
                "properties": {
                    "value": { "type": "string" }
                },
                "required": ["value"],
                "additionalProperties": false
            }),
            CommandEffects::new(
                true,
                false,
                true,
                false,
                vec![CommandEffect::FilesystemRead],
                RiskLevel::Low,
                "Reads local test data.",
                Reversibility::Yes,
            ),
        )
        .with_example(CommandExample::new(
            "Inspect one value",
            json!({ "value": "demo" }),
        ))
    }

    #[test]
    fn valid_catalog_passes() {
        let catalog = CommandCatalog::new("test-plugin", "test", vec![descriptor()]);
        validate_catalog(&metadata(), &catalog).expect("catalog should be valid");
    }

    #[test]
    fn catalog_rejects_reserved_context_property() {
        let mut descriptor = descriptor();
        descriptor.input_schema["properties"]["context"] = json!({ "type": "object" });
        let catalog = CommandCatalog::new("test-plugin", "test", vec![descriptor]);

        let error = validate_catalog(&metadata(), &catalog)
            .expect_err("reserved context property should fail");
        assert!(error.to_string().contains("reserved input property"));
    }

    #[test]
    fn catalog_rejects_incompatible_input_root_keywords() {
        let mut descriptor = descriptor();
        descriptor.input_schema["allOf"] = json!([{"required": ["value"]}]);
        let catalog = CommandCatalog::new("test-plugin", "test", vec![descriptor]);

        let error = validate_catalog(&metadata(), &catalog)
            .expect_err("root composition should be rejected");
        assert!(error.to_string().contains("incompatible with MCP context injection"));
    }

    #[test]
    fn closed_input_schema_accepts_injected_optional_context() {
        let descriptor = descriptor();

        let schema = mcp_input_schema(&descriptor).expect("schema should be augmented");
        let validator = jsonschema::validator_for(&schema).expect("schema should compile");

        validator
            .validate(&json!({
                "value": "ok",
                "context": {"cwd": ".", "limit": 10, "timeout_ms": 1_000}
            }))
            .expect("injected context should satisfy a closed root schema");
    }

    #[test]
    fn arguments_are_validated_against_input_schema() {
        let descriptor = descriptor();
        validate_arguments(&descriptor, &json!({ "value": "ok" }))
            .expect("valid input should pass");

        let error = validate_arguments(&descriptor, &json!({ "unknown": true }))
            .expect_err("invalid input should fail");
        assert!(error.to_string().contains("failed validation"));
    }

    #[test]
    fn success_response_is_validated_against_output_schema() {
        let descriptor = descriptor();
        let response = TypedInvocationResponse::success(json!({ "value": "ok" }), None);
        validate_response(&descriptor, &response).expect("valid response should pass");

        let invalid = TypedInvocationResponse::success(json!({ "value": 7 }), None);
        let error =
            validate_response(&descriptor, &invalid).expect_err("invalid response should fail");
        assert!(error.to_string().contains("failed validation"));
    }

    #[test]
    fn error_response_requires_error_and_no_data() {
        let descriptor = descriptor();
        let response = TypedInvocationResponse::error(CommandError::new(
            Some("test".to_owned()),
            Some("test.inspect".to_owned()),
            "FAILED",
            "failed",
            "test failure",
            1,
            false,
        ));
        validate_response(&descriptor, &response).expect("error response should pass");

        let invalid = TypedInvocationResponse {
            success: false,
            data: Some(json!({})),
            text: None,
            notices: Vec::new(),
            error: None,
        };
        let error =
            validate_response(&descriptor, &invalid).expect_err("invalid error shape should fail");
        assert!(error.to_string().contains("must not contain success data"));
    }
}
