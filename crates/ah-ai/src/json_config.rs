use std::path::Path;

use serde_json::{Map, Value, json};

use ah_error::AppError;

use super::targets::{JsonConfig, ServerSpec};

/// Read an agent's JSON configuration. A missing file is not an error; an
/// unreadable one is, and is never overwritten.
pub fn read(path: &Path) -> Result<Option<Value>, AppError> {
    let contents = match std::fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(source) => return Err(AppError::file_read(path.to_path_buf(), source)),
    };
    if contents.trim().is_empty() {
        return Ok(None);
    }
    serde_json::from_str(&contents).map(Some).map_err(|source| {
        AppError::external(
            "AI_CONFIG_UNPARSABLE",
            format!("{} is not valid JSON: {source}", path.display()),
        )
    })
}

pub fn lookup(document: &Value, config: &JsonConfig, name: &str) -> Option<ServerSpec> {
    spec_from_entry(document.get(config.key)?.get(name)?)
}

/// Agents disagree on the HTTP key: Cursor and Copilot use `url`, Gemini uses
/// `httpUrl` for Streamable HTTP and reserves `url` for SSE.
pub fn spec_from_entry(entry: &Value) -> Option<ServerSpec> {
    for key in ["httpUrl", "url"] {
        if let Some(url) = entry.get(key).and_then(Value::as_str) {
            return Some(ServerSpec::Http {
                url: url.to_owned(),
            });
        }
    }
    let command = entry.get("command").and_then(Value::as_str)?;
    Some(ServerSpec::Stdio {
        command: command.to_owned(),
        args: entry
            .get("args")
            .and_then(Value::as_array)
            .map(|values| {
                values
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default(),
    })
}

fn entry_for(spec: &ServerSpec) -> Value {
    match spec {
        ServerSpec::Http { url } => json!({ "type": "http", "url": url }),
        ServerSpec::Stdio { command, args } => json!({ "command": command, "args": args }),
    }
}

/// Insert or replace only this tool's server entry. Every other server and
/// every unrelated key in the document is preserved exactly.
pub fn merge(existing: Option<Value>, config: &JsonConfig, name: &str, spec: &ServerSpec) -> Value {
    let mut document = match existing {
        Some(Value::Object(map)) => map,
        _ => Map::new(),
    };
    let mut servers = match document.remove(config.key) {
        Some(Value::Object(map)) => map,
        _ => Map::new(),
    };
    servers.insert(name.to_owned(), entry_for(spec));
    document.insert(config.key.to_owned(), Value::Object(servers));
    Value::Object(document)
}

/// Remove only this tool's server entry. Returns `None` when it was absent.
pub fn remove(existing: Option<Value>, config: &JsonConfig, name: &str) -> Option<Value> {
    let Some(Value::Object(mut document)) = existing else {
        return None;
    };
    let Some(Value::Object(mut servers)) = document.remove(config.key) else {
        return None;
    };
    servers.remove(name)?;
    document.insert(config.key.to_owned(), Value::Object(servers));
    Some(Value::Object(document))
}

pub fn write(path: &Path, document: &Value) -> Result<(), AppError> {
    ah_persist::atomic_write_json(path, document)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{lookup, merge, read, remove, spec_from_entry};
    use crate::targets::{JsonConfig, ServerSpec};

    fn config() -> JsonConfig {
        JsonConfig {
            key: "mcpServers",
            project_path: Some(".cursor/mcp.json"),
            user_path: Some(".cursor/mcp.json"),
        }
    }

    fn stdio() -> ServerSpec {
        ServerSpec::Stdio {
            command: "C:\\bin\\ah.exe".to_owned(),
            args: vec!["mcp".to_owned(), "serve".to_owned()],
        }
    }

    #[test]
    fn merging_into_a_missing_document_creates_only_our_entry() {
        let document = merge(None, &config(), "ah", &stdio());
        assert_eq!(
            document,
            json!({"mcpServers": {"ah": {"command": "C:\\bin\\ah.exe", "args": ["mcp", "serve"]}}})
        );
    }

    #[test]
    fn merging_preserves_foreign_servers_and_unrelated_keys() {
        let existing = json!({
            "mcpServers": {"other": {"command": "node", "args": ["x.js"]}},
            "someUnrelatedSetting": {"deep": true}
        });
        let document = merge(Some(existing), &config(), "ah", &stdio());
        assert_eq!(
            document["mcpServers"]["other"],
            json!({"command": "node", "args": ["x.js"]}),
            "a foreign server must survive untouched"
        );
        assert_eq!(document["someUnrelatedSetting"], json!({"deep": true}));
        assert!(document["mcpServers"]["ah"].is_object());
    }

    #[test]
    fn merging_replaces_an_existing_entry_of_ours() {
        let existing =
            json!({"mcpServers": {"ah": {"type": "http", "url": "http://127.0.0.1:1/mcp"}}});
        let document = merge(Some(existing), &config(), "ah", &stdio());
        assert_eq!(
            lookup(&document, &config(), "ah"),
            Some(stdio()),
            "the stale http entry must be gone, not merged into"
        );
    }

    #[test]
    fn removal_reports_absence_and_keeps_the_rest() {
        assert!(remove(None, &config(), "ah").is_none());
        assert!(remove(Some(json!({"mcpServers": {}})), &config(), "ah").is_none());

        let existing = json!({"mcpServers": {"ah": {"command": "x"}, "other": {"command": "y"}}});
        let document = remove(Some(existing), &config(), "ah").expect("ah was present");
        assert_eq!(document, json!({"mcpServers": {"other": {"command": "y"}}}));
    }

    #[test]
    fn both_http_key_spellings_are_understood() {
        assert_eq!(
            spec_from_entry(&json!({"httpUrl": "http://127.0.0.1:8787/mcp"})),
            Some(ServerSpec::Http {
                url: "http://127.0.0.1:8787/mcp".to_owned()
            }),
            "Gemini writes Streamable HTTP as httpUrl"
        );
        assert_eq!(
            spec_from_entry(&json!({"type": "http", "url": "http://127.0.0.1:8787/mcp"})),
            Some(ServerSpec::Http {
                url: "http://127.0.0.1:8787/mcp".to_owned()
            })
        );
        assert_eq!(spec_from_entry(&json!({"type": "http"})), None);
    }

    #[test]
    fn an_unparsable_file_is_reported_and_left_alone() {
        let directory = tempfile::tempdir().expect("temp dir");
        let path = directory.path().join("mcp.json");
        std::fs::write(&path, "{ not json").expect("seed file");
        let error = read(&path).expect_err("invalid JSON must be refused");
        assert_eq!(error.code(), "AI_CONFIG_UNPARSABLE");
        assert_eq!(
            std::fs::read_to_string(&path).expect("file should still exist"),
            "{ not json",
            "a file we could not parse must not be touched"
        );
    }

    #[test]
    fn a_missing_or_empty_file_reads_as_absent() {
        let directory = tempfile::tempdir().expect("temp dir");
        let missing = directory.path().join("mcp.json");
        assert_eq!(read(&missing).expect("missing is not an error"), None);
        std::fs::write(&missing, "   \n").expect("seed empty file");
        assert_eq!(read(&missing).expect("empty is not an error"), None);
    }
}
