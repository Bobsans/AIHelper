# `ah project`

Project detection helpers for agents that need simple, cross-platform context before choosing build or test commands.

## `ah project detect`

Detect common ecosystems and important project files.

```bash
ah project detect [path] [--json]
```

Detected file groups:
- package files: Cargo, npm, Python, Go, Maven, Gradle, .NET
- CI files: GitHub Actions, GitLab CI, Azure Pipelines
- docs files: README
- changelog files: CHANGELOG, CHANGES, HISTORY

Behavior:
- scans the target directory up to a bounded depth
- skips common generated folders such as `.git`, `target`, `node_modules`, `dist`, and `build`
- does not execute package managers or shell commands

Status: implemented.

## `ah project commands`

Suggest common commands from detected ecosystems.

```bash
ah project commands [path] [--json]
```

Examples of suggestions:
- Rust: `cargo fmt --all -- --check`, `cargo test --workspace --all-targets --locked`, `cargo build --locked`
- Node: `npm install`, `npm test`, `npm run build`
- .NET: `dotnet restore`, `dotnet test`, `dotnet build`
- Go: `go test ./...`, `go build ./...`

Status: implemented.

## `ah project version`

Detect project versions from common manifest files without executing package managers.

```bash
ah project version [path] [--limit N] [--json]
```

Supported manifests:
- Rust: `Cargo.toml` `[package] version`
- Node: `package.json` `version`
- Python: `pyproject.toml` `[project] version`
- .NET: `.csproj` `<Version>`
- Maven: `pom.xml` `<version>`
- Gradle: `build.gradle` / `build.gradle.kts` `version`

Status: implemented.
