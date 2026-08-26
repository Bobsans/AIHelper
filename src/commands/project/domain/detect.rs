//! Recognising a project: which manifests are present, and what they imply
//! about the ecosystems and roles in play.

use super::*;

pub(super) fn detect_project(path: &Path) -> Result<ProjectSnapshot, AppError> {
    let root = io::canonical_project_root(path)?;

    let mut ecosystems = BTreeSet::new();
    let mut tools = BTreeSet::new();
    let mut roles = BTreeSet::new();
    let mut files = ProjectFileGroups::default();

    let candidates = io::collect_project_files(&root);
    for file in &candidates {
        let rel = normalize_relative(&root, file);
        let Some(name) = file.file_name().and_then(|value| value.to_str()) else {
            continue;
        };
        for detection in classify_file(&rel, name) {
            if let Some(ecosystem) = detection.ecosystem {
                ecosystems.insert(ecosystem.to_owned());
            }
            if let Some(tool) = detection.tool {
                tools.insert(tool.to_owned());
            }
            if let Some(role) = detection.role {
                roles.insert(role.to_owned());
            }
            push_detected_file(&mut files, detection.group, detection.kind, &rel);
        }
    }

    enrich_project(&root, &mut ecosystems, &mut tools, &mut roles, &files)?;
    let ecosystems = ecosystems.into_iter().collect::<Vec<_>>();
    let tools = tools.into_iter().collect::<Vec<_>>();
    let roles = roles.into_iter().collect::<Vec<_>>();
    let versions = detect_versions(&root, &candidates)?;
    let commands = suggest_commands(&ecosystems, &tools, &files, &root)?;

    Ok(ProjectSnapshot {
        root: core::normalize_path(&root),
        ecosystems,
        tools,
        roles,
        files,
        versions,
        commands,
    })
}

pub(super) fn push_detected_file(
    files: &mut ProjectFileGroups,
    group: FileGroup,
    kind: &str,
    path: &str,
) {
    let target = match group {
        FileGroup::Package => &mut files.packages,
        FileGroup::Lock => &mut files.locks,
        FileGroup::Ci => &mut files.ci,
        FileGroup::Docs => &mut files.docs,
        FileGroup::Changelog => &mut files.changelogs,
        FileGroup::Deploy => &mut files.deploy,
        FileGroup::Infra => &mut files.infra,
        FileGroup::Config => &mut files.config,
        FileGroup::Quality => &mut files.quality,
        FileGroup::Security => &mut files.security,
    };
    push_unique_file(target, detected(kind, path));
}

pub(super) fn push_unique_file(target: &mut Vec<DetectedFile>, file: DetectedFile) {
    if !target
        .iter()
        .any(|existing| existing.kind == file.kind && existing.path == file.path)
    {
        target.push(file);
    }
}

pub(super) fn enrich_project(
    root: &Path,
    ecosystems: &mut BTreeSet<String>,
    tools: &mut BTreeSet<String>,
    roles: &mut BTreeSet<String>,
    files: &ProjectFileGroups,
) -> Result<(), AppError> {
    for file in &files.packages {
        if file.kind == "pub" && pubspec_looks_like_flutter(root, &file.path)? {
            ecosystems.insert("flutter".to_owned());
            tools.insert("flutter".to_owned());
            roles.insert("app".to_owned());
        }
        if file.kind == "npm" {
            enrich_package_json_roles(root, &file.path, ecosystems, tools, roles)?;
        }
    }
    if files.packages.len() > 1 {
        roles.insert("monorepo".to_owned());
    }
    if !files.deploy.is_empty() {
        roles.insert("deploy".to_owned());
    }
    if !files.infra.is_empty() {
        roles.insert("infra".to_owned());
    }
    if !files.quality.is_empty() {
        roles.insert("quality".to_owned());
    }
    if !files.security.is_empty() {
        roles.insert("security".to_owned());
    }
    Ok(())
}

pub(super) fn enrich_package_json_roles(
    root: &Path,
    rel: &str,
    ecosystems: &mut BTreeSet<String>,
    tools: &mut BTreeSet<String>,
    roles: &mut BTreeSet<String>,
) -> Result<(), AppError> {
    let path = root.join(rel);
    let raw = io::read_to_string(&path)?;
    let value = serde_json::from_str::<Value>(&raw)
        .map_err(|source| AppError::json_deserialization(path, source))?;
    for dep in package_json_dependency_names(&value) {
        match dep.as_str() {
            "next" | "vite" | "astro" | "react" | "vue" | "svelte" => {
                roles.insert("web".to_owned());
                tools.insert(dep);
            }
            "docusaurus" | "@docusaurus/core" => {
                roles.insert("docs".to_owned());
                tools.insert("docusaurus".to_owned());
            }
            "express" | "fastify" | "@nestjs/core" | "nestjs" => {
                roles.insert("backend".to_owned());
                tools.insert(dep);
            }
            "react-native" | "expo" => {
                ecosystems.insert("mobile".to_owned());
                roles.insert("mobile".to_owned());
                tools.insert(dep);
            }
            "electron" => {
                ecosystems.insert("electron".to_owned());
                roles.insert("desktop".to_owned());
                tools.insert(dep);
            }
            "@tauri-apps/cli" | "@tauri-apps/api" => {
                ecosystems.insert("tauri".to_owned());
                roles.insert("desktop".to_owned());
                tools.insert("tauri".to_owned());
            }
            "playwright" | "@playwright/test" | "cypress" | "vitest" | "jest" | "eslint"
            | "prettier" => {
                roles.insert("quality".to_owned());
                tools.insert(dep);
            }
            "semgrep" => {
                roles.insert("security".to_owned());
                tools.insert(dep);
            }
            _ => {}
        }
    }
    Ok(())
}

pub(super) fn package_json_dependency_names(value: &Value) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    for section in [
        "dependencies",
        "devDependencies",
        "peerDependencies",
        "optionalDependencies",
    ] {
        if let Some(map) = value.get(section).and_then(Value::as_object) {
            names.extend(map.keys().cloned());
        }
    }
    names
}

pub(super) fn pubspec_looks_like_flutter(root: &Path, rel: &str) -> Result<bool, AppError> {
    let path = root.join(rel);
    let raw = io::read_to_string(&path)?;
    Ok(raw.lines().any(|line| {
        let trimmed = line.trim();
        trimmed == "flutter:" || trimmed.contains("sdk: flutter")
    }))
}

pub(super) fn detected(kind: &str, path: &str) -> DetectedFile {
    DetectedFile {
        kind: kind.to_owned(),
        path: path.to_owned(),
    }
}

pub(super) fn normalize_relative(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .map(core::normalize_path)
        .unwrap_or_else(|_| core::normalize_path(path))
}
