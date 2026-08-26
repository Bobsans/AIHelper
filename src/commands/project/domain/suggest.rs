//! The suggested commands per ecosystem.
//!
//! This is a table written as code: one branch per ecosystem, each pushing
//! `(kind, argv, confidence, reason)`. Turning it into an actual table would
//! shorten it, but that is a change of representation, not a move.

use super::*;

pub(super) fn node_package_manager(tools: &[&str]) -> &'static str {
    if tools.contains(&"bun") {
        "bun"
    } else if tools.contains(&"pnpm") {
        "pnpm"
    } else if tools.contains(&"yarn") {
        "yarn"
    } else {
        "npm"
    }
}

pub(super) fn node_install_command(package_manager: &str) -> Vec<&'static str> {
    match package_manager {
        "bun" => vec!["bun", "install"],
        "pnpm" => vec!["pnpm", "install"],
        "yarn" => vec!["yarn", "install"],
        _ => vec!["npm", "install"],
    }
}

pub(super) fn add_node_script_commands(
    root: &Path,
    files: &ProjectFileGroups,
    package_manager: &str,
    commands: &mut Vec<SuggestedCommand>,
) -> Result<(), AppError> {
    let mut scripts = BTreeSet::new();
    for file in &files.packages {
        if file.kind != "npm" {
            continue;
        }
        for script in read_package_json_scripts(root, &file.path)?.keys() {
            scripts.insert(script.clone());
        }
    }

    for script in ["test", "build", "lint", "format"] {
        if !scripts.contains(script) {
            continue;
        }
        let command = node_script_command(package_manager, script);
        commands.push(suggested(
            script,
            command.as_slice(),
            "medium",
            "package.json script detected",
        ));
    }
    if scripts.is_empty() {
        commands.push(suggested(
            "test",
            node_script_command(package_manager, "test").as_slice(),
            "low",
            "package.json detected",
        ));
        commands.push(suggested(
            "build",
            node_script_command(package_manager, "build").as_slice(),
            "low",
            "package.json detected",
        ));
    }
    Ok(())
}

pub(super) fn read_package_json_scripts(
    root: &Path,
    rel: &str,
) -> Result<BTreeMap<String, String>, AppError> {
    let path = root.join(rel);
    let raw = io::read_to_string(&path)?;
    let value = serde_json::from_str::<Value>(&raw)
        .map_err(|source| AppError::json_deserialization(path, source))?;
    let mut scripts = BTreeMap::new();
    if let Some(map) = value.get("scripts").and_then(Value::as_object) {
        for (key, value) in map {
            if let Some(command) = value.as_str() {
                scripts.insert(key.clone(), command.to_owned());
            }
        }
    }
    Ok(scripts)
}

pub(super) fn node_script_command(package_manager: &str, script: &str) -> Vec<&'static str> {
    match (package_manager, script) {
        ("npm", "test") => vec!["npm", "test"],
        ("npm", "build") => vec!["npm", "run", "build"],
        ("npm", "lint") => vec!["npm", "run", "lint"],
        ("npm", "format") => vec!["npm", "run", "format"],
        ("pnpm", "test") => vec!["pnpm", "test"],
        ("pnpm", "build") => vec!["pnpm", "build"],
        ("pnpm", "lint") => vec!["pnpm", "lint"],
        ("pnpm", "format") => vec!["pnpm", "format"],
        ("yarn", "test") => vec!["yarn", "test"],
        ("yarn", "build") => vec!["yarn", "build"],
        ("yarn", "lint") => vec!["yarn", "lint"],
        ("yarn", "format") => vec!["yarn", "format"],
        ("bun", "test") => vec!["bun", "test"],
        ("bun", "build") => vec!["bun", "run", "build"],
        ("bun", "lint") => vec!["bun", "run", "lint"],
        ("bun", "format") => vec!["bun", "run", "format"],
        _ => vec!["npm", "run", "test"],
    }
}

pub(super) fn python_runner(tools: &[&str]) -> Vec<&'static str> {
    if tools.contains(&"uv") {
        vec!["uv", "run", "pytest"]
    } else if tools.contains(&"poetry") {
        vec!["poetry", "run", "pytest"]
    } else {
        vec!["pytest"]
    }
}

pub(super) fn deduplicate_commands(commands: Vec<SuggestedCommand>) -> Vec<SuggestedCommand> {
    let mut seen = BTreeSet::new();
    let mut result = Vec::new();
    for command in commands {
        let key = format!("{}:{}", command.kind, command.command.join(" "));
        if seen.insert(key) {
            result.push(command);
        }
    }
    result
}

pub(super) fn suggest_commands(
    ecosystems: &[String],
    tools: &[String],
    files: &ProjectFileGroups,
    root: &Path,
) -> Result<Vec<SuggestedCommand>, AppError> {
    let ecosystems = ecosystems.iter().map(String::as_str).collect::<Vec<_>>();
    let tools = tools.iter().map(String::as_str).collect::<Vec<_>>();
    let mut commands = Vec::new();
    if ecosystems.contains(&"rust") {
        commands.push(suggested(
            "format_check",
            &["cargo", "fmt", "--all", "--", "--check"],
            "high",
            "Cargo.toml detected",
        ));
        commands.push(suggested(
            "test",
            &["cargo", "test", "--workspace", "--all-targets", "--locked"],
            "high",
            "Cargo.toml detected",
        ));
        commands.push(suggested(
            "build",
            &["cargo", "build", "--locked"],
            "high",
            "Cargo.toml detected",
        ));
        commands.push(suggested(
            "release_build",
            &["cargo", "build", "--release", "--locked"],
            "high",
            "Cargo.toml detected",
        ));
    }
    if ecosystems.contains(&"node") {
        let package_manager = node_package_manager(&tools);
        commands.push(suggested(
            "install",
            node_install_command(package_manager).as_slice(),
            "medium",
            "Node manifest detected",
        ));
        add_node_script_commands(root, files, package_manager, &mut commands)?;
    }
    if ecosystems.contains(&"dotnet") {
        commands.push(suggested(
            "restore",
            &["dotnet", "restore"],
            "medium",
            ".csproj detected",
        ));
        commands.push(suggested(
            "test",
            &["dotnet", "test"],
            "medium",
            ".csproj detected",
        ));
        commands.push(suggested(
            "build",
            &["dotnet", "build"],
            "medium",
            ".csproj detected",
        ));
    }
    if ecosystems.contains(&"python") {
        let runner = python_runner(&tools);
        commands.push(suggested(
            "test",
            runner.as_slice(),
            "low",
            "pyproject.toml detected",
        ));
    }
    if ecosystems.contains(&"go") {
        commands.push(suggested(
            "test",
            &["go", "test", "./..."],
            "high",
            "go.mod detected",
        ));
        commands.push(suggested(
            "build",
            &["go", "build", "./..."],
            "high",
            "go.mod detected",
        ));
    }
    if ecosystems.contains(&"java-maven") {
        commands.push(suggested(
            "test",
            &["mvn", "test"],
            "medium",
            "pom.xml detected",
        ));
        commands.push(suggested(
            "build",
            &["mvn", "package"],
            "medium",
            "pom.xml detected",
        ));
    }
    if ecosystems.contains(&"java-gradle") {
        commands.push(suggested(
            "test",
            &["gradle", "test"],
            "medium",
            "Gradle build file detected",
        ));
        commands.push(suggested(
            "build",
            &["gradle", "build"],
            "medium",
            "Gradle build file detected",
        ));
    }
    if ecosystems.contains(&"php") {
        commands.push(suggested(
            "install",
            &["composer", "install"],
            "medium",
            "composer.json detected",
        ));
        commands.push(suggested(
            "test",
            &["composer", "test"],
            "low",
            "composer.json detected",
        ));
    }
    if ecosystems.contains(&"ruby") {
        commands.push(suggested(
            "install",
            &["bundle", "install"],
            "medium",
            "Gemfile detected",
        ));
        commands.push(suggested(
            "test",
            &["bundle", "exec", "rspec"],
            "low",
            "Gemfile detected",
        ));
    }
    if ecosystems.contains(&"elixir") {
        commands.push(suggested(
            "deps",
            &["mix", "deps.get"],
            "medium",
            "mix.exs detected",
        ));
        commands.push(suggested(
            "test",
            &["mix", "test"],
            "medium",
            "mix.exs detected",
        ));
    }
    if ecosystems.contains(&"dart") {
        let dart_tool = if tools.contains(&"flutter") {
            "flutter"
        } else {
            "dart"
        };
        commands.push(suggested(
            "test",
            &[dart_tool, "test"],
            "medium",
            "pubspec.yaml detected",
        ));
    }
    if ecosystems.contains(&"swift") {
        commands.push(suggested(
            "test",
            &["swift", "test"],
            "medium",
            "Package.swift detected",
        ));
        commands.push(suggested(
            "build",
            &["swift", "build"],
            "medium",
            "Package.swift detected",
        ));
    }
    if ecosystems.contains(&"scala") {
        commands.push(suggested(
            "test",
            &["sbt", "test"],
            "medium",
            "build.sbt detected",
        ));
    }
    if tools.contains(&"clojure") {
        commands.push(suggested(
            "test",
            &["clojure", "-X:test"],
            "low",
            "deps.edn detected",
        ));
    }
    if tools.contains(&"leiningen") {
        commands.push(suggested(
            "test",
            &["lein", "test"],
            "medium",
            "project.clj detected",
        ));
    }
    if tools.contains(&"stack") {
        commands.push(suggested(
            "test",
            &["stack", "test"],
            "medium",
            "stack.yaml detected",
        ));
    }
    if tools.contains(&"cabal") {
        commands.push(suggested(
            "test",
            &["cabal", "test", "all"],
            "medium",
            "Cabal project detected",
        ));
        commands.push(suggested(
            "build",
            &["cabal", "build", "all"],
            "medium",
            "Cabal project detected",
        ));
    }
    if tools.contains(&"dune") {
        commands.push(suggested(
            "test",
            &["dune", "runtest"],
            "medium",
            "dune-project detected",
        ));
        commands.push(suggested(
            "build",
            &["dune", "build"],
            "medium",
            "dune-project detected",
        ));
    }
    if ecosystems.contains(&"julia") {
        commands.push(suggested(
            "test",
            &["julia", "--project=.", "-e", "using Pkg; Pkg.test()"],
            "medium",
            "Julia Project.toml detected",
        ));
    }
    if ecosystems.contains(&"r") {
        commands.push(suggested(
            "test",
            &["Rscript", "-e", "devtools::test()"],
            "low",
            "R DESCRIPTION detected",
        ));
    }
    if tools.contains(&"zig") {
        commands.push(suggested(
            "test",
            &["zig", "build", "test"],
            "medium",
            "build.zig detected",
        ));
        commands.push(suggested(
            "build",
            &["zig", "build"],
            "medium",
            "build.zig detected",
        ));
    }
    if tools.contains(&"platformio") {
        commands.push(suggested(
            "build",
            &["pio", "run"],
            "medium",
            "platformio.ini detected",
        ));
        commands.push(suggested(
            "test",
            &["pio", "test"],
            "low",
            "platformio.ini detected",
        ));
    }
    if tools.contains(&"meson") {
        commands.push(suggested(
            "configure",
            &["meson", "setup", "build"],
            "medium",
            "meson.build detected",
        ));
        commands.push(suggested(
            "test",
            &["meson", "test", "-C", "build"],
            "medium",
            "meson.build detected",
        ));
    }
    if tools.contains(&"bazel") {
        commands.push(suggested(
            "test",
            &["bazel", "test", "//..."],
            "medium",
            "Bazel workspace detected",
        ));
        commands.push(suggested(
            "build",
            &["bazel", "build", "//..."],
            "medium",
            "Bazel workspace detected",
        ));
    }
    if tools.contains(&"cmake") {
        commands.push(suggested(
            "configure",
            &["cmake", "-S", ".", "-B", "build"],
            "medium",
            "CMakeLists.txt detected",
        ));
        commands.push(suggested(
            "build",
            &["cmake", "--build", "build"],
            "medium",
            "CMakeLists.txt detected",
        ));
        commands.push(suggested(
            "test",
            &["ctest", "--test-dir", "build"],
            "low",
            "CMakeLists.txt detected",
        ));
    }
    if tools.contains(&"make") {
        commands.push(suggested("build", &["make"], "low", "Makefile detected"));
        commands.push(suggested(
            "test",
            &["make", "test"],
            "low",
            "Makefile detected",
        ));
    }
    if tools.contains(&"terraform") {
        commands.push(suggested(
            "init",
            &["terraform", "init"],
            "medium",
            "Terraform files detected",
        ));
        commands.push(suggested(
            "validate",
            &["terraform", "validate"],
            "medium",
            "Terraform files detected",
        ));
        commands.push(suggested(
            "plan",
            &["terraform", "plan"],
            "low",
            "Terraform files detected",
        ));
    }
    if tools.contains(&"docker") {
        commands.push(suggested(
            "container_build",
            &["docker", "build", "-t", "app", "."],
            "low",
            "Dockerfile detected",
        ));
    }
    if tools.contains(&"docker-compose") {
        commands.push(suggested(
            "compose_config",
            &["docker", "compose", "config"],
            "medium",
            "Compose file detected",
        ));
    }
    if tools.contains(&"pulumi") {
        commands.push(suggested(
            "preview",
            &["pulumi", "preview"],
            "medium",
            "Pulumi.yaml detected",
        ));
    }
    if tools.contains(&"tofu") {
        commands.push(suggested(
            "init",
            &["tofu", "init"],
            "medium",
            "OpenTofu files detected",
        ));
        commands.push(suggested(
            "plan",
            &["tofu", "plan"],
            "low",
            "OpenTofu files detected",
        ));
    }
    if tools.contains(&"nomad") {
        commands.push(suggested(
            "validate",
            &["nomad", "job", "validate"],
            "low",
            "Nomad job files detected",
        ));
    }
    if tools.contains(&"pre-commit") {
        commands.push(suggested(
            "quality",
            &["pre-commit", "run", "--all-files"],
            "medium",
            "pre-commit config detected",
        ));
    }
    if tools.contains(&"semgrep") {
        commands.push(suggested(
            "security",
            &["semgrep", "scan"],
            "medium",
            "Semgrep config detected",
        ));
    }
    if tools.contains(&"trivy") {
        commands.push(suggested(
            "security",
            &["trivy", "fs", "."],
            "medium",
            "Trivy config detected",
        ));
    }
    Ok(deduplicate_commands(commands))
}

pub(super) fn suggested(
    kind: &str,
    command: &[&str],
    confidence: &str,
    reason: &str,
) -> SuggestedCommand {
    SuggestedCommand {
        kind: kind.to_owned(),
        command: command.iter().map(|value| (*value).to_owned()).collect(),
        confidence: confidence.to_owned(),
        reason: reason.to_owned(),
    }
}
