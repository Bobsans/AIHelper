//! Golden snapshots of the artifacts the refactoring program must not change
//! by accident: the typed command catalog, the plugin manuals, the rendered CLI
//! help tree, and the error-code table.
//!
//! These tests exist to make structural refactors provably behavior-preserving.
//! A failing snapshot is not automatically a bug — it is a diff that a human has
//! to look at and either accept or fix. Regenerate with:
//!
//! ```text
//! AH_UPDATE_SNAPSHOTS=1 cargo test --lib snapshots
//! ```
//!
//! and review the resulting `tests/snapshots/*.snap` diff in the pull request.

use std::{
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use ah_runtime::PluginManager;
use serde_json::{Value, json};

use crate::{
    plugin_settings::PluginSettings,
    secrets::{KeyProvider, VaultError, VaultStore},
};

const UPDATE_ENV: &str = "AH_UPDATE_SNAPSHOTS";
const HELP_TERM_WIDTH: usize = 100;

fn snapshot_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("snapshots")
        .join(format!("{name}.snap"))
}

/// Compare `actual` against the checked-in snapshot, or rewrite the snapshot
/// when `AH_UPDATE_SNAPSHOTS` is set.
#[track_caller]
fn assert_snapshot(name: &str, actual: &str) {
    let path = snapshot_path(name);
    let actual = normalize(actual);

    if std::env::var_os(UPDATE_ENV).is_some() {
        let parent = path.parent().expect("snapshot path should have a parent");
        fs::create_dir_all(parent).expect("snapshot directory should be creatable");
        fs::write(&path, actual.as_bytes()).expect("snapshot should be writable");
        return;
    }

    let expected = fs::read_to_string(&path).map(|raw| normalize(&raw)).unwrap_or_else(|error| {
        panic!(
            "missing snapshot '{}' ({error}); create it with `{UPDATE_ENV}=1 cargo test --lib snapshots`",
            path.display()
        )
    });

    if expected != actual {
        panic!(
            "snapshot '{}' does not match.\n{}\n\nAccept the change with `{UPDATE_ENV}=1 cargo test --lib snapshots` \
             and review the diff, or fix the regression.",
            path.display(),
            first_difference(&expected, &actual)
        );
    }
}

/// Snapshots are compared line-wise with LF endings and a single trailing
/// newline, so a checkout with `core.autocrlf` cannot fail them.
fn normalize(value: &str) -> String {
    let mut normalized = value.replace("\r\n", "\n");
    while normalized.ends_with('\n') {
        normalized.pop();
    }
    normalized.push('\n');
    normalized
}

fn first_difference(expected: &str, actual: &str) -> String {
    let expected_lines = expected.lines().collect::<Vec<_>>();
    let actual_lines = actual.lines().collect::<Vec<_>>();
    for (index, (expected_line, actual_line)) in
        expected_lines.iter().zip(actual_lines.iter()).enumerate()
    {
        if expected_line != actual_line {
            return format!(
                "first difference at line {}:\n  expected: {expected_line}\n  actual:   {actual_line}",
                index + 1
            );
        }
    }
    format!(
        "line counts differ: expected {} lines, got {}",
        expected_lines.len(),
        actual_lines.len()
    )
}

struct StubKeyProvider;

impl KeyProvider for StubKeyProvider {
    fn load_or_create(&self) -> Result<[u8; 32], VaultError> {
        Ok([0_u8; 32])
    }
}

/// The same registration sequence `runtime_flow` performs for `ah mcp serve`,
/// minus dynamic plugin discovery so the result does not depend on what happens
/// to sit next to the test binary.
fn snapshot_manager() -> Arc<PluginManager> {
    let missing = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("snapshot-nonexistent");
    let settings = Arc::new(Mutex::new(
        PluginSettings::load_from_path(missing.join("plugins.json"))
            .expect("absent plugin settings should load as defaults"),
    ));
    let vault = Arc::new(VaultStore::at(&missing, Arc::new(StubKeyProvider)));

    Arc::new_cyclic(|weak| {
        let mut manager = PluginManager::new();
        manager.reserve_dynamic_domains(["ai", "plugins", "mcp", "secrets", "upgrade"]);
        for plugin in crate::plugins::builtins() {
            manager.register_builtin(plugin);
        }
        for plugin in
            crate::host_commands::builtins(weak.clone(), Arc::clone(&settings), Arc::clone(&vault))
        {
            manager.register_host_builtin(plugin);
        }
        manager
    })
}

fn pretty(value: &Value) -> String {
    serde_json::to_string_pretty(value).expect("snapshot payload should serialize")
}

#[test]
fn typed_command_catalog_is_stable() {
    let manager = snapshot_manager();
    let commands = manager
        .list_enabled_commands()
        .expect("the built-in catalog should validate");

    let payload = commands
        .into_iter()
        .map(|command| {
            let descriptor =
                serde_json::to_value(&command.descriptor).expect("descriptor should serialize");
            json!({
                "domain": command.plugin.domain,
                "plugin": command.plugin.plugin_name,
                "descriptor": descriptor,
            })
        })
        .collect::<Vec<_>>();

    assert_snapshot("typed-command-catalog", &pretty(&Value::Array(payload)));
}

#[test]
fn plugin_manuals_are_stable() {
    let manager = snapshot_manager();
    let manuals =
        serde_json::to_value(manager.collect_plugin_manuals()).expect("manuals should serialize");
    assert_snapshot("plugin-manuals", &pretty(&manuals));
}

#[test]
fn cli_help_tree_is_stable() {
    let manager = snapshot_manager();
    let mut command =
        crate::cli::build_cli_command(&manager.list_enabled_plugins()).term_width(HELP_TERM_WIDTH);

    let mut rendered = String::new();
    render_help_tree(&mut command, "ah", &mut rendered);
    assert_snapshot("cli-help", &rendered);
}

fn render_help_tree(command: &mut clap::Command, path: &str, out: &mut String) {
    out.push_str(&format!("$ {path} --help\n"));
    out.push_str(command.render_help().to_string().trim_end());
    out.push_str("\n\n");

    let mut names = command
        .get_subcommands()
        .map(|sub| sub.get_name().to_owned())
        .collect::<Vec<_>>();
    names.sort();
    for name in names {
        let child_path = format!("{path} {name}");
        let child = command
            .find_subcommand_mut(&name)
            .expect("listed subcommand should be resolvable");
        render_help_tree(child, &child_path, out);
    }
}

/// One fixture per dispatch arm of `symbols::extract_symbols`, covering
/// every extraction pattern the module defines.
///
/// The corpus was checked mechanically: each of the 61 regexes in that module
/// matches at least one line here. That matters because the module had no unit
/// tests of its own and the integration tests assert only that a handful of
/// expected symbols are *present*, never that the whole extraction is
/// unchanged.
pub(crate) const SYMBOL_FIXTURES: &[(&str, &str)] = &[
    (
        "lib.rs",
        "pub async fn build_index(root: &Path) -> Result<()> {\nstruct Config {\npub enum Mode {\ntrait Render {\nimpl<T: Clone> Render for Config {\npub mod helpers {\n",
    ),
    (
        "README.md",
        "# Title\n## Section\n### Sub section\n#### Deep\n",
    ),
    (
        "app.py",
        "class Service:\n    async def handle(self):\n    def plain(self):\n",
    ),
    (
        "app.ts",
        "export class Widget {\nexport async function render() {\nexport interface Props {\nexport type Alias = string;\nexport const build = async (\n",
    ),
    ("main.go", "func Serve() {\ntype Server struct {\n"),
    (
        "Main.java",
        "package com.example.demo;\npublic sealed interface Service {\npublic class App {\n    public static String render(int value) {\n",
    ),
    (
        "App.kt",
        "package com.example.demo\ndata class Person(val name: String)\nprivate fun boot() {\n",
    ),
    (
        "App.scala",
        "package com.example\ncase class Item(name: String)\noverride def run(): Unit = {\n",
    ),
    (
        "Program.cs",
        "namespace Demo.App;\npublic record Item(string Name);\npublic static string Render(int value) {\n",
    ),
    (
        "lib.php",
        "namespace App\\Domain;\nabstract class Handler {\nfunction handle() {\n",
    ),
    ("worker.rb", "module Demo\nclass Worker\n  def perform!\n"),
    (
        "app.ex",
        "defmodule Demo.Worker do\n  def perform do\n  defp helper? do\n  defmacro guarded do\n",
    ),
    ("app.erl", "-module(demo_worker).\nhandle(Request) ->\n"),
    (
        "App.swift",
        "public struct Model {\ninternal actor Store {\nprivate func reload() {\n",
    ),
    (
        "main.dart",
        "class Widget {\nmixin Logging {\nvoid render(\n",
    ),
    (
        "main.cpp",
        "namespace demo::core {\nclass Engine {\nstruct Point {\nint compute(int value) {\n",
    ),
    (
        "main.zig",
        "pub fn main() void {\nconst Config = struct {\n",
    ),
    (
        "init.lua",
        "local function setup()\nfunction M.teardown()\n",
    ),
    ("Module.pm", "package Demo::Module;\nsub render {\n"),
    (
        "analysis.r",
        "summarise <- function(data) {\nplot.data = function(x) {\n",
    ),
    (
        "model.jl",
        "struct Point\nmutable struct Buffer\nfunction solve(x)\n",
    ),
    (
        "Lib.hs",
        "module Demo.Lib where\ndata Shape = Circle\nnewtype Wrapper = Wrapper Int\nclass Render a where\nrender :: Shape -> String\n",
    ),
    (
        "lib.ml",
        "module Store = struct\ntype shape = Circle\nlet rec walk node =\n",
    ),
    (
        "main.tf",
        "resource \"aws_s3_bucket\" \"logs\" {\nmodule \"network\" {\nvariable \"region\" {\n",
    ),
    ("config.yml", "service:\nimage_name:\n"),
    ("Cargo.toml", "[package]\n[[bin]]\n[dependencies.serde]\n"),
    ("deploy.sh", "function build() {\nteardown() {\n"),
    ("Module.psm1", "function Invoke-Build {\n"),
    (
        "Dockerfile",
        "FROM rust:1.88 AS builder\nFROM debian:bookworm\n",
    ),
    ("Makefile", "build:\ntest-all:\n"),
    (
        "notes.txt",
        "class Loose\ninterface Loose\nfn loose\ndef loose\n",
    ),
];

#[test]
fn symbol_extraction_is_stable() {
    let payload = SYMBOL_FIXTURES
        .iter()
        .map(|(name, content)| {
            let symbols = crate::commands::ctx::symbols::extract_symbols(Path::new(name), content);
            json!({
                "file": name,
                "symbols": serde_json::to_value(&symbols)
                    .expect("symbols should serialize"),
            })
        })
        .collect::<Vec<_>>();

    assert_snapshot("symbol-extraction", &pretty(&Value::Array(payload)));
}

/// Every literal the rule table compares against, turned into a path.
///
/// The corpus is derived from the rules rather than guessed: each exact name,
/// suffix, name prefix, path, path prefix and path fragment appears here, plus
/// the compound cases that need the name and the location to agree. `rules.rs`
/// has no other unit tests and the integration tests assert only that a few
/// ecosystems and tools show up, so this is what pins the classification.
pub(crate) const PROJECT_RULE_FIXTURES: &[(&str, &str)] = &[
    (".drone.yaml", ".drone.yaml"),
    (".drone.yml", ".drone.yml"),
    (".eslintrc", ".eslintrc"),
    (".eslintrc.json", ".eslintrc.json"),
    (".gitlab-ci.yaml", ".gitlab-ci.yaml"),
    (".gitlab-ci.yml", ".gitlab-ci.yml"),
    (".pre-commit-config.yaml", ".pre-commit-config.yaml"),
    (".pre-commit-config.yml", ".pre-commit-config.yml"),
    (".prettierrc", ".prettierrc"),
    (".prettierrc.json", ".prettierrc.json"),
    (".renovaterc", ".renovaterc"),
    (".renovaterc.json", ".renovaterc.json"),
    (".rubocop.yml", ".rubocop.yml"),
    (".ruff.toml", ".ruff.toml"),
    (".semgrep.yaml", ".semgrep.yaml"),
    (".semgrep.yml", ".semgrep.yml"),
    (".trivyignore", ".trivyignore"),
    (".woodpecker.yaml", ".woodpecker.yaml"),
    (".woodpecker.yml", ".woodpecker.yml"),
    ("androidmanifest.xml", "androidmanifest.xml"),
    ("appfile", "appfile"),
    ("application.properties", "application.properties"),
    ("application.yaml", "application.yaml"),
    ("application.yml", "application.yml"),
    ("artisan", "artisan"),
    ("astro.config.js", "astro.config.js"),
    ("astro.config.mjs", "astro.config.mjs"),
    ("astro.config.ts", "astro.config.ts"),
    ("azure-pipelines.yaml", "azure-pipelines.yaml"),
    ("azure-pipelines.yml", "azure-pipelines.yml"),
    ("build.gradle", "build.gradle"),
    ("build.gradle.kts", "build.gradle.kts"),
    ("build.pl", "build.pl"),
    ("build.sbt", "build.sbt"),
    ("build.zig", "build.zig"),
    ("bun.lock", "bun.lock"),
    ("bun.lockb", "bun.lockb"),
    ("cabal.project", "cabal.project"),
    ("cargo.lock", "cargo.lock"),
    ("cargo.toml", "cargo.toml"),
    ("cartfile", "cartfile"),
    ("cdk.json", "cdk.json"),
    ("changelog.md", "changelog.md"),
    ("changes.md", "changes.md"),
    ("chart.yaml", "chart.yaml"),
    ("cmakelists.txt", "cmakelists.txt"),
    ("compose.yaml", "compose.yaml"),
    ("compose.yml", "compose.yml"),
    ("composer.json", "composer.json"),
    ("composer.lock", "composer.lock"),
    ("conanfile.py", "conanfile.py"),
    ("conanfile.txt", "conanfile.txt"),
    ("conf.py", "conf.py"),
    ("config.toml", "config.toml"),
    ("config.yaml", "config.yaml"),
    ("cpanfile", "cpanfile"),
    ("dependabot.yaml", "dependabot.yaml"),
    ("dependabot.yml", "dependabot.yml"),
    ("deps.edn", "deps.edn"),
    ("description", "description"),
    (
        "docker-compose.override.yaml",
        "docker-compose.override.yaml",
    ),
    ("docker-compose.override.yml", "docker-compose.override.yml"),
    ("docker-compose.yaml", "docker-compose.yaml"),
    ("docker-compose.yml", "docker-compose.yml"),
    ("dockerfile", "dockerfile"),
    ("docusaurus.config.js", "docusaurus.config.js"),
    ("docusaurus.config.ts", "docusaurus.config.ts"),
    ("dune-project", "dune-project"),
    ("eslint.config.js", "eslint.config.js"),
    ("eslint.config.mjs", "eslint.config.mjs"),
    ("eslint.config.ts", "eslint.config.ts"),
    ("fastfile", "fastfile"),
    ("flake.nix", "flake.nix"),
    ("gemfile", "gemfile"),
    ("gemfile.lock", "gemfile.lock"),
    ("go.mod", "go.mod"),
    ("go.sum", "go.sum"),
    ("history.md", "history.md"),
    ("hugo.toml", "hugo.toml"),
    ("hugo.yaml", "hugo.yaml"),
    ("jenkinsfile", "jenkinsfile"),
    ("kustomization.yaml", "kustomization.yaml"),
    ("kustomization.yml", "kustomization.yml"),
    ("lefthook.yaml", "lefthook.yaml"),
    ("lefthook.yml", "lefthook.yml"),
    ("makefile", "makefile"),
    ("makefile.pl", "makefile.pl"),
    ("manage.py", "manage.py"),
    ("manifest.toml", "manifest.toml"),
    ("meson.build", "meson.build"),
    ("meta.json", "meta.json"),
    ("meta.yml", "meta.yml"),
    ("meta6.json", "meta6.json"),
    ("mix.exs", "mix.exs"),
    ("mix.lock", "mix.lock"),
    ("mkdocs.yaml", "mkdocs.yaml"),
    ("mkdocs.yml", "mkdocs.yml"),
    ("module.bazel", "module.bazel"),
    ("next.config.js", "next.config.js"),
    ("next.config.mjs", "next.config.mjs"),
    ("next.config.ts", "next.config.ts"),
    ("package-lock.json", "package-lock.json"),
    ("package.json", "package.json"),
    ("package.swift", "package.swift"),
    ("packages.lock.json", "packages.lock.json"),
    ("phoenix", "phoenix"),
    ("phpstan.neon", "phpstan.neon"),
    ("platformio.ini", "platformio.ini"),
    ("playbook.yaml", "playbook.yaml"),
    ("playbook.yml", "playbook.yml"),
    ("pnpm-lock.yaml", "pnpm-lock.yaml"),
    ("podfile", "podfile"),
    ("poetry.lock", "poetry.lock"),
    ("pom.xml", "pom.xml"),
    ("prettier.config.js", "prettier.config.js"),
    ("prettier.config.mjs", "prettier.config.mjs"),
    ("project.clj", "project.clj"),
    ("project.toml", "project.toml"),
    ("projectversion.txt", "projectversion.txt"),
    ("psalm.xml", "psalm.xml"),
    ("pubspec.lock", "pubspec.lock"),
    ("pubspec.yaml", "pubspec.yaml"),
    ("pulumi.yaml", "pulumi.yaml"),
    ("pyproject.toml", "pyproject.toml"),
    ("readme", "readme"),
    ("readme.md", "readme.md"),
    ("rebar.config", "rebar.config"),
    ("renovate.json", "renovate.json"),
    ("renv.lock", "renv.lock"),
    ("rockspec", "rockspec"),
    ("rubocop.yml", "rubocop.yml"),
    ("ruff.toml", "ruff.toml"),
    ("semgrep.yaml", "semgrep.yaml"),
    ("semgrep.yml", "semgrep.yml"),
    ("serverless.yaml", "serverless.yaml"),
    ("serverless.yml", "serverless.yml"),
    ("sfdx-project.json", "sfdx-project.json"),
    ("shard.lock", "shard.lock"),
    ("shard.yml", "shard.yml"),
    ("shell.nix", "shell.nix"),
    ("site.yaml", "site.yaml"),
    ("site.yml", "site.yml"),
    ("skaffold.yaml", "skaffold.yaml"),
    ("skaffold.yml", "skaffold.yml"),
    ("sketch.yaml", "sketch.yaml"),
    ("stack.yaml", "stack.yaml"),
    ("template.yaml", "template.yaml"),
    ("template.yml", "template.yml"),
    ("tiltfile", "tiltfile"),
    ("trivy.yaml", "trivy.yaml"),
    ("trivy.yml", "trivy.yml"),
    ("tsconfig.json", "tsconfig.json"),
    ("uv.lock", "uv.lock"),
    ("vcpkg.json", "vcpkg.json"),
    ("vite.config.js", "vite.config.js"),
    ("vite.config.mjs", "vite.config.mjs"),
    ("vite.config.ts", "vite.config.ts"),
    ("workspace", "workspace"),
    ("workspace.bazel", "workspace.bazel"),
    ("yarn.lock", "yarn.lock"),
    ("sample.cabal", "sample.cabal"),
    ("sample.csproj", "sample.csproj"),
    ("sample.gemspec", "sample.gemspec"),
    ("sample.gradle", "sample.gradle"),
    ("sample.gradle.kts", "sample.gradle.kts"),
    ("sample.ipynb", "sample.ipynb"),
    ("sample.nomad", "sample.nomad"),
    ("sample.opam", "sample.opam"),
    ("sample.rockspec", "sample.rockspec"),
    ("sample.tf", "sample.tf"),
    ("sample.tfvars", "sample.tfvars"),
    ("sample.tofu", "sample.tofu"),
    ("sample.uplugin", "sample.uplugin"),
    ("sample.uproject", "sample.uproject"),
    ("sample.xcodeproj", "sample.xcodeproj"),
    ("sample.xcworkspace", "sample.xcworkspace"),
    ("dockerfile.dev", "dockerfile.dev"),
    (".circleci/config.yaml", "config.yaml"),
    (".circleci/config.yml", "config.yml"),
    ("settings.gradle", "settings.gradle"),
    ("settings.gradle.kts", "settings.gradle.kts"),
    (".buildkite/entry.yml", "entry.yml"),
    (".fluxcd/entry.yml", "entry.yml"),
    (".github/workflows/entry.yml", "entry.yml"),
    ("android/entry.yml", "entry.yml"),
    ("ios/entry.yml", "entry.yml"),
    ("projectsettings/entry.yml", "entry.yml"),
    ("roles/entry.yml", "entry.yml"),
    ("src-tauri/entry.yml", "entry.yml"),
    ("nested/roles/entry.yml", "entry.yml"),
    ("nested/android/entry.yml", "entry.yml"),
    ("nested/argo-cd/entry.yml", "entry.yml"),
    ("nested/argocd/entry.yml", "entry.yml"),
    ("nested/codeql/entry.yml", "entry.yml"),
    ("nested/doc/entry.yml", "entry.yml"),
    ("nested/docs/entry.yml", "entry.yml"),
    ("nested/gotk-components/entry.yml", "entry.yml"),
    ("docs/conf.py", "conf.py"),
    ("doc/conf.py", "conf.py"),
    (".github/workflows/codeql.yml", "codeql.yml"),
    ("android/app/build.gradle", "build.gradle"),
    ("app/build.gradle.kts", "build.gradle.kts"),
    ("projectsettings/projectversion.txt", "projectversion.txt"),
    ("src-tauri/tauri.conf.json", "tauri.conf.json"),
    ("ios/Runner/Info.plist", "Info.plist"),
    ("roles/web/tasks/main.yml", "main.yml"),
    ("nested/roles/web/tasks/main.yml", "main.yml"),
    ("unclassified/random.txt", "random.txt"),
];

#[test]
fn project_file_classification_is_stable() {
    let payload = PROJECT_RULE_FIXTURES
        .iter()
        .map(|(rel, name)| {
            let detections = crate::commands::project::rules::classify_file(rel, name)
                .into_iter()
                .map(|detection| {
                    json!({
                        "group": format!("{:?}", detection.group),
                        "kind": detection.kind,
                        "ecosystem": detection.ecosystem,
                        "tool": detection.tool,
                        "role": detection.role,
                    })
                })
                .collect::<Vec<_>>();
            json!({ "path": rel, "detections": detections })
        })
        .collect::<Vec<_>>();

    assert_snapshot("project-file-rules", &pretty(&Value::Array(payload)));
}

#[test]
fn error_codes_are_stable() {
    use ah_runtime::RuntimeError;
    use std::path::PathBuf as StdPathBuf;

    let runtime_errors: Vec<RuntimeError> = vec![
        RuntimeError::DomainNotFound("nope".to_owned()),
        RuntimeError::AbiVersionMismatch {
            path: StdPathBuf::from("plugin.dll"),
            found: 2,
            expected: 1,
        },
        RuntimeError::ApiVersionMismatch {
            path: StdPathBuf::from("plugin.dll"),
            found_major: 9,
            found_minor: 1,
            supported_major: 1,
            supported_minor: 0,
        },
        RuntimeError::InvalidMetadata {
            path: StdPathBuf::from("plugin.dll"),
            reason: "empty domain".to_owned(),
        },
        RuntimeError::Invocation("boom".to_owned()),
        RuntimeError::ResponseParse("bad json".to_owned()),
        RuntimeError::InvalidCommandCatalog {
            domain: "probe".to_owned(),
            reason: "duplicate id".to_owned(),
        },
        RuntimeError::TypedCommandNotFound("probe.missing".to_owned()),
        RuntimeError::TypedInvocation("bad arguments".to_owned()),
        RuntimeError::SecretRequired {
            command: "http.get".to_owned(),
            slot: "basic".to_owned(),
        },
        RuntimeError::SecretNotFound {
            command: "http.get".to_owned(),
            slot: "basic".to_owned(),
            id: "private-id".to_owned(),
        },
        RuntimeError::SecretKindMismatch {
            command: "http.get".to_owned(),
            slot: "basic".to_owned(),
            id: "private-id".to_owned(),
            kind: "private-kind".to_owned(),
            accepted_kinds: vec!["private-accepted".to_owned()],
        },
        RuntimeError::VaultLocked {
            command: "http.get".to_owned(),
            slot: "basic".to_owned(),
            id: "private-id".to_owned(),
        },
        RuntimeError::VaultKeyUnavailable {
            command: "http.get".to_owned(),
            slot: "basic".to_owned(),
            id: "private-id".to_owned(),
        },
        RuntimeError::TypedResponseValidation {
            command: "probe.run".to_owned(),
            reason: "missing field".to_owned(),
        },
        RuntimeError::InvalidExecutionRequest("empty command".to_owned()),
        RuntimeError::ExecutionCapacityFull { capacity: 4 },
        RuntimeError::ExecutionCancelled {
            request_id: "req-1".to_owned(),
        },
        RuntimeError::ExecutionTimeout {
            request_id: "req-1".to_owned(),
        },
        RuntimeError::ExecutorShuttingDown,
        RuntimeError::ExecutionWorker("worker gone".to_owned()),
        RuntimeError::ExecutionPanic {
            request_id: "req-1".to_owned(),
        },
        RuntimeError::DomainDisabled("git".to_owned()),
        RuntimeError::DependencyMissing {
            domain: "git".to_owned(),
            operation: Some("git.status".to_owned()),
            tool: "git".to_owned(),
            reason: "not on PATH".to_owned(),
        },
    ];

    let mut rows = runtime_errors
        .into_iter()
        .map(|error| error_row("RuntimeError", crate::map_runtime_error(error)))
        .collect::<Vec<_>>();

    let io_error = || std::io::Error::from(std::io::ErrorKind::NotFound);
    let app_errors = vec![
        crate::error::AppError::invalid_argument("--limit must be >= 1"),
        crate::error::AppError::external("PROBE_FAILED", "probe failed"),
        crate::error::AppError::unknown_command("serach", None),
        crate::error::AppError::cwd(StdPathBuf::from("missing"), io_error()),
        crate::error::AppError::file_read(StdPathBuf::from("missing.txt"), io_error()),
        crate::error::AppError::file_write(StdPathBuf::from("missing.txt"), io_error()),
        crate::error::AppError::file_metadata(StdPathBuf::from("missing.txt"), io_error()),
        crate::error::AppError::directory_read(StdPathBuf::from("missing"), io_error()),
        crate::error::AppError::command_execution("git status", io_error()),
        crate::error::AppError::command_failed("git status", Some(128), "not a repository"),
    ];
    rows.extend(
        app_errors
            .into_iter()
            .map(|error| error_row("AppError", error)),
    );

    assert_snapshot("error-codes", &pretty(&Value::Array(rows)));
}

fn error_row(origin: &str, error: crate::error::AppError) -> Value {
    json!({
        "origin": origin,
        "code": error.code(),
        "user_message": error.user_message(),
        "detail_message": error.detail_message(),
    })
}
