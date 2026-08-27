//! Running `psql` and reading what it wrote.
//!
//! Three shapes come back - an array, an object, a scalar - and all three go
//! through one capture that sets the connection environment, prepends the
//! tool's `bin` to `PATH`, and bounds nothing else: the queries here are the
//! plugin's own, so what limits the output is the `LIMIT` in the SQL.

use super::*;

#[derive(Debug)]
pub(crate) enum PsqlOutputMode {
    Json,
    Raw { single_transaction: bool },
}

#[derive(Debug)]
pub(crate) struct PsqlOutput {
    pub(crate) stdout: String,
    pub(crate) stderr: String,
}

pub(crate) fn run_json_array<T>(
    context: &ToolContext,
    connection: &ConnectionArgs,
    sql: &str,
    read_only: bool,
) -> Result<Vec<T>, InvocationResponse>
where
    T: DeserializeOwned,
{
    let output = run_psql_capture(context, connection, sql, PsqlOutputMode::Json, read_only)?;
    serde_json::from_str::<Vec<T>>(output.stdout.trim()).map_err(|error| {
        InvocationResponse::error(
            "POSTGRES_RESPONSE_INVALID",
            format!("failed to decode PostgreSQL JSON array response: {error}"),
        )
    })
}

pub(crate) fn run_json_object<T>(
    context: &ToolContext,
    connection: &ConnectionArgs,
    sql: &str,
    read_only: bool,
) -> Result<T, InvocationResponse>
where
    T: DeserializeOwned,
{
    let output = run_psql_capture(context, connection, sql, PsqlOutputMode::Json, read_only)?;
    serde_json::from_str::<T>(output.stdout.trim()).map_err(|error| {
        InvocationResponse::error(
            "POSTGRES_RESPONSE_INVALID",
            format!("failed to decode PostgreSQL JSON object response: {error}"),
        )
    })
}

pub(crate) fn run_json_value(
    context: &ToolContext,
    connection: &ConnectionArgs,
    sql: &str,
    read_only: bool,
) -> Result<Value, InvocationResponse> {
    let output = run_psql_capture(context, connection, sql, PsqlOutputMode::Json, read_only)?;
    serde_json::from_str::<Value>(output.stdout.trim()).map_err(|error| {
        InvocationResponse::error(
            "POSTGRES_RESPONSE_INVALID",
            format!("failed to decode PostgreSQL JSON response: {error}"),
        )
    })
}

pub(crate) fn run_psql_capture(
    context: &ToolContext,
    connection: &ConnectionArgs,
    sql: &str,
    mode: PsqlOutputMode,
    read_only: bool,
) -> Result<PsqlOutput, InvocationResponse> {
    let mut command = noninteractive_command(&context.psql_path);
    command
        .args(["-X", "-v", "ON_ERROR_STOP=1", "--no-password", "-q"])
        .env("PATH", prepend_path_env(&context.bin_dir))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    match mode {
        PsqlOutputMode::Json => {
            command.args(["-t", "-A"]);
        }
        PsqlOutputMode::Raw { single_transaction } => {
            if single_transaction {
                command.arg("--single-transaction");
            }
        }
    }
    apply_connection_env(&mut command, connection, read_only)?;
    command.args(["-c", sql]);
    let output = command.output().map_err(|error| {
        InvocationResponse::error(
            "POSTGRES_PSQL_FAILED",
            format!(
                "failed to execute '{}': {error}",
                context.psql_path.display()
            ),
        )
    })?;
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    if !output.status.success() {
        return Err(InvocationResponse::error(
            "POSTGRES_PSQL_FAILED",
            format!(
                "psql failed with exit code {:?}: {}",
                output.status.code(),
                render::truncate_for_error(&stderr, 1200)
            ),
        ));
    }
    Ok(PsqlOutput { stdout, stderr })
}

pub(crate) fn apply_connection_env(
    command: &mut Command,
    connection: &ConnectionArgs,
    read_only: bool,
) -> Result<(), InvocationResponse> {
    if let Some(host) = &connection.host {
        command.env("PGHOST", host);
    }
    if let Some(port) = connection.port {
        command.env("PGPORT", port.to_string());
    }
    if let Some(database) = &connection.database {
        command.env("PGDATABASE", database);
    }
    if let Some(user) = &connection.user {
        command.env("PGUSER", user);
    }
    if let Some(service) = &connection.service {
        command.env("PGSERVICE", service);
    }
    if let Some(sslmode) = &connection.sslmode {
        command.env("PGSSLMODE", sslmode);
    }
    if let Some(password) = &connection.resolved_password {
        command.env("PGPASSWORD", password.expose());
    } else if let Some(password_env) = &connection.password_env {
        let password = env::var(password_env).map_err(|_| {
            InvocationResponse::error(
                "POSTGRES_PASSWORD_ENV_MISSING",
                format!("password environment variable is not set: {password_env}"),
            )
        })?;
        command.env("PGPASSWORD", password);
    }
    if connection.connect_timeout_secs > 0 {
        command.env(
            "PGCONNECT_TIMEOUT",
            connection.connect_timeout_secs.to_string(),
        );
    }
    let mut pgoptions = env::var("PGOPTIONS").unwrap_or_default();
    if let Some(timeout_ms) = connection.statement_timeout_ms {
        append_pgoption(
            &mut pgoptions,
            &format!("-c statement_timeout={}ms", timeout_ms),
        );
    }
    if read_only {
        append_pgoption(&mut pgoptions, "-c default_transaction_read_only=on");
    }
    if !pgoptions.trim().is_empty() {
        command.env("PGOPTIONS", pgoptions);
    }
    Ok(())
}

pub(crate) fn append_pgoption(target: &mut String, value: &str) {
    if !target.trim().is_empty() {
        target.push(' ');
    }
    target.push_str(value);
}

pub(crate) fn prepend_path_env(bin_dir: &Path) -> std::ffi::OsString {
    let old_path = env::var_os("PATH").unwrap_or_default();
    let mut paths = vec![bin_dir.to_path_buf()];
    paths.extend(env::split_paths(&old_path));
    env::join_paths(paths).unwrap_or(old_path)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    #[test]
    fn resolved_password_is_applied_only_as_pgpassword() {
        let mut command = Command::new("psql");
        let connection = ConnectionArgs {
            host: None,
            port: None,
            database: None,
            user: None,
            service: None,
            sslmode: None,
            password_env: None,
            resolved_password: Some(SecretValue::new("postgres-boundary-sentinel")),
            connect_timeout_secs: 0,
            statement_timeout_ms: None,
        };

        apply_connection_env(&mut command, &connection, false).expect("env should apply");

        let envs = command
            .get_envs()
            .map(|(key, value)| {
                (
                    key.to_string_lossy().into_owned(),
                    value.map(|value| value.to_string_lossy().into_owned()),
                )
            })
            .collect::<BTreeMap<_, _>>();
        assert_eq!(
            envs.get("PGPASSWORD").and_then(Option::as_deref),
            Some("postgres-boundary-sentinel")
        );
        assert!(!format!("{connection:?}").contains("postgres-boundary-sentinel"));
    }
}
