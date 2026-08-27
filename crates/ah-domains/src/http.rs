//! `ah http`: the command surface, the typed dispatch and the credential
//! binding, over `domain`.
//!
//! 846 lines, of which twenty argument structs and eight catalog descriptors
//! were declaration. Those move to `args` and `catalog`, both registered in
//! `commands/layout.rs` as deliberate extras rather than a new layer.

use std::{
    collections::BTreeMap,
    fmt,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use ah_plugin_api::{
    CommandCatalog, CommandDescriptor, CommandEffect, CommandEffects, CommandError,
    InvocationResponse, ResolvedSecret, Reversibility, RiskLevel, SecretSlot,
    TypedInvocationRequest, TypedInvocationResponse,
    schema::{input_schema_for, output_schema_for},
};
use clap::{Args, Subcommand, ValueEnum};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;

use ah_error::AppError;
use ah_output::{Emitter, GlobalOptions, OutputSink};

mod args;
mod catalog;
pub(crate) use args::BasicCredential;
pub use args::{
    AssertArgs, AssertReportArg, AssertWireArgs, HttpArgs, HttpCommand, MethodShortcutArgs,
    MethodShortcutWireArgs, ReplayArgs, ReplayWireArgs, RequestArgs, RequestExpectArgs,
    RequestOptionsArgs, RequestOptionsWireArgs, RequestWireArgs,
};
pub use catalog::command_catalog;

pub mod io;

mod output;

mod domain;

pub fn execute(
    mut args: HttpArgs,
    options: &GlobalOptions,
    sink: &OutputSink,
) -> Result<(), AppError> {
    if let Some(cwd) = options.cwd.as_deref() {
        rebase(&mut args.command, cwd);
    }
    match args.command {
        HttpCommand::Request(request_args) => execute_request(
            domain::run_request_command(request_args, "request"),
            options,
            sink,
        ),
        HttpCommand::Get(method_args) => execute_shortcut("get", "GET", method_args, options, sink),
        HttpCommand::Post(method_args) => {
            execute_shortcut("post", "POST", method_args, options, sink)
        }
        HttpCommand::Put(method_args) => execute_shortcut("put", "PUT", method_args, options, sink),
        HttpCommand::Patch(method_args) => {
            execute_shortcut("patch", "PATCH", method_args, options, sink)
        }
        HttpCommand::Delete(method_args) => {
            execute_shortcut("delete", "DELETE", method_args, options, sink)
        }
        HttpCommand::Replay(replay_args) => {
            execute_request(domain::run_replay(replay_args, "replay"), options, sink)
        }
        HttpCommand::Assert(assert_args) => execute_assert(assert_args, options, sink, "assert"),
        HttpCommand::Run(assert_args) => execute_assert(assert_args, options, sink, "run"),
    }
}

/// Resolve every file argument against the directory the request named.
///
/// Shared by both entry points: the CLI used to get this by the process having
/// been `chdir`-ed, which is the same answer only as long as one request is in
/// flight at a time.
fn rebase(command: &mut HttpCommand, cwd: &Path) {
    let body = |request: &mut RequestOptionsArgs| {
        request.json_file = request
            .json_file
            .as_deref()
            .map(|path| rebase_path(cwd, path));
        request.body_file = request
            .body_file
            .as_deref()
            .map(|path| rebase_path(cwd, path));
    };
    match command {
        HttpCommand::Request(args) => body(&mut args.request),
        HttpCommand::Get(args)
        | HttpCommand::Post(args)
        | HttpCommand::Put(args)
        | HttpCommand::Patch(args)
        | HttpCommand::Delete(args) => body(&mut args.request),
        HttpCommand::Replay(args) => body(&mut args.request),
        HttpCommand::Assert(args) | HttpCommand::Run(args) => {
            args.spec_path = rebase_path(cwd, &args.spec_path);
        }
    }
}

pub fn invoke_typed(request: &TypedInvocationRequest) -> TypedInvocationResponse {
    let result = match request.command.as_str() {
        "http.request" => typed_request(request, "request", None),
        "http.get" => typed_request(request, "get", Some("GET")),
        "http.post" => typed_request(request, "post", Some("POST")),
        "http.put" => typed_request(request, "put", Some("PUT")),
        "http.patch" => typed_request(request, "patch", Some("PATCH")),
        "http.delete" => typed_request(request, "delete", Some("DELETE")),
        "http.replay" => typed_replay(request),
        "http.assert" => typed_assert(request, "assert"),
        "http.run" => typed_assert(request, "run"),
        _ => Err(AppError::invalid_argument(format!(
            "unknown typed HTTP command: {}",
            request.command
        ))),
    };
    match result {
        Ok(data) => {
            TypedInvocationResponse::success(data, Some(format!("Completed {}.", request.command)))
        }
        Err(error) => TypedInvocationResponse::error(CommandError::from_diagnostic(
            error
                .diagnostic()
                .with_domain("http")
                .with_operation(request.command.clone()),
            retryable_http_error(error.code()),
        )),
    }
}

fn typed_request(
    request: &TypedInvocationRequest,
    command_name: &'static str,
    method: Option<&str>,
) -> Result<Value, AppError> {
    let args = match method {
        Some(method) => {
            let wire: MethodShortcutWireArgs = decode(request)?;
            let (options, expect) = wire.options.split(request)?;
            RequestArgs {
                method: method.to_owned(),
                url: wire.url,
                request: options,
                expect,
            }
        }
        None => {
            let wire: RequestWireArgs = decode(request)?;
            let (options, expect) = wire.options.split(request)?;
            RequestArgs {
                method: wire.method,
                url: wire.url,
                request: options,
                expect,
            }
        }
    };
    let output = domain::run_request_command(args, command_name)?;
    if !output.ok {
        return Err(AppError::external(
            "HTTP_ASSERTION_FAILED",
            format!(
                "{} expectation(s) failed: {}",
                output.assertions.failed,
                output.assertions.failures.join("; ")
            ),
        ));
    }
    Ok(serde_json::to_value(output)?)
}

fn typed_replay(request: &TypedInvocationRequest) -> Result<Value, AppError> {
    let wire: ReplayWireArgs = decode(request)?;
    let (options, expect) = wire.options.split(request)?;
    let args = ReplayArgs {
        curl: wire.curl,
        request: options,
        expect,
    };
    let output = domain::run_replay(args, "replay")?;
    if !output.ok {
        return Err(AppError::external(
            "HTTP_ASSERTION_FAILED",
            format!(
                "{} expectation(s) failed: {}",
                output.assertions.failed,
                output.assertions.failures.join("; ")
            ),
        ));
    }
    Ok(serde_json::to_value(output)?)
}

fn typed_assert(
    request: &TypedInvocationRequest,
    command_name: &'static str,
) -> Result<Value, AppError> {
    let wire: AssertWireArgs = decode(request)?;
    let args = wire.into_args(request);
    let (output, _) = domain::run_assert(args, ah_output::OutputMode::Json, command_name)?;
    if output.summary.failed > 0 {
        return Err(AppError::external(
            "HTTP_ASSERTION_FAILED",
            format!(
                "{} of {} HTTP assertion case(s) failed",
                output.summary.failed, output.summary.total
            ),
        ));
    }
    Ok(serde_json::to_value(output)?)
}

/// Arguments are validated against the derived input schema before dispatch, so
/// a failure here means the schema and the wire type disagree.
fn decode<T: serde::de::DeserializeOwned>(request: &TypedInvocationRequest) -> Result<T, AppError> {
    serde_json::from_value(request.arguments.clone()).map_err(|error| {
        AppError::invalid_argument(format!(
            "invalid arguments for {}: {error}",
            request.command
        ))
    })
}

fn resolved_basic_credential(
    credentials: Option<&Value>,
    resolved_secrets: &BTreeMap<String, ResolvedSecret>,
) -> Result<Option<BasicCredential>, AppError> {
    let credentials = credentials.and_then(Value::as_object);
    if let Some(slot) = credentials.and_then(|items| items.keys().find(|key| *key != "basic")) {
        return Err(AppError::invalid_argument(format!(
            "unsupported HTTP credential slot '{slot}'"
        )));
    }
    let public_id = credentials
        .and_then(|items| items.get("basic"))
        .and_then(Value::as_str);
    let resolved = basic_from_resolved_secrets(resolved_secrets)?;
    match (public_id, &resolved) {
        (None, None) | (Some(_), Some(_)) => {}
        _ => {
            return Err(AppError::external(
                "SECRET_REQUIRED",
                "HTTP Basic credential was not resolved",
            ));
        }
    }
    if let Some((public_id, secret)) = public_id.zip(resolved_secrets.get("basic"))
        && secret.id != public_id
    {
        return Err(AppError::external(
            "SECRET_KIND_MISMATCH",
            "resolved HTTP Basic credential does not match the selected credential",
        ));
    }
    Ok(resolved)
}

/// Binds `--credential basic=ID` on the direct CLI path. The host resolved the
/// mapping, so there is no public credential map to cross-check here.
pub fn bind_resolved_credentials(
    args: &mut HttpArgs,
    secrets: &BTreeMap<String, ResolvedSecret>,
) -> Result<(), InvocationResponse> {
    if secrets.is_empty() {
        return Ok(());
    }
    let credential = basic_from_resolved_secrets(secrets).map_err(|error| {
        InvocationResponse::error_diagnostic(error.diagnostic().with_domain("http"))
    })?;
    let request = match &mut args.command {
        HttpCommand::Request(args) => &mut args.request,
        HttpCommand::Get(args) | HttpCommand::Post(args) => &mut args.request,
        HttpCommand::Replay(args) => &mut args.request,
        _ => {
            return Err(InvocationResponse::error(
                "INVALID_ARGUMENT",
                "--credential is supported for http request, get, post, and replay",
            )
            .with_error_domain("http"));
        }
    };
    request.resolved_basic = credential;
    Ok(())
}

/// Shared by the typed and direct CLI paths: validates the slot and shape of the
/// credential the host resolved.
fn basic_from_resolved_secrets(
    secrets: &BTreeMap<String, ResolvedSecret>,
) -> Result<Option<BasicCredential>, AppError> {
    if let Some(slot) = secrets.keys().find(|slot| slot.as_str() != "basic") {
        return Err(AppError::invalid_argument(format!(
            "unsupported resolved HTTP credential slot '{slot}'"
        )));
    }
    let Some(resolved) = secrets.get("basic") else {
        return Ok(None);
    };
    if resolved.kind != "http-basic" {
        return Err(AppError::external(
            "SECRET_KIND_MISMATCH",
            "resolved HTTP Basic credential does not match the selected credential",
        ));
    }
    let username = resolved.values.get("username").ok_or_else(|| {
        AppError::external(
            "SECRET_REQUIRED",
            "resolved HTTP Basic credential has no username",
        )
    })?;
    let password = resolved.values.get("password").ok_or_else(|| {
        AppError::external(
            "SECRET_REQUIRED",
            "resolved HTTP Basic credential has no password",
        )
    })?;
    if username.trim().is_empty() {
        return Err(AppError::invalid_argument(
            "resolved HTTP Basic username must not be empty",
        ));
    }
    Ok(Some(BasicCredential::new(username, password)))
}

fn resolve_context_path(cwd: &str, path: &str) -> PathBuf {
    rebase_path(Path::new(cwd), Path::new(path))
}

fn rebase_path(cwd: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    }
}

fn request_deadline(request: &TypedInvocationRequest) -> Option<Instant> {
    Instant::now().checked_add(Duration::from_millis(request.context.remaining_timeout_ms))
}

fn retryable_http_error(code: &str) -> bool {
    code.contains("HTTP_REQUEST") || code.contains("HTTP_RESPONSE") || code.contains("TIMEOUT")
}

fn execute_shortcut(
    command_name: &'static str,
    method: &str,
    args: MethodShortcutArgs,
    options: &GlobalOptions,
    sink: &OutputSink,
) -> Result<(), AppError> {
    execute_request(
        domain::run_request_shortcut(command_name, method, args),
        options,
        sink,
    )
}

fn execute_request(
    request: Result<domain::HttpRequestOutput, AppError>,
    options: &GlobalOptions,
    sink: &OutputSink,
) -> Result<(), AppError> {
    let payload = request?;
    let failed = !payload.ok;
    output::emit_request(payload, options.limit, &mut Emitter::to_sink(options, sink))?;
    if failed {
        return Err(AppError::external(
            "HTTP_ASSERTION_FAILED",
            "request expectations failed",
        ));
    }
    Ok(())
}

fn execute_assert(
    args: AssertArgs,
    options: &GlobalOptions,
    sink: &OutputSink,
    command_name: &'static str,
) -> Result<(), AppError> {
    let (output, report_format) = domain::run_assert(args, options.output, command_name)?;
    let failed = output.summary.failed > 0;
    output::emit_assert(&output, report_format, &mut Emitter::to_sink(options, sink))?;
    if failed {
        return Err(AppError::external(
            "HTTP_ASSERTION_FAILED",
            format!(
                "{} of {} HTTP assertion case(s) failed",
                output.summary.failed, output.summary.total
            ),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use ah_plugin_api::{ExecutionContextWire, ResolvedSecret};
    use serde_json::json;

    use super::*;

    fn typed_request_options(
        request: &TypedInvocationRequest,
    ) -> Result<RequestOptionsArgs, AppError> {
        let wire: MethodShortcutWireArgs = decode(request)?;
        wire.options.split(request).map(|(options, _)| options)
    }

    #[test]
    fn typed_http_schemas_expose_retry_options() {
        let catalog = command_catalog();
        for command_id in [
            "http.request",
            "http.get",
            "http.replay",
            "http.assert",
            "http.run",
        ] {
            let command = catalog
                .commands
                .iter()
                .find(|command| command.id == command_id)
                .expect("command should exist");
            assert_eq!(command.input_schema["properties"]["retry"]["minimum"], 0);
            assert_eq!(
                command.input_schema["properties"]["retry_delay_ms"]["minimum"],
                0
            );
        }
    }

    #[test]
    fn only_request_get_post_and_replay_declare_http_basic_slot() {
        let catalog = command_catalog();

        for command in &catalog.commands {
            let expected = matches!(
                command.id.as_str(),
                "http.request" | "http.get" | "http.post" | "http.replay"
            );
            assert_eq!(
                command.secret_slots.len(),
                usize::from(expected),
                "{}",
                command.id
            );
            if expected {
                assert_eq!(command.secret_slots[0].name, "basic");
                assert_eq!(command.secret_slots[0].accepted_kinds, ["http-basic"]);
                assert!(!command.secret_slots[0].required);
            }
        }
    }

    #[test]
    fn typed_request_options_bind_matching_resolved_basic_auth() {
        let request = TypedInvocationRequest::new(
            "http.get",
            json!({"url": "https://example.test", "credentials": {"basic": "api"}}),
            ExecutionContextWire::new("http-secret", ".", None, 1_000),
        )
        .with_resolved_secrets(BTreeMap::from([(
            "basic".to_owned(),
            ResolvedSecret {
                id: "api".to_owned(),
                kind: "http-basic".to_owned(),
                values: BTreeMap::from([
                    ("username".to_owned(), "vault-user".to_owned()),
                    ("password".to_owned(), "http-private-sentinel".to_owned()),
                ]),
            },
        )]));

        let options = typed_request_options(&request).expect("credential should bind");
        let basic = options.resolved_basic.expect("resolved basic auth");
        assert_eq!(basic.username, "vault-user");
        assert_eq!(basic.password, "http-private-sentinel");
    }

    #[test]
    fn typed_http_rejects_unresolved_and_wrong_slots_without_leaking_values() {
        let unresolved = TypedInvocationRequest::new(
            "http.get",
            json!({"url": "https://example.test", "credentials": {"basic": "api"}}),
            ExecutionContextWire::new("http-unresolved", ".", None, 1_000),
        );
        assert_eq!(
            typed_request_options(&unresolved)
                .expect_err("public id requires private resolution")
                .code(),
            "SECRET_REQUIRED"
        );

        let wrong_slot = TypedInvocationRequest::new(
            "http.get",
            json!({"url": "https://example.test", "credentials": {"basic": "api"}}),
            ExecutionContextWire::new("http-wrong-slot", ".", None, 1_000),
        )
        .with_resolved_secrets(BTreeMap::from([(
            "other".to_owned(),
            ResolvedSecret {
                id: "api".to_owned(),
                kind: "http-basic".to_owned(),
                values: BTreeMap::from([
                    ("username".to_owned(), "leak-user".to_owned()),
                    ("password".to_owned(), "http-leak-sentinel".to_owned()),
                ]),
            },
        )]));
        let response = invoke_typed(&wrong_slot);
        let serialized = serde_json::to_string(&response).expect("response serializes");
        assert!(!response.success);
        assert!(!serialized.contains("leak-user"));
        assert!(!serialized.contains("http-leak-sentinel"));
    }

    #[test]
    fn typed_http_rejects_authorization_header_with_resolved_basic() {
        let request = TypedInvocationRequest::new(
            "http.get",
            json!({
                "url": "https://example.test",
                "headers": ["aUtHoRiZaTiOn: Bearer raw-header-sentinel"],
                "credentials": {"basic": "api"}
            }),
            ExecutionContextWire::new("http-auth-header", ".", None, 1_000),
        )
        .with_resolved_secrets(BTreeMap::from([(
            "basic".to_owned(),
            ResolvedSecret {
                id: "api".to_owned(),
                kind: "http-basic".to_owned(),
                values: BTreeMap::from([
                    ("username".to_owned(), "vault-user".to_owned()),
                    (
                        "password".to_owned(),
                        "http-header-password-sentinel".to_owned(),
                    ),
                ]),
            },
        )]));

        let response = invoke_typed(&request);
        let error = response.error.expect("conflict should fail");
        let serialized = serde_json::to_string(&error).expect("error serializes");
        assert_eq!(error.code, "INVALID_ARGUMENT");
        assert!(!serialized.contains("raw-header-sentinel"));
        assert!(!serialized.contains("http-header-password-sentinel"));
    }

    #[test]
    fn typed_replay_rejects_curl_authorization_header_with_resolved_basic() {
        let request = TypedInvocationRequest::new(
            "http.replay",
            json!({
                "curl": "curl https://example.test -H 'AUTHORIZATION: Bearer replay-header-sentinel'",
                "credentials": {"basic": "api"}
            }),
            ExecutionContextWire::new("http-replay-auth-header", ".", None, 1_000),
        )
        .with_resolved_secrets(BTreeMap::from([(
            "basic".to_owned(),
            ResolvedSecret {
                id: "api".to_owned(),
                kind: "http-basic".to_owned(),
                values: BTreeMap::from([
                    ("username".to_owned(), "vault-user".to_owned()),
                    ("password".to_owned(), "http-replay-password-sentinel".to_owned()),
                ]),
            },
        )]));

        let response = invoke_typed(&request);
        let error = response.error.expect("replay conflict should fail");
        let serialized = serde_json::to_string(&error).expect("error serializes");
        assert_eq!(error.code, "INVALID_ARGUMENT");
        assert!(!serialized.contains("replay-header-sentinel"));
        assert!(!serialized.contains("http-replay-password-sentinel"));
    }
}
