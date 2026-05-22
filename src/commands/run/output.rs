use crate::{
    cli::GlobalOptions,
    error::AppError,
    output::OutputMode,
    commands::run::domain::RunCheckOutput,
};

pub(crate) fn emit_check_result(
    result: RunCheckOutput,
    options: &GlobalOptions,
) -> Result<(), AppError> {
    if options.quiet {
        return Ok(());
    }

    match options.output {
        OutputMode::Text => {
            println!(
                "success={} exit_code={} timed_out={} duration_ms={}",
                result.success,
                result
                    .exit_code
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "-".to_owned()),
                result.timed_out,
                result.duration_ms
            );
            if !result.stdout.is_empty() {
                println!("stdout:\n{}", result.stdout);
            }
            if !result.stderr.is_empty() {
                eprintln!("stderr:\n{}", result.stderr);
            }
            if result.stdout_truncated {
                eprintln!("warning: stdout truncated");
            }
            if result.stderr_truncated {
                eprintln!("warning: stderr truncated");
            }
        }
        OutputMode::Json => {
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
    }

    Ok(())
}
