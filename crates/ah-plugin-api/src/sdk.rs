//! Conveniences for writing a plugin in Rust.
//!
//! None of this crosses the ABI - it is the Rust-side sugar over the boundary
//! `abi` defines.

use super::*;

pub fn noninteractive_command<S: AsRef<OsStr>>(program: S) -> Command {
    let mut command = Command::new(program);
    command.env_remove(AH_VAULT_MASTER_KEY_ENV);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;

        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
}

pub mod plugin_capabilities {
    pub const MANUAL_JSON: &str = "manual_json";
    pub const TYPED_COMMANDS_V1: &str = "typed_commands_v1";
}

/// Lets one parsed CLI model accept the credentials the host resolved for
/// `--credential SLOT=ID`. Every plugin implements this once; the default
/// rejects credentials rather than silently dropping them, so a domain only
/// accepts a slot it actually knows.
pub trait BindResolvedSecrets: Sized {
    fn bind_resolved_secrets(
        &mut self,
        secrets: &BTreeMap<String, ResolvedSecret>,
    ) -> Result<(), InvocationResponse> {
        match secrets.keys().next() {
            None => Ok(()),
            Some(slot) => Err(InvocationResponse::error(
                "INVALID_ARGUMENT",
                format!("this command does not accept the '{slot}' credential slot"),
            )),
        }
    }
}

/// Converts a raw invocation request into a typed response using the plugin-local
/// argument parser and command executor.
#[allow(clippy::not_unsafe_ptr_arg_deref)]
pub fn invoke_request_with_parser<TArgs, TParse, TExecute>(
    expected_domain: &str,
    request_json: *const c_char,
    parse_args: TParse,
    execute: TExecute,
) -> InvocationResponse
where
    TArgs: BindResolvedSecrets,
    TParse: Fn(&[String]) -> Result<TArgs, InvocationResponse>,
    TExecute: Fn(TArgs, &GlobalOptionsWire) -> InvocationResponse,
{
    let request_json = match unsafe { c_ptr_to_string(request_json) } {
        Ok(value) => value,
        Err(error) => {
            return InvocationResponse::error(
                "INVALID_ARGUMENT",
                format!("invalid request pointer: {error}"),
            );
        }
    };

    let request = match serde_json::from_str::<InvocationRequest>(&request_json) {
        Ok(value) => value,
        Err(error) => {
            return InvocationResponse::error(
                "INVALID_ARGUMENT",
                format!("invalid request JSON: {error}"),
            );
        }
    };

    if request.domain != expected_domain {
        return InvocationResponse::error(
            "INVALID_ARGUMENT",
            format!(
                "plugin domain mismatch: expected '{expected_domain}', got '{}'",
                request.domain
            ),
        );
    }

    let normalized = match normalize_invocation_argv(&request.argv, request.globals) {
        Ok(value) => value,
        Err(error) => return error.with_error_domain(expected_domain),
    };

    let mut parsed = match parse_args(&normalized.argv) {
        Ok(value) => value,
        Err(response) => return response.with_error_domain(expected_domain),
    };

    if let Err(response) = parsed.bind_resolved_secrets(&request.resolved_secrets) {
        return response.with_error_domain(expected_domain);
    }

    execute(parsed, &normalized.globals).with_error_domain(expected_domain)
}

/// Runs a plugin parser and executor without allowing an unwind to cross the C ABI boundary.
pub fn invoke_request_with_parser_catch_unwind<TArgs, TParse, TExecute>(
    expected_domain: &str,
    request_json: *const c_char,
    parse_args: TParse,
    execute: TExecute,
) -> InvocationResponse
where
    TArgs: BindResolvedSecrets,
    TParse: Fn(&[String]) -> Result<TArgs, InvocationResponse>,
    TExecute: Fn(TArgs, &GlobalOptionsWire) -> InvocationResponse,
{
    match catch_unwind(AssertUnwindSafe(|| {
        invoke_request_with_parser(expected_domain, request_json, parse_args, execute)
    })) {
        Ok(response) => response,
        Err(_) => InvocationResponse::error(
            "PLUGIN_PANIC",
            format!("plugin '{expected_domain}' panicked while handling invocation"),
        )
        .with_error_domain(expected_domain),
    }
}
