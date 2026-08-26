//! `ah project detect`, `project commands` and `project version`.
//!
//! The 1181 production lines this came from ran recognition, suggestion and
//! version parsing together, with a 438-line ecosystem table in the middle of
//! them:
//!
//! | Module     | Owns                                                      |
//! |------------|-----------------------------------------------------------|
//! | `output`   | what the three commands report                            |
//! | `detect`   | which manifests are present and what they imply           |
//! | `suggest`  | the per-ecosystem command table                           |
//! | `version`  | one version parser per manifest format                    |

use super::io;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

use regex::Regex;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;

use ah_runtime::core;

use crate::error::AppError;

use super::{
    ProjectPathArgs,
    rules::{FileGroup, classify_file},
};

mod detect;
mod output;
mod suggest;
mod version;
use detect::{detect_project, normalize_relative};
use output::ProjectSnapshot;
pub(crate) use output::{
    DetectedFile, ProjectCommandsOutput, ProjectDetectOutput, ProjectFileGroups,
    ProjectVersionEntry, ProjectVersionOutput, SuggestedCommand,
};
use suggest::suggest_commands;
use version::detect_versions;

pub(crate) fn run_detect(args: ProjectPathArgs) -> Result<ProjectDetectOutput, AppError> {
    let snapshot = detect_project(&args.path)?;

    Ok(ProjectDetectOutput {
        command: "project.detect",
        root: snapshot.root,
        ecosystems: snapshot.ecosystems,
        tools: snapshot.tools,
        roles: snapshot.roles,
        package_files: snapshot.files.packages.clone(),
        ci_files: snapshot.files.ci.clone(),
        docs_files: snapshot.files.docs.clone(),
        changelog_files: snapshot.files.changelogs.clone(),
        files: snapshot.files,
        versions: snapshot.versions,
        commands: snapshot.commands,
    })
}

pub(crate) fn run_commands(args: ProjectPathArgs) -> Result<ProjectCommandsOutput, AppError> {
    let snapshot = detect_project(&args.path)?;

    Ok(ProjectCommandsOutput {
        command: "project.commands",
        root: snapshot.root,
        ecosystems: snapshot.ecosystems,
        tools: snapshot.tools,
        roles: snapshot.roles,
        commands: snapshot.commands,
    })
}

pub(crate) fn run_version(
    args: ProjectPathArgs,
    limit: Option<usize>,
) -> Result<ProjectVersionOutput, AppError> {
    let snapshot = detect_project(&args.path)?;
    let mut versions = snapshot.versions;
    let truncated = if let Some(limit) = limit {
        if versions.len() > limit {
            versions.truncate(limit);
            true
        } else {
            false
        }
    } else {
        false
    };
    Ok(ProjectVersionOutput {
        command: "project.version",
        root: snapshot.root,
        version_count: versions.len(),
        truncated,
        versions,
    })
}
