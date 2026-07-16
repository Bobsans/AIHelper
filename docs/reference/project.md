# `ah project`

Project detection helpers for agents that need cross-platform context before choosing build, test, release, or infrastructure commands.

## `ah project detect`

Detect common ecosystems, tools, project roles, versions, suggested commands, and important project files.

```bash
ah project detect [path] [--json]
```

Detected file groups:
- package files: Cargo, npm, Python, Go, Maven, Gradle, .NET
- lock files: Cargo, npm, pnpm, Yarn, Bun, uv, Poetry, Composer, Bundler, Mix, Pub, Go, .NET
- CI files: GitHub Actions, GitLab CI, Azure Pipelines
- docs files: README
- changelog files: CHANGELOG, CHANGES, HISTORY
- deploy files: Docker, Compose, Helm, Kustomize
- infra files: Terraform, OpenTofu, Pulumi, Ansible, Nomad, AWS CDK
- config files: TypeScript/Vite/Next.js, Android, iOS, Tauri, Unity, Unreal, framework hints
- quality files: Renovate, Dependabot, pre-commit, lefthook, ESLint, Prettier, Ruff, PHPStan/Psalm, RuboCop
- security files: Semgrep, Trivy, CodeQL workflow hints

Additional ecosystems/tools include PHP/Composer, Ruby/Bundler, Elixir/Mix, Dart/Flutter/Pub, SwiftPM, Scala/SBT, Clojure, Haskell, OCaml, R, Julia, Lua, Zig, Erlang, Crystal, Perl, CMake, Meson, Bazel, Make, Conan, vcpkg, Docker/Compose, Terraform/OpenTofu, Helm, Kustomize, Ansible, Pulumi, Serverless, AWS SAM/CDK, Skaffold, Tilt, Argo CD, Flux, Nomad, Nix, Salesforce, Unity, Unreal, Electron, Tauri, and React Native.

Behavior:
- scans the target directory up to a bounded depth
- skips common generated folders such as `.git`, `target`, `node_modules`, `dist`, and `build`
- does not execute package managers or shell commands
- infers roles such as `web`, `backend`, `mobile`, `desktop`, `game`, `data-science`, `embedded`, `cloud`, `infra`, `deploy`, `docs`, `quality`, `security`, and `monorepo`
- reads `package.json` dependency names to infer common JS roles/tools such as Next/Vite/Astro, Express/Fastify/Nest, React Native/Expo, Electron/Tauri, Docusaurus, Playwright/Cypress/Vitest/Jest, ESLint/Prettier, and Semgrep
- JSON output keeps legacy fields (`package_files`, `ci_files`, `docs_files`, `changelog_files`) and adds richer `tools`, `roles`, `files`, `versions`, and `commands` fields

Interactive text output uses semantic colors for project roots, ecosystems,
tools, roles, file groups, paths, command kinds, versions, and confidence.
Colors are disabled automatically for pipes, redirects, captured output, and
JSON. Set `NO_COLOR` to disable colors explicitly.

Status: implemented.

## `ah project commands`

Suggest common commands from detected ecosystems, package managers, lockfiles, and project files.

```bash
ah project commands [path] [--json]
```

Examples of suggestions:
- Rust: `cargo fmt --all -- --check`, `cargo test --workspace --all-targets --locked`, `cargo build --locked`
- Node: npm/pnpm/yarn/bun install and real `package.json` scripts for test/build/lint/format
- .NET: `dotnet restore`, `dotnet test`, `dotnet build`
- Go: `go test ./...`, `go build ./...`
- PHP/Ruby/Elixir/Dart/Swift/Scala/Clojure/Haskell/OCaml/Julia/R/Zig/PlatformIO/CMake/Meson/Bazel/Make/Terraform/OpenTofu/Pulumi/Docker/security tooling: common safe suggestions based on detected files

Text formatting does not add confidence or reason fields that are otherwise
available in JSON output.

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
- PHP: `composer.json` `version`
- Dart/Flutter: `pubspec.yaml` `version`
- Ruby/Elixir: simple assignment-style version extraction from `.gemspec` / `mix.exs`
- .NET: `.csproj` `<Version>`
- Maven: `pom.xml` `<version>`
- Gradle: `build.gradle` / `build.gradle.kts` `version`

Status: implemented.
