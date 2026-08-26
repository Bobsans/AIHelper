//! A small JSONPath subset: dotted segments with optional bracket indices.
//!
//! Split out of `domain.rs` because it parses untrusted input and is worth
//! reviewing and fuzzing on its own.

use serde_json::Value;

use crate::error::AppError;

pub(super) fn resolve_json_path<'a>(value: &'a Value, path: &str) -> Option<&'a Value> {
    if path.trim().is_empty() {
        return None;
    }

    let mut current = value;
    for token in parse_json_path_tokens(path).ok()? {
        current = match token {
            JsonPathToken::Key(key) => current.get(key.as_str())?,
            JsonPathToken::Index(index) => current.get(index)?,
        };
    }
    Some(current)
}

#[derive(Debug)]
pub(super) enum JsonPathToken {
    Key(String),
    Index(usize),
}

pub(super) fn parse_json_path_tokens(path: &str) -> Result<Vec<JsonPathToken>, AppError> {
    let mut tokens = Vec::new();
    for part in path.split('.') {
        if part.trim().is_empty() {
            return Err(AppError::invalid_argument(format!(
                "invalid json path '{path}'"
            )));
        }
        parse_json_path_part(part, &mut tokens)?;
    }
    Ok(tokens)
}

fn parse_json_path_part(part: &str, out: &mut Vec<JsonPathToken>) -> Result<(), AppError> {
    let mut remaining = part;
    if let Some(index_start) = remaining.find('[') {
        if index_start > 0 {
            out.push(JsonPathToken::Key(remaining[..index_start].to_owned()));
        }
        remaining = &remaining[index_start..];
    } else {
        out.push(JsonPathToken::Key(remaining.to_owned()));
        return Ok(());
    }

    while !remaining.is_empty() {
        if !remaining.starts_with('[') {
            return Err(AppError::invalid_argument(format!(
                "invalid json path segment '{part}'"
            )));
        }
        let close = remaining.find(']').ok_or_else(|| {
            AppError::invalid_argument(format!("invalid json path segment '{part}'"))
        })?;
        let index_text = &remaining[1..close];
        let index = index_text.parse::<usize>().map_err(|_| {
            AppError::invalid_argument(format!("invalid json path segment '{part}'"))
        })?;
        out.push(JsonPathToken::Index(index));
        remaining = &remaining[(close + 1)..];
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::super::hostile_input::{PATH_ALPHABET, Rng};
    use super::*;

    #[test]
    fn json_path_supports_arrays() {
        let value: Value = serde_json::json!({
            "items": [{"name":"first"}, {"name":"second"}]
        });
        let resolved = resolve_json_path(&value, "items[1].name").expect("path should resolve");
        assert_eq!(resolved, "second");
    }

    /// The parser slices by byte offset around `[`, `]` and `.`, so the inputs
    /// that matter are the ones where those land beside a multi-byte character.
    /// Nothing here may panic, whatever it decides about the path.
    #[test]
    fn generated_paths_never_panic() {
        let mut rng = Rng::new(0x5eed_0001);
        let document = json!({
            "a": [{"b": [1, 2, {"c": "d"}]}, null],
            "": {"": []},
            "é": {"中": [true]},
        });

        for _ in 0..20_000 {
            let path = rng.string(PATH_ALPHABET, 24);
            if let Ok(tokens) = parse_json_path_tokens(&path) {
                // One token per segment at most; a path cannot expand into
                // more work than it has characters.
                assert!(
                    tokens.len() <= path.len() + 1,
                    "{path:?} produced {} tokens",
                    tokens.len()
                );
            }
            let _ = resolve_json_path(&document, &path);
        }
    }

    /// The index is parsed with `usize::from_str`, which must reject rather
    /// than wrap, and a long run of brackets must not recurse.
    #[test]
    fn hostile_indices_and_nesting_are_rejected_not_survived() {
        assert!(parse_json_path_tokens("a[99999999999999999999999]").is_err());
        assert!(parse_json_path_tokens("a[-1]").is_err());
        assert!(parse_json_path_tokens("a[").is_err());
        assert!(parse_json_path_tokens("a[]").is_err());
        assert!(parse_json_path_tokens("a[[[[[[[[[[]").is_err());

        let deep = "a".to_owned() + &"[0]".repeat(10_000);
        assert_eq!(parse_json_path_tokens(&deep).unwrap().len(), 10_001);
    }
}
