#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

/// When a rule applies.
///
/// Every comparison is against the lowercased file name or the lowercased path
/// relative to the project root, so a rule never has to say which.
#[derive(Debug, Clone, Copy)]
enum When {
    Name(&'static str),
    NameStartsWith(&'static str),
    NameEndsWith(&'static str),
    Path(&'static str),
    PathStartsWith(&'static str),
    PathContains(&'static str),
    Any(&'static [When]),
    All(&'static [When]),
}

impl When {
    fn matches(&self, path: &str, name: &str) -> bool {
        match self {
            Self::Name(value) => name == *value,
            Self::NameStartsWith(value) => name.starts_with(value),
            Self::NameEndsWith(value) => name.ends_with(value),
            Self::Path(value) => path == *value,
            Self::PathStartsWith(value) => path.starts_with(value),
            Self::PathContains(value) => path.contains(value),
            Self::Any(conditions) => conditions
                .iter()
                .any(|condition| condition.matches(path, name)),
            Self::All(conditions) => conditions
                .iter()
                .all(|condition| condition.matches(path, name)),
        }
    }
}

/// One classification: what has to be true, and what that means.
#[derive(Debug, Clone, Copy)]
struct FileRule {
    when: When,
    detection: FileRuleDetection,
}

/// A table entry.
///
/// This is a function rather than a struct literal only so that rustfmt keeps
/// one rule on one line: 121 four-line literals read as code rather than as the
/// data they are.
const fn rule(when: When, detection: FileRuleDetection) -> FileRule {
    FileRule { when, detection }
}

/// Rule order is output order, and a file may match several rules - an Android
/// `build.gradle` is a Gradle package, an Android config and a mobile project.
/// This is the order the hand-written `match` and the `if` chain after it
/// produced, so it is part of the published output.
static RULES: &[FileRule] = &[
    rule(When::Name("cargo.toml"), package("cargo", "rust", "cargo")),
    rule(When::Name("package.json"), package("npm", "node", "npm")),
    rule(
        When::Name("pyproject.toml"),
        package("python", "python", "python"),
    ),
    rule(When::Name("go.mod"), package("go", "go", "go")),
    rule(
        When::Name("pom.xml"),
        package("maven", "java-maven", "maven"),
    ),
    rule(
        When::Any(&[When::Name("build.gradle"), When::Name("build.gradle.kts")]),
        package("gradle", "java-gradle", "gradle"),
    ),
    rule(
        When::Name("composer.json"),
        package("composer", "php", "composer"),
    ),
    rule(When::Name("gemfile"), package("bundler", "ruby", "bundler")),
    rule(When::Name("mix.exs"), package("mix", "elixir", "mix")),
    rule(When::Name("pubspec.yaml"), package("pub", "dart", "pub")),
    rule(
        When::Name("package.swift"),
        package("swiftpm", "swift", "swift"),
    ),
    rule(
        When::Name("build.sbt"),
        grouped(
            FileGroup::Package,
            "sbt",
            Some("scala"),
            Some("sbt"),
            Some("backend"),
        ),
    ),
    rule(
        When::Name("deps.edn"),
        grouped(
            FileGroup::Package,
            "clojure-deps",
            Some("clojure"),
            Some("clojure"),
            Some("backend"),
        ),
    ),
    rule(
        When::Name("project.clj"),
        grouped(
            FileGroup::Package,
            "leiningen",
            Some("clojure"),
            Some("leiningen"),
            Some("backend"),
        ),
    ),
    rule(
        When::Name("stack.yaml"),
        package("stack", "haskell", "stack"),
    ),
    rule(
        When::Name("cabal.project"),
        package("cabal-project", "haskell", "cabal"),
    ),
    rule(When::Name("dune-project"), package("dune", "ocaml", "dune")),
    rule(
        When::Name("project.toml"),
        grouped(
            FileGroup::Package,
            "julia-project",
            Some("julia"),
            Some("julia"),
            Some("data-science"),
        ),
    ),
    rule(
        When::Name("manifest.toml"),
        lock("julia-manifest", "julia", "julia"),
    ),
    rule(
        When::Name("description"),
        grouped(
            FileGroup::Package,
            "r-package",
            Some("r"),
            Some("r"),
            Some("data-science"),
        ),
    ),
    rule(When::Name("renv.lock"), lock("renv-lock", "r", "renv")),
    rule(
        When::Name("rebar.config"),
        grouped(
            FileGroup::Package,
            "rebar",
            Some("erlang"),
            Some("rebar3"),
            Some("backend"),
        ),
    ),
    rule(
        When::Name("shard.yml"),
        grouped(
            FileGroup::Package,
            "shards",
            Some("crystal"),
            Some("shards"),
            Some("backend"),
        ),
    ),
    rule(
        When::Name("rockspec"),
        package("luarocks", "lua", "luarocks"),
    ),
    rule(When::Name("cpanfile"), package("cpanfile", "perl", "cpanm")),
    rule(
        When::Name("cmakelists.txt"),
        package("cmake", "cpp", "cmake"),
    ),
    rule(When::Name("meson.build"), package("meson", "cpp", "meson")),
    rule(When::Name("build.zig"), package("zig", "zig", "zig")),
    rule(
        When::Name("platformio.ini"),
        grouped(
            FileGroup::Package,
            "platformio",
            Some("embedded"),
            Some("platformio"),
            Some("embedded"),
        ),
    ),
    rule(
        When::Name("sketch.yaml"),
        grouped(
            FileGroup::Config,
            "arduino-sketch",
            Some("embedded"),
            Some("arduino"),
            Some("embedded"),
        ),
    ),
    rule(
        When::Any(&[
            When::Name("workspace"),
            When::Name("workspace.bazel"),
            When::Name("module.bazel"),
        ]),
        package("bazel", "bazel", "bazel"),
    ),
    rule(When::Name("makefile"), package("make", "make", "make")),
    rule(
        When::Any(&[When::Name("conanfile.txt"), When::Name("conanfile.py")]),
        package("conan", "cpp", "conan"),
    ),
    rule(When::Name("vcpkg.json"), package("vcpkg", "cpp", "vcpkg")),
    rule(When::Name("flake.nix"), package("nix-flake", "nix", "nix")),
    rule(When::Name("shell.nix"), package("nix-shell", "nix", "nix")),
    rule(
        When::Name("cargo.lock"),
        lock("cargo-lock", "rust", "cargo"),
    ),
    rule(
        When::Name("package-lock.json"),
        lock("package-lock", "node", "npm"),
    ),
    rule(
        When::Name("pnpm-lock.yaml"),
        lock("pnpm-lock", "node", "pnpm"),
    ),
    rule(When::Name("yarn.lock"), lock("yarn-lock", "node", "yarn")),
    rule(
        When::Any(&[When::Name("bun.lock"), When::Name("bun.lockb")]),
        lock("bun-lock", "node", "bun"),
    ),
    rule(When::Name("uv.lock"), lock("uv-lock", "python", "uv")),
    rule(
        When::Name("poetry.lock"),
        lock("poetry-lock", "python", "poetry"),
    ),
    rule(
        When::Name("composer.lock"),
        lock("composer-lock", "php", "composer"),
    ),
    rule(
        When::Name("gemfile.lock"),
        lock("gemfile-lock", "ruby", "bundler"),
    ),
    rule(When::Name("mix.lock"), lock("mix-lock", "elixir", "mix")),
    rule(
        When::Name("pubspec.lock"),
        lock("pubspec-lock", "dart", "pub"),
    ),
    rule(When::Name("go.sum"), lock("go-sum", "go", "go")),
    rule(
        When::Name("packages.lock.json"),
        lock("dotnet-lock", "dotnet", "dotnet"),
    ),
    rule(
        When::Name("shard.lock"),
        lock("shard-lock", "crystal", "shards"),
    ),
    rule(
        When::Any(&[When::Name("readme.md"), When::Name("readme")]),
        grouped(FileGroup::Docs, "readme", None, None, Some("docs")),
    ),
    rule(
        When::Any(&[
            When::Name("changelog.md"),
            When::Name("changes.md"),
            When::Name("history.md"),
        ]),
        grouped(FileGroup::Changelog, "changelog", None, None, Some("docs")),
    ),
    rule(
        When::Name("dockerfile"),
        deploy("dockerfile", "docker", "docker", "container"),
    ),
    rule(
        When::Any(&[
            When::Name("docker-compose.yml"),
            When::Name("docker-compose.yaml"),
            When::Name("compose.yml"),
            When::Name("compose.yaml"),
        ]),
        deploy("compose", "docker", "docker-compose", "container"),
    ),
    rule(
        When::Name("chart.yaml"),
        deploy("helm-chart", "helm", "helm", "deploy"),
    ),
    rule(
        When::Any(&[
            When::Name("kustomization.yaml"),
            When::Name("kustomization.yml"),
        ]),
        deploy("kustomize", "kubernetes", "kustomize", "deploy"),
    ),
    rule(
        When::Name("pulumi.yaml"),
        grouped(
            FileGroup::Infra,
            "pulumi",
            Some("pulumi"),
            Some("pulumi"),
            Some("cloud"),
        ),
    ),
    rule(
        When::Any(&[When::Name("serverless.yml"), When::Name("serverless.yaml")]),
        grouped(
            FileGroup::Deploy,
            "serverless",
            Some("serverless"),
            Some("serverless"),
            Some("cloud"),
        ),
    ),
    rule(
        When::Any(&[When::Name("template.yml"), When::Name("template.yaml")]),
        grouped(
            FileGroup::Deploy,
            "aws-sam",
            Some("aws-sam"),
            Some("sam"),
            Some("cloud"),
        ),
    ),
    rule(
        When::Name("cdk.json"),
        grouped(
            FileGroup::Infra,
            "aws-cdk",
            Some("aws-cdk"),
            Some("cdk"),
            Some("cloud"),
        ),
    ),
    rule(
        When::Any(&[When::Name("skaffold.yaml"), When::Name("skaffold.yml")]),
        deploy("skaffold", "kubernetes", "skaffold", "deploy"),
    ),
    rule(
        When::Name("tiltfile"),
        deploy("tilt", "kubernetes", "tilt", "deploy"),
    ),
    rule(
        When::Any(&[When::Name("appfile"), When::Name("fastfile")]),
        grouped(
            FileGroup::Config,
            "fastlane",
            Some("mobile"),
            Some("fastlane"),
            Some("mobile"),
        ),
    ),
    rule(
        When::Name("podfile"),
        grouped(
            FileGroup::Package,
            "cocoapods",
            Some("ios"),
            Some("cocoapods"),
            Some("mobile"),
        ),
    ),
    rule(
        When::Name("cartfile"),
        grouped(
            FileGroup::Package,
            "carthage",
            Some("ios"),
            Some("carthage"),
            Some("mobile"),
        ),
    ),
    rule(
        When::Name("sfdx-project.json"),
        grouped(
            FileGroup::Package,
            "salesforce",
            Some("salesforce"),
            Some("sfdx"),
            Some("backend"),
        ),
    ),
    rule(
        When::Name("manage.py"),
        grouped(
            FileGroup::Config,
            "django",
            Some("python"),
            Some("django"),
            Some("backend"),
        ),
    ),
    rule(
        When::Name("artisan"),
        grouped(
            FileGroup::Config,
            "laravel",
            Some("php"),
            Some("laravel"),
            Some("backend"),
        ),
    ),
    rule(
        When::Name("phoenix"),
        grouped(
            FileGroup::Config,
            "phoenix",
            Some("elixir"),
            Some("phoenix"),
            Some("backend"),
        ),
    ),
    rule(
        When::Any(&[
            When::Name("application.properties"),
            When::Name("application.yml"),
            When::Name("application.yaml"),
        ]),
        grouped(
            FileGroup::Config,
            "spring-boot",
            Some("java"),
            Some("spring-boot"),
            Some("backend"),
        ),
    ),
    rule(
        When::Any(&[
            When::Name("docker-compose.override.yml"),
            When::Name("docker-compose.override.yaml"),
        ]),
        deploy("compose", "docker", "docker-compose", "container"),
    ),
    rule(
        When::All(&[
            When::Name("projectversion.txt"),
            When::PathStartsWith("projectsettings/"),
        ]),
        grouped(
            FileGroup::Config,
            "unity-project",
            Some("unity"),
            Some("unity"),
            Some("game"),
        ),
    ),
    rule(
        When::Any(&[
            When::Name("renovate.json"),
            When::Name(".renovaterc"),
            When::Name(".renovaterc.json"),
        ]),
        grouped(
            FileGroup::Quality,
            "renovate",
            None,
            Some("renovate"),
            Some("quality"),
        ),
    ),
    rule(
        When::Any(&[When::Name("dependabot.yml"), When::Name("dependabot.yaml")]),
        grouped(
            FileGroup::Quality,
            "dependabot",
            None,
            Some("dependabot"),
            Some("quality"),
        ),
    ),
    rule(
        When::Any(&[
            When::Name(".pre-commit-config.yaml"),
            When::Name(".pre-commit-config.yml"),
        ]),
        grouped(
            FileGroup::Quality,
            "pre-commit",
            None,
            Some("pre-commit"),
            Some("quality"),
        ),
    ),
    rule(
        When::Any(&[When::Name("lefthook.yml"), When::Name("lefthook.yaml")]),
        grouped(
            FileGroup::Quality,
            "lefthook",
            None,
            Some("lefthook"),
            Some("quality"),
        ),
    ),
    rule(
        When::Any(&[
            When::Name(".eslintrc"),
            When::Name(".eslintrc.json"),
            When::Name("eslint.config.js"),
            When::Name("eslint.config.mjs"),
            When::Name("eslint.config.ts"),
        ]),
        grouped(
            FileGroup::Quality,
            "eslint",
            Some("node"),
            Some("eslint"),
            Some("quality"),
        ),
    ),
    rule(
        When::Any(&[
            When::Name(".prettierrc"),
            When::Name(".prettierrc.json"),
            When::Name("prettier.config.js"),
            When::Name("prettier.config.mjs"),
        ]),
        grouped(
            FileGroup::Quality,
            "prettier",
            Some("node"),
            Some("prettier"),
            Some("quality"),
        ),
    ),
    rule(
        When::Any(&[When::Name("ruff.toml"), When::Name(".ruff.toml")]),
        grouped(
            FileGroup::Quality,
            "ruff",
            Some("python"),
            Some("ruff"),
            Some("quality"),
        ),
    ),
    rule(
        When::Any(&[
            When::Name("phpstan.neon"),
            When::Name("psalm.xml"),
            When::Name("rubocop.yml"),
            When::Name(".rubocop.yml"),
        ]),
        grouped(
            FileGroup::Quality,
            "static-analysis",
            None,
            None,
            Some("quality"),
        ),
    ),
    rule(
        When::Any(&[
            When::Name("semgrep.yml"),
            When::Name("semgrep.yaml"),
            When::Name(".semgrep.yml"),
            When::Name(".semgrep.yaml"),
        ]),
        grouped(
            FileGroup::Security,
            "semgrep",
            None,
            Some("semgrep"),
            Some("security"),
        ),
    ),
    rule(
        When::Any(&[
            When::Name(".trivyignore"),
            When::Name("trivy.yaml"),
            When::Name("trivy.yml"),
        ]),
        grouped(
            FileGroup::Security,
            "trivy",
            None,
            Some("trivy"),
            Some("security"),
        ),
    ),
    rule(
        When::Any(&[When::Name(".gitlab-ci.yml"), When::Name(".gitlab-ci.yaml")]),
        ci("gitlab-ci", "gitlab-ci"),
    ),
    rule(
        When::Any(&[
            When::Name("azure-pipelines.yml"),
            When::Name("azure-pipelines.yaml"),
        ]),
        ci("azure-pipelines", "azure-pipelines"),
    ),
    rule(When::Name("jenkinsfile"), ci("jenkins", "jenkins")),
    rule(
        When::Any(&[When::Name(".drone.yml"), When::Name(".drone.yaml")]),
        ci("drone", "drone"),
    ),
    rule(
        When::Any(&[
            When::Name(".woodpecker.yml"),
            When::Name(".woodpecker.yaml"),
        ]),
        ci("woodpecker", "woodpecker"),
    ),
    rule(
        When::Name("tsconfig.json"),
        config("tsconfig", "node", "typescript"),
    ),
    rule(
        When::Any(&[
            When::Name("vite.config.js"),
            When::Name("vite.config.ts"),
            When::Name("vite.config.mjs"),
        ]),
        grouped(
            FileGroup::Config,
            "vite",
            Some("node"),
            Some("vite"),
            Some("web"),
        ),
    ),
    rule(
        When::Any(&[
            When::Name("next.config.js"),
            When::Name("next.config.mjs"),
            When::Name("next.config.ts"),
        ]),
        grouped(
            FileGroup::Config,
            "next",
            Some("node"),
            Some("next"),
            Some("web"),
        ),
    ),
    rule(
        When::Any(&[
            When::Name("astro.config.mjs"),
            When::Name("astro.config.js"),
            When::Name("astro.config.ts"),
        ]),
        grouped(
            FileGroup::Config,
            "astro",
            Some("node"),
            Some("astro"),
            Some("web"),
        ),
    ),
    rule(
        When::Any(&[
            When::Name("hugo.toml"),
            When::Name("hugo.yaml"),
            When::Name("config.toml"),
            When::Name("config.yaml"),
        ]),
        grouped(
            FileGroup::Docs,
            "static-site-config",
            None,
            None,
            Some("docs"),
        ),
    ),
    rule(
        When::Any(&[When::Name("mkdocs.yml"), When::Name("mkdocs.yaml")]),
        grouped(
            FileGroup::Docs,
            "mkdocs",
            Some("python"),
            Some("mkdocs"),
            Some("docs"),
        ),
    ),
    rule(
        When::Any(&[
            When::Name("docusaurus.config.js"),
            When::Name("docusaurus.config.ts"),
        ]),
        grouped(
            FileGroup::Docs,
            "docusaurus",
            Some("node"),
            Some("docusaurus"),
            Some("docs"),
        ),
    ),
    rule(
        When::All(&[
            When::Name("conf.py"),
            When::Any(&[When::PathContains("docs/"), When::PathContains("doc/")]),
        ]),
        grouped(
            FileGroup::Docs,
            "sphinx",
            Some("python"),
            Some("sphinx"),
            Some("docs"),
        ),
    ),
    rule(
        When::NameEndsWith(".csproj"),
        package("dotnet", "dotnet", "dotnet"),
    ),
    rule(
        When::NameEndsWith(".gemspec"),
        package("gemspec", "ruby", "rubygems"),
    ),
    rule(
        When::NameEndsWith(".cabal"),
        package("cabal", "haskell", "cabal"),
    ),
    rule(
        When::NameEndsWith(".opam"),
        package("opam", "ocaml", "opam"),
    ),
    rule(
        When::NameEndsWith(".rockspec"),
        package("luarocks", "lua", "luarocks"),
    ),
    rule(
        When::Any(&[
            When::Name("makefile.pl"),
            When::Name("build.pl"),
            When::Name("meta.json"),
            When::Name("meta.yml"),
            When::Name("meta6.json"),
        ]),
        package("perl-meta", "perl", "perl"),
    ),
    rule(
        When::NameEndsWith(".ipynb"),
        grouped(
            FileGroup::Package,
            "jupyter-notebook",
            Some("jupyter"),
            Some("jupyter"),
            Some("data-science"),
        ),
    ),
    rule(
        When::Any(&[
            When::NameEndsWith(".xcodeproj"),
            When::NameEndsWith(".xcworkspace"),
        ]),
        grouped(
            FileGroup::Config,
            "xcode",
            Some("ios"),
            Some("xcodebuild"),
            Some("mobile"),
        ),
    ),
    rule(
        When::Any(&[
            When::NameEndsWith(".uproject"),
            When::NameEndsWith(".uplugin"),
        ]),
        grouped(
            FileGroup::Config,
            "unreal",
            Some("unreal"),
            Some("unreal"),
            Some("game"),
        ),
    ),
    rule(
        When::NameEndsWith(".nomad"),
        grouped(
            FileGroup::Infra,
            "nomad",
            Some("nomad"),
            Some("nomad"),
            Some("infra"),
        ),
    ),
    rule(
        When::NameEndsWith(".tofu"),
        grouped(
            FileGroup::Infra,
            "opentofu",
            Some("opentofu"),
            Some("tofu"),
            Some("infra"),
        ),
    ),
    rule(
        When::NameStartsWith("dockerfile."),
        deploy("dockerfile", "docker", "docker", "container"),
    ),
    rule(
        When::NameEndsWith(".tf"),
        grouped(
            FileGroup::Infra,
            "terraform",
            Some("terraform"),
            Some("terraform"),
            Some("infra"),
        ),
    ),
    rule(
        When::NameEndsWith(".tfvars"),
        grouped(
            FileGroup::Infra,
            "terraform-vars",
            Some("terraform"),
            Some("terraform"),
            Some("infra"),
        ),
    ),
    rule(
        When::All(&[
            When::Any(&[
                When::NameEndsWith(".gradle"),
                When::NameEndsWith(".gradle.kts"),
            ]),
            When::Any(&[
                When::PathContains("android"),
                When::Path("settings.gradle"),
                When::Path("settings.gradle.kts"),
            ]),
        ]),
        grouped(
            FileGroup::Config,
            "android-gradle",
            Some("android"),
            Some("gradle"),
            Some("mobile"),
        ),
    ),
    rule(
        When::Name("androidmanifest.xml"),
        grouped(
            FileGroup::Config,
            "android-manifest",
            Some("android"),
            Some("android"),
            Some("mobile"),
        ),
    ),
    rule(
        When::PathStartsWith("src-tauri/"),
        grouped(
            FileGroup::Config,
            "tauri",
            Some("tauri"),
            Some("tauri"),
            Some("desktop"),
        ),
    ),
    rule(
        When::Any(&[
            When::PathStartsWith("ios/"),
            When::PathStartsWith("android/"),
        ]),
        grouped(
            FileGroup::Config,
            "mobile-project",
            Some("mobile"),
            None,
            Some("mobile"),
        ),
    ),
    rule(
        When::PathStartsWith(".github/workflows/"),
        ci("github-actions", "github-actions"),
    ),
    rule(
        When::All(&[
            When::PathStartsWith(".github/workflows/"),
            When::PathContains("codeql"),
        ]),
        grouped(
            FileGroup::Security,
            "codeql",
            None,
            Some("codeql"),
            Some("security"),
        ),
    ),
    rule(
        When::Any(&[
            When::Path(".circleci/config.yml"),
            When::Path(".circleci/config.yaml"),
        ]),
        ci("circleci", "circleci"),
    ),
    rule(
        When::PathStartsWith(".buildkite/"),
        ci("buildkite", "buildkite"),
    ),
    rule(
        When::Any(&[
            When::PathStartsWith(".fluxcd/"),
            When::PathContains("gotk-components"),
        ]),
        deploy("flux", "kubernetes", "flux", "deploy"),
    ),
    rule(
        When::Any(&[When::PathContains("argocd"), When::PathContains("argo-cd")]),
        deploy("argo-cd", "kubernetes", "argocd", "deploy"),
    ),
    rule(
        When::Any(&[
            When::PathStartsWith("roles/"),
            When::PathContains("/roles/"),
        ]),
        grouped(
            FileGroup::Infra,
            "ansible-role",
            Some("ansible"),
            Some("ansible"),
            Some("infra"),
        ),
    ),
    rule(
        When::Any(&[
            When::Name("playbook.yml"),
            When::Name("playbook.yaml"),
            When::Name("site.yml"),
            When::Name("site.yaml"),
        ]),
        grouped(
            FileGroup::Infra,
            "ansible-playbook",
            Some("ansible"),
            Some("ansible"),
            Some("infra"),
        ),
    ),
];

/// Every rule whose condition holds, in table order.
pub fn classify_file(rel: &str, name: &str) -> Vec<FileRuleDetection> {
    let path = rel.to_ascii_lowercase();
    let name = name.to_ascii_lowercase();
    RULES
        .iter()
        .filter(|rule| rule.when.matches(&path, &name))
        .map(|rule| rule.detection)
        .collect()
}

const fn package(
    kind: &'static str,
    ecosystem: &'static str,
    tool: &'static str,
) -> FileRuleDetection {
    grouped(
        FileGroup::Package,
        kind,
        Some(ecosystem),
        Some(tool),
        Some("source"),
    )
}

const fn lock(
    kind: &'static str,
    ecosystem: &'static str,
    tool: &'static str,
) -> FileRuleDetection {
    grouped(
        FileGroup::Lock,
        kind,
        Some(ecosystem),
        Some(tool),
        Some("source"),
    )
}

const fn ci(kind: &'static str, tool: &'static str) -> FileRuleDetection {
    grouped(FileGroup::Ci, kind, None, Some(tool), Some("ci"))
}

const fn deploy(
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

const fn config(
    kind: &'static str,
    ecosystem: &'static str,
    tool: &'static str,
) -> FileRuleDetection {
    grouped(
        FileGroup::Config,
        kind,
        Some(ecosystem),
        Some(tool),
        Some("source"),
    )
}

const fn grouped(
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

/// Every literal the rule table compares against, turned into a path.
///
/// The corpus is derived from the rules rather than guessed: each exact name,
/// suffix, name prefix, path, path prefix and path fragment appears here, plus
/// the compound cases that need the name and the location to agree. `rules.rs`
/// has no other unit tests and the integration tests assert only that a few
/// ecosystems and tools show up, so this is what pins the classification.
///
/// Lives beside the table rather than with the snapshot that renders it: a rule
/// added without a fixture is a rule nothing pins.
#[cfg(test)]
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A rule nothing can reach is dead weight that still reads as coverage.
    /// The snapshot corpus is built from these literals, so every rule should
    /// fire for at least one of its paths.
    #[test]
    fn every_rule_fires_for_the_snapshot_corpus() {
        let mut unreached = Vec::new();
        for (index, rule) in RULES.iter().enumerate() {
            let fired = PROJECT_RULE_FIXTURES.iter().any(|(rel, name)| {
                rule.when
                    .matches(&rel.to_ascii_lowercase(), &name.to_ascii_lowercase())
            });
            if !fired {
                unreached.push(format!("{index}:{}", rule.detection.kind));
            }
        }

        assert!(
            unreached.is_empty(),
            "rules no fixture reaches: {unreached:?}"
        );
    }
}
