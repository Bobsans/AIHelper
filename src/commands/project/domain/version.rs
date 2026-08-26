//! Reading the project's declared version out of whichever manifest declares
//! it - one parser per format, because no two agree.

use super::*;

pub(super) fn detect_versions(
    root: &Path,
    candidates: &[PathBuf],
) -> Result<Vec<ProjectVersionEntry>, AppError> {
    let mut versions = Vec::new();
    for file in candidates {
        let Some(name) = file.file_name().and_then(|value| value.to_str()) else {
            continue;
        };
        let lower_name = name.to_ascii_lowercase();
        let rel = normalize_relative(root, file);
        let parsed = match lower_name.as_str() {
            "cargo.toml" => parse_cargo_version(file, &rel)?,
            "package.json" => parse_package_json_version(file, &rel)?,
            "composer.json" => parse_package_json_like_version(file, &rel, "composer")?,
            "pyproject.toml" => parse_pyproject_version(file, &rel)?,
            "pubspec.yaml" => parse_pubspec_version(file, &rel)?,
            "mix.exs" => parse_assignment_version(file, &rel, "mix", "medium")?,
            "pom.xml" => parse_xml_version(file, &rel, "maven", "medium")?,
            "build.gradle" | "build.gradle.kts" => parse_gradle_version(file, &rel)?,
            _ if lower_name.ends_with(".gemspec") => {
                parse_assignment_version(file, &rel, "gemspec", "medium")?
            }
            _ if lower_name.ends_with(".csproj") => {
                parse_xml_version(file, &rel, "dotnet", "high")?
            }
            _ => None,
        };
        if let Some(entry) = parsed {
            versions.push(entry);
        }
    }
    Ok(versions)
}

pub(super) fn parse_cargo_version(
    path: &Path,
    rel: &str,
) -> Result<Option<ProjectVersionEntry>, AppError> {
    let raw = io::read_to_string(path)?;
    let values = parse_toml_section_values(&raw, "package", &["name", "version"]);
    Ok(values.get("version").map(|version| ProjectVersionEntry {
        kind: "cargo".to_owned(),
        path: rel.to_owned(),
        name: values.get("name").cloned(),
        version: Some(version.clone()),
        confidence: "high".to_owned(),
    }))
}

pub(super) fn parse_pyproject_version(
    path: &Path,
    rel: &str,
) -> Result<Option<ProjectVersionEntry>, AppError> {
    let raw = io::read_to_string(path)?;
    let values = parse_toml_section_values(&raw, "project", &["name", "version"]);
    Ok(values.get("version").map(|version| ProjectVersionEntry {
        kind: "python".to_owned(),
        path: rel.to_owned(),
        name: values.get("name").cloned(),
        version: Some(version.clone()),
        confidence: "high".to_owned(),
    }))
}

pub(super) fn parse_package_json_version(
    path: &Path,
    rel: &str,
) -> Result<Option<ProjectVersionEntry>, AppError> {
    let raw = io::read_to_string(path)?;
    let value = serde_json::from_str::<Value>(&raw)
        .map_err(|source| AppError::json_deserialization(path.into(), source))?;
    let version = value
        .get("version")
        .and_then(Value::as_str)
        .map(str::to_owned);
    Ok(version.map(|version| ProjectVersionEntry {
        kind: "npm".to_owned(),
        path: rel.to_owned(),
        name: value.get("name").and_then(Value::as_str).map(str::to_owned),
        version: Some(version),
        confidence: "high".to_owned(),
    }))
}

pub(super) fn parse_package_json_like_version(
    path: &Path,
    rel: &str,
    kind: &str,
) -> Result<Option<ProjectVersionEntry>, AppError> {
    let raw = io::read_to_string(path)?;
    let value = serde_json::from_str::<Value>(&raw)
        .map_err(|source| AppError::json_deserialization(path.into(), source))?;
    let version = value
        .get("version")
        .and_then(Value::as_str)
        .map(str::to_owned);
    Ok(version.map(|version| ProjectVersionEntry {
        kind: kind.to_owned(),
        path: rel.to_owned(),
        name: value.get("name").and_then(Value::as_str).map(str::to_owned),
        version: Some(version),
        confidence: "high".to_owned(),
    }))
}

pub(super) fn parse_pubspec_version(
    path: &Path,
    rel: &str,
) -> Result<Option<ProjectVersionEntry>, AppError> {
    let raw = io::read_to_string(path)?;
    let mut name = None;
    let mut version = None;
    for line in raw.lines() {
        let trimmed = line.trim();
        if let Some(value) = trimmed.strip_prefix("name:") {
            name = Some(value.trim().trim_matches(&['"', '\''][..]).to_owned());
        } else if let Some(value) = trimmed.strip_prefix("version:") {
            version = Some(value.trim().trim_matches(&['"', '\''][..]).to_owned());
        }
    }
    Ok(version.map(|version| ProjectVersionEntry {
        kind: "pub".to_owned(),
        path: rel.to_owned(),
        name,
        version: Some(version),
        confidence: "medium".to_owned(),
    }))
}

pub(super) fn parse_assignment_version(
    path: &Path,
    rel: &str,
    kind: &str,
    confidence: &str,
) -> Result<Option<ProjectVersionEntry>, AppError> {
    let raw = io::read_to_string(path)?;
    let version_re = Regex::new(r#"(?m)\bversion\s*[:=]\s*['"]([^'"]+)['"]"#)
        .map_err(|error| AppError::invalid_argument(format!("internal regex error: {error}")))?;
    let name_re = Regex::new(r#"(?m)\bname\s*[:=]\s*['"]([^'"]+)['"]"#)
        .map_err(|error| AppError::invalid_argument(format!("internal regex error: {error}")))?;
    let version = version_re
        .captures(&raw)
        .and_then(|captures| captures.get(1))
        .map(|value| value.as_str().to_owned());
    Ok(version.map(|version| ProjectVersionEntry {
        kind: kind.to_owned(),
        path: rel.to_owned(),
        name: name_re
            .captures(&raw)
            .and_then(|captures| captures.get(1))
            .map(|value| value.as_str().to_owned()),
        version: Some(version),
        confidence: confidence.to_owned(),
    }))
}

pub(super) fn parse_xml_version(
    path: &Path,
    rel: &str,
    kind: &str,
    confidence: &str,
) -> Result<Option<ProjectVersionEntry>, AppError> {
    let raw = io::read_to_string(path)?;
    let version = capture_xml_tag(&raw, "Version").or_else(|| capture_xml_tag(&raw, "version"));
    let name =
        capture_xml_tag(&raw, "AssemblyName").or_else(|| capture_xml_tag(&raw, "artifactId"));
    Ok(version.map(|version| ProjectVersionEntry {
        kind: kind.to_owned(),
        path: rel.to_owned(),
        name,
        version: Some(version),
        confidence: confidence.to_owned(),
    }))
}

pub(super) fn parse_gradle_version(
    path: &Path,
    rel: &str,
) -> Result<Option<ProjectVersionEntry>, AppError> {
    let raw = io::read_to_string(path)?;
    let version_re = Regex::new(r#"(?m)^\s*version\s*(?:=|\s)\s*['"]([^'"]+)['"]"#)
        .map_err(|error| AppError::invalid_argument(format!("internal regex error: {error}")))?;
    let name_re = Regex::new(r#"(?m)^\s*rootProject\.name\s*=\s*['"]([^'"]+)['"]"#)
        .map_err(|error| AppError::invalid_argument(format!("internal regex error: {error}")))?;
    let version = version_re
        .captures(&raw)
        .and_then(|captures| captures.get(1))
        .map(|value| value.as_str().to_owned());
    Ok(version.map(|version| ProjectVersionEntry {
        kind: "gradle".to_owned(),
        path: rel.to_owned(),
        name: name_re
            .captures(&raw)
            .and_then(|captures| captures.get(1))
            .map(|value| value.as_str().to_owned()),
        version: Some(version),
        confidence: "medium".to_owned(),
    }))
}

pub(super) fn parse_toml_section_values(
    raw: &str,
    section_name: &str,
    keys: &[&str],
) -> std::collections::BTreeMap<String, String> {
    let mut in_section = false;
    let mut values = std::collections::BTreeMap::new();
    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            in_section = trimmed.trim_matches(&['[', ']'][..]) == section_name;
            continue;
        }
        if !in_section || trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let Some((key, value)) = trimmed.split_once('=') else {
            continue;
        };
        let key = key.trim();
        if !keys.contains(&key) {
            continue;
        }
        let value = value
            .split('#')
            .next()
            .unwrap_or("")
            .trim()
            .trim_matches(&['"', '\''][..])
            .to_owned();
        if !value.is_empty() {
            values.insert(key.to_owned(), value);
        }
    }
    values
}

pub(super) fn capture_xml_tag(raw: &str, tag: &str) -> Option<String> {
    let pattern = format!(r"(?is)<{tag}>\s*([^<]+?)\s*</{tag}>");
    Regex::new(&pattern)
        .ok()?
        .captures(raw)
        .and_then(|captures| captures.get(1))
        .map(|value| value.as_str().trim().to_owned())
        .filter(|value| !value.is_empty())
}
