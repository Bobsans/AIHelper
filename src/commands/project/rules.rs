#[derive(Debug, Clone, Copy)]
pub enum FileGroup {
    Package,
    Lock,
    Ci,
    Docs,
    Changelog,
    Deploy,
    Infra,
    Config,
    Quality,
    Security,
}

#[derive(Debug, Clone, Copy)]
pub struct FileRuleDetection {
    pub group: FileGroup,
    pub kind: &'static str,
    pub ecosystem: Option<&'static str>,
    pub tool: Option<&'static str>,
    pub role: Option<&'static str>,
}

pub fn classify_file(rel: &str, name: &str) -> Vec<FileRuleDetection> {
    let lower_rel = rel.to_ascii_lowercase();
    let lower_name = name.to_ascii_lowercase();
    let mut detections = Vec::new();

    match lower_name.as_str() {
        "cargo.toml" => detections.push(package("cargo", "rust", "cargo")),
        "package.json" => detections.push(package("npm", "node", "npm")),
        "pyproject.toml" => detections.push(package("python", "python", "python")),
        "go.mod" => detections.push(package("go", "go", "go")),
        "pom.xml" => detections.push(package("maven", "java-maven", "maven")),
        "build.gradle" | "build.gradle.kts" => {
            detections.push(package("gradle", "java-gradle", "gradle"))
        }
        "composer.json" => detections.push(package("composer", "php", "composer")),
        "gemfile" => detections.push(package("bundler", "ruby", "bundler")),
        "mix.exs" => detections.push(package("mix", "elixir", "mix")),
        "pubspec.yaml" => detections.push(package("pub", "dart", "pub")),
        "package.swift" => detections.push(package("swiftpm", "swift", "swift")),
        "build.sbt" => detections.push(grouped(
            FileGroup::Package,
            "sbt",
            Some("scala"),
            Some("sbt"),
            Some("backend"),
        )),
        "deps.edn" => detections.push(grouped(
            FileGroup::Package,
            "clojure-deps",
            Some("clojure"),
            Some("clojure"),
            Some("backend"),
        )),
        "project.clj" => detections.push(grouped(
            FileGroup::Package,
            "leiningen",
            Some("clojure"),
            Some("leiningen"),
            Some("backend"),
        )),
        "stack.yaml" => detections.push(package("stack", "haskell", "stack")),
        "cabal.project" => detections.push(package("cabal-project", "haskell", "cabal")),
        "dune-project" => detections.push(package("dune", "ocaml", "dune")),
        "project.toml" => detections.push(grouped(
            FileGroup::Package,
            "julia-project",
            Some("julia"),
            Some("julia"),
            Some("data-science"),
        )),
        "manifest.toml" => detections.push(lock("julia-manifest", "julia", "julia")),
        "description" => detections.push(grouped(
            FileGroup::Package,
            "r-package",
            Some("r"),
            Some("r"),
            Some("data-science"),
        )),
        "renv.lock" => detections.push(lock("renv-lock", "r", "renv")),
        "rebar.config" => detections.push(grouped(
            FileGroup::Package,
            "rebar",
            Some("erlang"),
            Some("rebar3"),
            Some("backend"),
        )),
        "shard.yml" => detections.push(grouped(
            FileGroup::Package,
            "shards",
            Some("crystal"),
            Some("shards"),
            Some("backend"),
        )),
        "rockspec" => detections.push(package("luarocks", "lua", "luarocks")),
        "cpanfile" => detections.push(package("cpanfile", "perl", "cpanm")),
        "cmakelists.txt" => detections.push(package("cmake", "cpp", "cmake")),
        "meson.build" => detections.push(package("meson", "cpp", "meson")),
        "build.zig" => detections.push(package("zig", "zig", "zig")),
        "platformio.ini" => detections.push(grouped(
            FileGroup::Package,
            "platformio",
            Some("embedded"),
            Some("platformio"),
            Some("embedded"),
        )),
        "sketch.yaml" => detections.push(grouped(
            FileGroup::Config,
            "arduino-sketch",
            Some("embedded"),
            Some("arduino"),
            Some("embedded"),
        )),
        "workspace" | "workspace.bazel" | "module.bazel" => {
            detections.push(package("bazel", "bazel", "bazel"))
        }
        "makefile" => detections.push(package("make", "make", "make")),
        "conanfile.txt" | "conanfile.py" => detections.push(package("conan", "cpp", "conan")),
        "vcpkg.json" => detections.push(package("vcpkg", "cpp", "vcpkg")),
        "flake.nix" => detections.push(package("nix-flake", "nix", "nix")),
        "shell.nix" => detections.push(package("nix-shell", "nix", "nix")),
        "cargo.lock" => detections.push(lock("cargo-lock", "rust", "cargo")),
        "package-lock.json" => detections.push(lock("package-lock", "node", "npm")),
        "pnpm-lock.yaml" => detections.push(lock("pnpm-lock", "node", "pnpm")),
        "yarn.lock" => detections.push(lock("yarn-lock", "node", "yarn")),
        "bun.lock" | "bun.lockb" => detections.push(lock("bun-lock", "node", "bun")),
        "uv.lock" => detections.push(lock("uv-lock", "python", "uv")),
        "poetry.lock" => detections.push(lock("poetry-lock", "python", "poetry")),
        "composer.lock" => detections.push(lock("composer-lock", "php", "composer")),
        "gemfile.lock" => detections.push(lock("gemfile-lock", "ruby", "bundler")),
        "mix.lock" => detections.push(lock("mix-lock", "elixir", "mix")),
        "pubspec.lock" => detections.push(lock("pubspec-lock", "dart", "pub")),
        "go.sum" => detections.push(lock("go-sum", "go", "go")),
        "packages.lock.json" => detections.push(lock("dotnet-lock", "dotnet", "dotnet")),
        "shard.lock" => detections.push(lock("shard-lock", "crystal", "shards")),
        "readme.md" | "readme" => {
            detections.push(grouped(FileGroup::Docs, "readme", None, None, Some("docs")))
        }
        "changelog.md" | "changes.md" | "history.md" => detections.push(grouped(
            FileGroup::Changelog,
            "changelog",
            None,
            None,
            Some("docs"),
        )),
        "dockerfile" => detections.push(deploy("dockerfile", "docker", "docker", "container")),
        "docker-compose.yml" | "docker-compose.yaml" | "compose.yml" | "compose.yaml" => {
            detections.push(deploy("compose", "docker", "docker-compose", "container"))
        }
        "chart.yaml" => detections.push(deploy("helm-chart", "helm", "helm", "deploy")),
        "kustomization.yaml" | "kustomization.yml" => {
            detections.push(deploy("kustomize", "kubernetes", "kustomize", "deploy"))
        }
        "pulumi.yaml" => detections.push(grouped(
            FileGroup::Infra,
            "pulumi",
            Some("pulumi"),
            Some("pulumi"),
            Some("cloud"),
        )),
        "serverless.yml" | "serverless.yaml" => detections.push(grouped(
            FileGroup::Deploy,
            "serverless",
            Some("serverless"),
            Some("serverless"),
            Some("cloud"),
        )),
        "template.yml" | "template.yaml" => detections.push(grouped(
            FileGroup::Deploy,
            "aws-sam",
            Some("aws-sam"),
            Some("sam"),
            Some("cloud"),
        )),
        "cdk.json" => detections.push(grouped(
            FileGroup::Infra,
            "aws-cdk",
            Some("aws-cdk"),
            Some("cdk"),
            Some("cloud"),
        )),
        "skaffold.yaml" | "skaffold.yml" => {
            detections.push(deploy("skaffold", "kubernetes", "skaffold", "deploy"))
        }
        "tiltfile" => detections.push(deploy("tilt", "kubernetes", "tilt", "deploy")),
        "appfile" | "fastfile" => detections.push(grouped(
            FileGroup::Config,
            "fastlane",
            Some("mobile"),
            Some("fastlane"),
            Some("mobile"),
        )),
        "podfile" => detections.push(grouped(
            FileGroup::Package,
            "cocoapods",
            Some("ios"),
            Some("cocoapods"),
            Some("mobile"),
        )),
        "cartfile" => detections.push(grouped(
            FileGroup::Package,
            "carthage",
            Some("ios"),
            Some("carthage"),
            Some("mobile"),
        )),
        "sfdx-project.json" => detections.push(grouped(
            FileGroup::Package,
            "salesforce",
            Some("salesforce"),
            Some("sfdx"),
            Some("backend"),
        )),
        "manage.py" => detections.push(grouped(
            FileGroup::Config,
            "django",
            Some("python"),
            Some("django"),
            Some("backend"),
        )),
        "artisan" => detections.push(grouped(
            FileGroup::Config,
            "laravel",
            Some("php"),
            Some("laravel"),
            Some("backend"),
        )),
        "phoenix" => detections.push(grouped(
            FileGroup::Config,
            "phoenix",
            Some("elixir"),
            Some("phoenix"),
            Some("backend"),
        )),
        "application.properties" | "application.yml" | "application.yaml" => {
            detections.push(grouped(
                FileGroup::Config,
                "spring-boot",
                Some("java"),
                Some("spring-boot"),
                Some("backend"),
            ))
        }
        "docker-compose.override.yml" | "docker-compose.override.yaml" => {
            detections.push(deploy("compose", "docker", "docker-compose", "container"))
        }
        "projectversion.txt" => {
            if lower_rel.starts_with("projectsettings/") {
                detections.push(grouped(
                    FileGroup::Config,
                    "unity-project",
                    Some("unity"),
                    Some("unity"),
                    Some("game"),
                ));
            }
        }
        "renovate.json" | ".renovaterc" | ".renovaterc.json" => detections.push(grouped(
            FileGroup::Quality,
            "renovate",
            None,
            Some("renovate"),
            Some("quality"),
        )),
        "dependabot.yml" | "dependabot.yaml" => detections.push(grouped(
            FileGroup::Quality,
            "dependabot",
            None,
            Some("dependabot"),
            Some("quality"),
        )),
        ".pre-commit-config.yaml" | ".pre-commit-config.yml" => detections.push(grouped(
            FileGroup::Quality,
            "pre-commit",
            None,
            Some("pre-commit"),
            Some("quality"),
        )),
        "lefthook.yml" | "lefthook.yaml" => detections.push(grouped(
            FileGroup::Quality,
            "lefthook",
            None,
            Some("lefthook"),
            Some("quality"),
        )),
        ".eslintrc" | ".eslintrc.json" | "eslint.config.js" | "eslint.config.mjs"
        | "eslint.config.ts" => detections.push(grouped(
            FileGroup::Quality,
            "eslint",
            Some("node"),
            Some("eslint"),
            Some("quality"),
        )),
        ".prettierrc" | ".prettierrc.json" | "prettier.config.js" | "prettier.config.mjs" => {
            detections.push(grouped(
                FileGroup::Quality,
                "prettier",
                Some("node"),
                Some("prettier"),
                Some("quality"),
            ))
        }
        "ruff.toml" | ".ruff.toml" => detections.push(grouped(
            FileGroup::Quality,
            "ruff",
            Some("python"),
            Some("ruff"),
            Some("quality"),
        )),
        "phpstan.neon" | "psalm.xml" | "rubocop.yml" | ".rubocop.yml" => detections.push(grouped(
            FileGroup::Quality,
            "static-analysis",
            None,
            None,
            Some("quality"),
        )),
        "semgrep.yml" | "semgrep.yaml" | ".semgrep.yml" | ".semgrep.yaml" => {
            detections.push(grouped(
                FileGroup::Security,
                "semgrep",
                None,
                Some("semgrep"),
                Some("security"),
            ))
        }
        ".trivyignore" | "trivy.yaml" | "trivy.yml" => detections.push(grouped(
            FileGroup::Security,
            "trivy",
            None,
            Some("trivy"),
            Some("security"),
        )),
        ".gitlab-ci.yml" | ".gitlab-ci.yaml" => detections.push(ci("gitlab-ci", "gitlab-ci")),
        "azure-pipelines.yml" | "azure-pipelines.yaml" => {
            detections.push(ci("azure-pipelines", "azure-pipelines"))
        }
        "jenkinsfile" => detections.push(ci("jenkins", "jenkins")),
        ".drone.yml" | ".drone.yaml" => detections.push(ci("drone", "drone")),
        ".woodpecker.yml" | ".woodpecker.yaml" => detections.push(ci("woodpecker", "woodpecker")),
        "tsconfig.json" => detections.push(config("tsconfig", "node", "typescript")),
        "vite.config.js" | "vite.config.ts" | "vite.config.mjs" => detections.push(grouped(
            FileGroup::Config,
            "vite",
            Some("node"),
            Some("vite"),
            Some("web"),
        )),
        "next.config.js" | "next.config.mjs" | "next.config.ts" => detections.push(grouped(
            FileGroup::Config,
            "next",
            Some("node"),
            Some("next"),
            Some("web"),
        )),
        "astro.config.mjs" | "astro.config.js" | "astro.config.ts" => detections.push(grouped(
            FileGroup::Config,
            "astro",
            Some("node"),
            Some("astro"),
            Some("web"),
        )),
        "hugo.toml" | "hugo.yaml" | "config.toml" | "config.yaml" => detections.push(grouped(
            FileGroup::Docs,
            "static-site-config",
            None,
            None,
            Some("docs"),
        )),
        "mkdocs.yml" | "mkdocs.yaml" => detections.push(grouped(
            FileGroup::Docs,
            "mkdocs",
            Some("python"),
            Some("mkdocs"),
            Some("docs"),
        )),
        "docusaurus.config.js" | "docusaurus.config.ts" => detections.push(grouped(
            FileGroup::Docs,
            "docusaurus",
            Some("node"),
            Some("docusaurus"),
            Some("docs"),
        )),
        "conf.py" => {
            if lower_rel.contains("docs/") || lower_rel.contains("doc/") {
                detections.push(grouped(
                    FileGroup::Docs,
                    "sphinx",
                    Some("python"),
                    Some("sphinx"),
                    Some("docs"),
                ));
            }
        }
        _ => {}
    }

    if lower_name.ends_with(".csproj") {
        detections.push(package("dotnet", "dotnet", "dotnet"));
    }
    if lower_name.ends_with(".gemspec") {
        detections.push(package("gemspec", "ruby", "rubygems"));
    }
    if lower_name.ends_with(".cabal") {
        detections.push(package("cabal", "haskell", "cabal"));
    }
    if lower_name.ends_with(".opam") {
        detections.push(package("opam", "ocaml", "opam"));
    }
    if lower_name.ends_with(".rockspec") {
        detections.push(package("luarocks", "lua", "luarocks"));
    }
    if matches!(
        lower_name.as_str(),
        "makefile.pl" | "build.pl" | "meta.json" | "meta.yml" | "meta6.json"
    ) {
        detections.push(package("perl-meta", "perl", "perl"));
    }
    if lower_name.ends_with(".ipynb") {
        detections.push(grouped(
            FileGroup::Package,
            "jupyter-notebook",
            Some("jupyter"),
            Some("jupyter"),
            Some("data-science"),
        ));
    }
    if lower_name.ends_with(".xcodeproj") || lower_name.ends_with(".xcworkspace") {
        detections.push(grouped(
            FileGroup::Config,
            "xcode",
            Some("ios"),
            Some("xcodebuild"),
            Some("mobile"),
        ));
    }
    if lower_name.ends_with(".uproject") || lower_name.ends_with(".uplugin") {
        detections.push(grouped(
            FileGroup::Config,
            "unreal",
            Some("unreal"),
            Some("unreal"),
            Some("game"),
        ));
    }
    if lower_name.ends_with(".nomad") {
        detections.push(grouped(
            FileGroup::Infra,
            "nomad",
            Some("nomad"),
            Some("nomad"),
            Some("infra"),
        ));
    }
    if lower_name.ends_with(".tofu") {
        detections.push(grouped(
            FileGroup::Infra,
            "opentofu",
            Some("opentofu"),
            Some("tofu"),
            Some("infra"),
        ));
    }
    if lower_name.starts_with("dockerfile.") {
        detections.push(deploy("dockerfile", "docker", "docker", "container"));
    }
    if lower_name.ends_with(".tf") {
        detections.push(grouped(
            FileGroup::Infra,
            "terraform",
            Some("terraform"),
            Some("terraform"),
            Some("infra"),
        ));
    }
    if lower_name.ends_with(".tfvars") {
        detections.push(grouped(
            FileGroup::Infra,
            "terraform-vars",
            Some("terraform"),
            Some("terraform"),
            Some("infra"),
        ));
    }
    if lower_name.ends_with(".gradle") || lower_name.ends_with(".gradle.kts") {
        if lower_rel.contains("android")
            || lower_rel == "settings.gradle"
            || lower_rel == "settings.gradle.kts"
        {
            detections.push(grouped(
                FileGroup::Config,
                "android-gradle",
                Some("android"),
                Some("gradle"),
                Some("mobile"),
            ));
        }
    }
    if lower_name == "androidmanifest.xml" {
        detections.push(grouped(
            FileGroup::Config,
            "android-manifest",
            Some("android"),
            Some("android"),
            Some("mobile"),
        ));
    }
    if lower_rel.starts_with("src-tauri/") {
        detections.push(grouped(
            FileGroup::Config,
            "tauri",
            Some("tauri"),
            Some("tauri"),
            Some("desktop"),
        ));
    }
    if lower_rel.starts_with("ios/") || lower_rel.starts_with("android/") {
        detections.push(grouped(
            FileGroup::Config,
            "mobile-project",
            Some("mobile"),
            None,
            Some("mobile"),
        ));
    }
    if lower_rel.starts_with(".github/workflows/") {
        detections.push(ci("github-actions", "github-actions"));
    }
    if lower_rel.starts_with(".github/workflows/") && lower_rel.contains("codeql") {
        detections.push(grouped(
            FileGroup::Security,
            "codeql",
            None,
            Some("codeql"),
            Some("security"),
        ));
    }
    if lower_rel == ".circleci/config.yml" || lower_rel == ".circleci/config.yaml" {
        detections.push(ci("circleci", "circleci"));
    }
    if lower_rel.starts_with(".buildkite/") {
        detections.push(ci("buildkite", "buildkite"));
    }
    if lower_rel.starts_with(".fluxcd/") || lower_rel.contains("gotk-components") {
        detections.push(deploy("flux", "kubernetes", "flux", "deploy"));
    }
    if lower_rel.contains("argocd") || lower_rel.contains("argo-cd") {
        detections.push(deploy("argo-cd", "kubernetes", "argocd", "deploy"));
    }
    if lower_rel.starts_with("roles/") || lower_rel.contains("/roles/") {
        detections.push(grouped(
            FileGroup::Infra,
            "ansible-role",
            Some("ansible"),
            Some("ansible"),
            Some("infra"),
        ));
    }
    if matches!(
        lower_name.as_str(),
        "playbook.yml" | "playbook.yaml" | "site.yml" | "site.yaml"
    ) {
        detections.push(grouped(
            FileGroup::Infra,
            "ansible-playbook",
            Some("ansible"),
            Some("ansible"),
            Some("infra"),
        ));
    }

    detections
}

fn package(kind: &'static str, ecosystem: &'static str, tool: &'static str) -> FileRuleDetection {
    grouped(
        FileGroup::Package,
        kind,
        Some(ecosystem),
        Some(tool),
        Some("source"),
    )
}

fn lock(kind: &'static str, ecosystem: &'static str, tool: &'static str) -> FileRuleDetection {
    grouped(
        FileGroup::Lock,
        kind,
        Some(ecosystem),
        Some(tool),
        Some("source"),
    )
}

fn ci(kind: &'static str, tool: &'static str) -> FileRuleDetection {
    grouped(FileGroup::Ci, kind, None, Some(tool), Some("ci"))
}

fn deploy(
    kind: &'static str,
    ecosystem: &'static str,
    tool: &'static str,
    role: &'static str,
) -> FileRuleDetection {
    grouped(
        FileGroup::Deploy,
        kind,
        Some(ecosystem),
        Some(tool),
        Some(role),
    )
}

fn config(kind: &'static str, ecosystem: &'static str, tool: &'static str) -> FileRuleDetection {
    grouped(
        FileGroup::Config,
        kind,
        Some(ecosystem),
        Some(tool),
        Some("source"),
    )
}

fn grouped(
    group: FileGroup,
    kind: &'static str,
    ecosystem: Option<&'static str>,
    tool: Option<&'static str>,
    role: Option<&'static str>,
) -> FileRuleDetection {
    FileRuleDetection {
        group,
        kind,
        ecosystem,
        tool,
        role,
    }
}
