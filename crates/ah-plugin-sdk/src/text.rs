//! An argument a caller may pass inline or point at a file.
//!
//! Issue bodies and release notes are the two that need it, and both SCM
//! plugins offer `--body`/`--body-file` and `--notes`/`--notes-file`. The rule
//! is the same in every case: one or the other, never both, and for a required
//! field never neither.
//!
//! Like [`crate::git`] and unlike [`crate::logs`], the failures are rendered
//! here, because the two copies this replaces had the same codes and the same
//! wording. The `--{field}`/`--{field}-file` shape is baked into the message,
//! so `field_name` is the flag's name without its dashes.

use ah_plugin_api::InvocationResponse;

/// The text of an optional field, from `inline` or from the file at `file`.
///
/// # Errors
///
/// `INVALID_ARGUMENT` when both are given, and `FILE_READ_FAILED` when the file
/// cannot be read.
pub fn optional(
    inline: Option<String>,
    file: Option<String>,
    field_name: &str,
) -> Result<Option<String>, InvocationResponse> {
    match (inline, file) {
        (Some(value), None) => Ok(Some(value)),
        (None, Some(path)) => std::fs::read_to_string(&path).map(Some).map_err(|error| {
            InvocationResponse::error(
                "FILE_READ_FAILED",
                format!("failed to read {field_name} file '{path}': {error}"),
            )
        }),
        (None, None) => Ok(None),
        (Some(_), Some(_)) => Err(InvocationResponse::error(
            "INVALID_ARGUMENT",
            format!("use either --{field_name} or --{field_name}-file, not both"),
        )),
    }
}

/// The same, for a field the command cannot run without.
///
/// Whitespace-only counts as absent: a file containing a newline is a mistake,
/// not a body.
///
/// # Errors
///
/// Everything [`optional`] reports, plus `INVALID_ARGUMENT` when neither is
/// given or the value is blank.
pub fn required(
    inline: Option<String>,
    file: Option<String>,
    field_name: &str,
) -> Result<String, InvocationResponse> {
    match optional(inline, file, field_name)? {
        Some(value) if !value.trim().is_empty() => Ok(value),
        _ => Err(InvocationResponse::error(
            "INVALID_ARGUMENT",
            format!("--{field_name} or --{field_name}-file is required"),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn code(error: &InvocationResponse) -> Option<&str> {
        error.error_code.as_deref()
    }

    #[test]
    fn an_inline_value_is_taken_as_it_is() {
        let value = optional(Some("  spaced  ".to_owned()), None, "body")
            .expect("an inline value is accepted");

        assert_eq!(value.as_deref(), Some("  spaced  "));
    }

    #[test]
    fn a_file_is_read_whole() {
        let path = std::env::temp_dir().join("ah-plugin-sdk-text-body.txt");
        std::fs::write(&path, "from a file\n").expect("the file should be written");

        let value = optional(None, Some(path.to_string_lossy().into_owned()), "body")
            .expect("the file is read");

        assert_eq!(value.as_deref(), Some("from a file\n"));
    }

    #[test]
    fn both_at_once_is_the_callers_mistake() {
        let error = optional(Some("inline".to_owned()), Some("path".to_owned()), "notes")
            .expect_err("both is refused");

        assert_eq!(code(&error), Some("INVALID_ARGUMENT"));
        assert_eq!(
            error.error_message.as_deref(),
            Some("use either --notes or --notes-file, not both")
        );
    }

    #[test]
    fn an_unreadable_file_names_the_field_and_the_path() {
        let error = optional(None, Some("no-such-file.txt".to_owned()), "body")
            .expect_err("a missing file is refused");

        assert_eq!(code(&error), Some("FILE_READ_FAILED"));
        assert!(
            error
                .error_message
                .as_deref()
                .unwrap_or_default()
                .starts_with("failed to read body file 'no-such-file.txt': "),
            "{:?}",
            error.error_message
        );
    }

    #[test]
    fn neither_is_absent_rather_than_empty() {
        assert_eq!(
            optional(None, None, "body").expect("absent is allowed"),
            None
        );

        let error = required(None, None, "body").expect_err("a required field is not optional");
        assert_eq!(code(&error), Some("INVALID_ARGUMENT"));
        assert_eq!(
            error.error_message.as_deref(),
            Some("--body or --body-file is required")
        );
    }

    /// A file holding nothing but a newline is the case this exists for.
    #[test]
    fn whitespace_does_not_satisfy_a_required_field() {
        let error =
            required(Some("\n  \t".to_owned()), None, "title").expect_err("blank is not a value");

        assert_eq!(code(&error), Some("INVALID_ARGUMENT"));
    }
}
