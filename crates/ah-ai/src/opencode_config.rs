use std::{
    env,
    path::{Path, PathBuf},
};

use jsonc_parser::{
    ParseOptions,
    cst::{CstInputValue, CstRootNode},
};
use serde_json::Value;

use ah_error::AppError;

use super::targets::{Scope, ServerSpec, home_dir};

pub fn path(scope: Scope, project_root: &Path) -> Result<PathBuf, AppError> {
    let candidates = paths(scope, project_root)?;
    if let Some(existing) = candidates.iter().rev().find(|path| path.is_file()) {
        return Ok(existing.clone());
    }
    if scope.is_project_local() {
        Ok(project_root.join("opencode.json"))
    } else {
        Ok(config_directory()?.join("opencode.jsonc"))
    }
}

pub fn lookup(
    scope: Scope,
    project_root: &Path,
    name: &str,
) -> Result<Option<ServerSpec>, AppError> {
    let mut effective = serde_json::Map::new();
    for path in paths(scope, project_root)? {
        if let Some(Value::Object(entry)) = entry(&path, name)? {
            effective.extend(entry);
        }
    }
    if effective.is_empty() {
        return Ok(None);
    }
    Ok(spec_from_entry(&Value::Object(effective)))
}

pub fn contains(scope: Scope, project_root: &Path, name: &str) -> Result<bool, AppError> {
    for path in paths(scope, project_root)? {
        if entry(&path, name)?.is_some() {
            return Ok(true);
        }
    }
    Ok(false)
}

pub fn lookup_path(path: &Path, name: &str) -> Result<Option<ServerSpec>, AppError> {
    Ok(entry(path, name)?.and_then(|entry| spec_from_entry(&entry)))
}

fn entry(path: &Path, name: &str) -> Result<Option<Value>, AppError> {
    let Some(root) = read(path)? else {
        return Ok(None);
    };
    Ok(root
        .object_value()
        .and_then(|root| root.object_value("mcp"))
        .and_then(|mcp| mcp.get(name))
        .and_then(|entry| entry.to_serde_value()))
}

pub fn merge(path: &Path, name: &str, spec: &ServerSpec) -> Result<(), AppError> {
    let root = read(path)?.unwrap_or_else(empty_document);
    let document = root
        .object_value_or_create()
        .ok_or_else(|| malformed(path))?;
    let mcp = document
        .object_value_or_create("mcp")
        .ok_or_else(|| malformed(path))?;
    let entry = entry_for(spec);
    if let Some(existing) = mcp.get(name) {
        existing.set_value(entry);
    } else {
        mcp.append(name, entry);
    }
    ah_persist::atomic_write(path, root.to_string().as_bytes())
}

pub fn remove(scope: Scope, project_root: &Path, name: &str) -> Result<(), AppError> {
    let documents = paths(scope, project_root)?
        .into_iter()
        .map(|path| read(&path).map(|root| (path, root)))
        .collect::<Result<Vec<_>, _>>()?;
    for (path, root) in documents {
        let Some(root) = root else {
            continue;
        };
        let Some(entry) = root
            .object_value()
            .and_then(|root| root.object_value("mcp"))
            .and_then(|mcp| mcp.get(name))
        else {
            continue;
        };
        entry.remove();
        ah_persist::atomic_write(&path, root.to_string().as_bytes())?;
    }
    Ok(())
}

fn paths(scope: Scope, project_root: &Path) -> Result<Vec<PathBuf>, AppError> {
    if scope.is_project_local() {
        return Ok(vec![
            project_root.join("opencode.json"),
            project_root.join("opencode.jsonc"),
            project_root.join(".opencode").join("opencode.json"),
            project_root.join(".opencode").join("opencode.jsonc"),
        ]);
    }
    let directory = config_directory()?;
    Ok(["config.json", "opencode.json", "opencode.jsonc"]
        .into_iter()
        .map(|name| directory.join(name))
        .collect())
}

fn config_directory() -> Result<PathBuf, AppError> {
    if let Some(directory) = env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty())
    {
        return Ok(directory.join("opencode"));
    }
    Ok(home_dir()?.join(".config").join("opencode"))
}

fn read(path: &Path) -> Result<Option<CstRootNode>, AppError> {
    let contents = match std::fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(source) => return Err(AppError::file_read(path.to_path_buf(), source)),
    };
    if contents.trim().is_empty() {
        return Ok(None);
    }
    CstRootNode::parse(&contents, &ParseOptions::default())
        .map(Some)
        .map_err(|source| {
            AppError::external(
                "AI_CONFIG_UNPARSABLE",
                format!("{} is not valid JSONC: {source}", path.display()),
            )
        })
}

fn empty_document() -> CstRootNode {
    CstRootNode::parse("{}", &ParseOptions::default()).expect("empty JSON object is valid")
}

fn malformed(path: &Path) -> AppError {
    AppError::external(
        "AI_CONFIG_UNPARSABLE",
        format!(
            "{} must contain JSON objects at the root and `mcp`",
            path.display()
        ),
    )
}

fn entry_for(spec: &ServerSpec) -> CstInputValue {
    match spec {
        ServerSpec::Http { url } => CstInputValue::Object(vec![
            ("type".to_owned(), "remote".into()),
            ("url".to_owned(), url.clone().into()),
            ("enabled".to_owned(), true.into()),
        ]),
        ServerSpec::Stdio { command, args } => {
            let command = std::iter::once(command.clone())
                .chain(args.iter().cloned())
                .collect::<Vec<_>>();
            CstInputValue::Object(vec![
                ("type".to_owned(), "local".into()),
                ("command".to_owned(), command.into()),
                ("enabled".to_owned(), true.into()),
            ])
        }
    }
}

fn spec_from_entry(entry: &Value) -> Option<ServerSpec> {
    if entry.get("enabled").and_then(Value::as_bool) == Some(false) {
        return None;
    }
    match entry.get("type").and_then(Value::as_str)? {
        "remote" => Some(ServerSpec::Http {
            url: entry.get("url").and_then(Value::as_str)?.to_owned(),
        }),
        "local" => {
            let mut command = entry
                .get("command")?
                .as_array()?
                .iter()
                .filter_map(Value::as_str);
            Some(ServerSpec::Stdio {
                command: command.next()?.to_owned(),
                args: command.map(str::to_owned).collect(),
            })
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::lookup_path;
    use crate::targets::ServerSpec;

    #[test]
    fn managed_jsonc_file_can_be_inspected_directly() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("opencode.jsonc");
        std::fs::write(
            &path,
            "{\n  // managed\n  \"mcp\": {\"aihelper\": {\"type\": \"remote\", \"url\": \"http://127.0.0.1:8787/mcp\"}}\n}\n",
        )
        .expect("seed managed config");

        assert_eq!(
            lookup_path(&path, "aihelper").expect("config should parse"),
            Some(ServerSpec::Http {
                url: "http://127.0.0.1:8787/mcp".to_owned()
            })
        );
    }
}
