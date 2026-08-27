//! Running a command in this process, with its output as a value.
//!
//! Every assertion about what `ah` prints used to cost a subprocess: the
//! built-in domains built their own `stdio` emitter after parsing, so nothing
//! above them could read what they rendered. `OutputSink` changed that, and
//! this is what a test drives instead of `assert_cmd`.
//!
//! What it covers is the whole of what the process-level suite was mostly
//! asserting: argv in, rendered stdout and stderr out, and the diagnostic a
//! failure produces. What it deliberately does not cover, and what therefore
//! still belongs in `tests/integration/`, is everything the process itself is:
//! the exit code, which stream the text reached, crash recovery, the event log,
//! `--json` error routing (chosen from a process-global), and the managed
//! service.
//!
//! Behind the `harness` feature, which the root crate enables for itself as a
//! dev-dependency, so a release build compiles none of this.

use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use ah_output::{GlobalOptions, OutputSink};
use ah_runtime::PluginManager;
use ah_secrets::VaultStore;
use tempfile::TempDir;

use crate::{
    cli::{CliParseResult, RuntimeCommand},
    error::AppError,
    plugin_settings::PluginSettings,
};

/// What one command did.
#[derive(Debug)]
pub struct Run {
    stdout: String,
    stderr: String,
    error: Option<AppError>,
}

impl Run {
    /// Everything the command rendered to standard output.
    #[must_use]
    pub fn stdout(&self) -> &str {
        &self.stdout
    }

    #[must_use]
    pub fn stderr(&self) -> &str {
        &self.stderr
    }

    /// The refusal, if the command produced one.
    #[must_use]
    pub fn error(&self) -> Option<&AppError> {
        self.error.as_ref()
    }

    /// Standard output parsed as JSON, for a command run with `--json`.
    ///
    /// # Panics
    ///
    /// If the command failed, or did not print JSON - both of which are the
    /// assertion the test meant to make, reported with what was printed.
    #[must_use]
    pub fn json(&self) -> serde_json::Value {
        let stdout = self.expect_success();
        serde_json::from_str(stdout)
            .unwrap_or_else(|error| panic!("stdout should be JSON ({error}): {stdout}"))
    }

    /// Standard output, having asserted the command succeeded.
    ///
    /// # Panics
    ///
    /// If the command was refused, quoting the diagnostic - a failure is worth
    /// more in the panic message than in a separate assertion.
    #[must_use]
    pub fn expect_success(&self) -> &str {
        assert!(
            self.error.is_none(),
            "the command should have succeeded, but reported {}: {}",
            self.code(),
            self.detail()
        );
        &self.stdout
    }

    /// The diagnostic code of the refusal.
    ///
    /// # Panics
    ///
    /// If the command succeeded.
    #[must_use]
    pub fn expect_failure(&self) -> &str {
        assert!(
            self.error.is_some(),
            "the command should have been refused, but printed: {}",
            self.stdout
        );
        self.code()
    }

    /// What a user would have seen on stderr, rendered without colour.
    ///
    /// The rendering is `AppError::print`'s, minus the stream and the process's
    /// own argv.
    #[must_use]
    pub fn diagnostic_text(&self, invocation: &[&str]) -> String {
        let invocation = invocation
            .iter()
            .map(|value| (*value).to_owned())
            .collect::<Vec<_>>();
        self.error
            .as_ref()
            .map(|error| {
                error.console_text(&invocation, ah_plugin_api::TextFormatter::with_color(false))
            })
            .unwrap_or_default()
    }

    /// The diagnostic code, or an empty string when the command succeeded.
    #[must_use]
    pub fn code(&self) -> &str {
        self.error.as_ref().map_or("", AppError::code)
    }

    /// The refusal's detail message, which is what a `contains` assertion on
    /// stderr was reading.
    #[must_use]
    pub fn detail(&self) -> String {
        self.error
            .as_ref()
            .map(AppError::detail_message)
            .unwrap_or_default()
    }
}

/// A host with its own configuration directory, vault and plugin registry.
///
/// One per test: the configuration directory is a temporary one that lives as
/// long as the harness, so two tests never read each other's state.
pub struct Harness {
    config_dir: TempDir,
    manager: Arc<PluginManager>,
    cwd: Option<PathBuf>,
}

impl Harness {
    /// # Panics
    ///
    /// If the temporary configuration directory cannot be created, which is a
    /// broken test environment rather than a failing assertion.
    #[must_use]
    pub fn new() -> Self {
        let config_dir = TempDir::new().expect("a temporary config dir should be created");
        let settings = Arc::new(Mutex::new(
            PluginSettings::load_from_path(config_dir.path().join("plugins.json"))
                .expect("absent plugin settings should load as defaults"),
        ));
        let vault = Arc::new(VaultStore::at(
            config_dir.path(),
            Arc::new(HarnessKeyProvider),
        ));
        // The registry the shipped binary builds, minus dynamic discovery: a
        // test must not depend on what happens to sit next to the test binary.
        let manager = Arc::new_cyclic(|weak| {
            let mut manager = PluginManager::new();
            manager.reserve_dynamic_domains(["ai", "plugins", "mcp", "secrets", "upgrade"]);
            for plugin in crate::plugins::builtins() {
                manager.register_builtin(plugin);
            }
            for plugin in crate::host_commands::builtins(
                weak.clone(),
                Arc::clone(&settings),
                Arc::clone(&vault),
            ) {
                manager.register_host_builtin(plugin);
            }
            manager
        });
        Self {
            config_dir,
            manager,
            cwd: None,
        }
    }

    /// Run every command as if it had been given `--cwd <dir>`.
    #[must_use]
    pub fn in_directory(mut self, cwd: impl AsRef<Path>) -> Self {
        self.cwd = Some(cwd.as_ref().to_path_buf());
        self
    }

    #[must_use]
    pub fn config_dir(&self) -> &Path {
        self.config_dir.path()
    }

    /// Run `ah <argv>` and collect what it rendered.
    ///
    /// The argv is the one a user types, without the program name. Parsing,
    /// routing and dispatch are the production ones; only the sink is the
    /// harness's.
    ///
    /// # Panics
    ///
    /// If the command is one the harness cannot run in process - anything that
    /// is not a plugin domain invocation. The panic names it rather than
    /// silently asserting about something else.
    pub fn run(&self, argv: &[&str]) -> Run {
        let mut raw = vec![OsString::from("ah")];
        if let Some(cwd) = &self.cwd {
            raw.push(OsString::from("--cwd"));
            raw.push(cwd.clone().into_os_string());
        }
        raw.extend(argv.iter().map(OsString::from));

        let plugins = self.manager.list_enabled_plugins();
        let parsed = match crate::cli::parse_runtime_command(raw, &plugins) {
            Ok(parsed) => parsed,
            Err(error) => return Run::refused(error),
        };
        let (domain, argv, options) = match parsed {
            CliParseResult::Command(RuntimeCommand::Invoke {
                domain,
                argv,
                options,
            }) => (domain, argv, options),
            CliParseResult::Command(other) => panic!(
                "the harness runs plugin domains; `{}` needs the process-level suite",
                command_label(&other)
            ),
            CliParseResult::ExitSuccess => {
                return Run {
                    stdout: String::new(),
                    stderr: String::new(),
                    error: None,
                };
            }
        };
        self.invoke(&domain, argv, &options)
    }

    fn invoke(&self, domain: &str, argv: Vec<String>, options: &GlobalOptions) -> Run {
        let (sink, captured) = OutputSink::capture();
        let outcome = self
            .manager
            .invoke_credentialed_into(
                domain,
                argv,
                options.to_wire(),
                &std::collections::BTreeMap::new(),
                &sink,
            )
            .map_err(crate::map_runtime_error)
            .and_then(|observation| {
                // A dynamic plugin returns its text in the response, so the
                // host renders that; a built-in has already written into the
                // sink and its response carries no message.
                crate::handle_response_into(observation.response, options, &sink)
            });
        Run {
            stdout: captured.stdout(),
            stderr: captured.stderr(),
            error: outcome.err(),
        }
    }
}

impl Default for Harness {
    fn default() -> Self {
        Self::new()
    }
}

impl Run {
    fn refused(error: AppError) -> Self {
        Self {
            stdout: String::new(),
            stderr: String::new(),
            error: Some(error),
        }
    }
}

fn command_label(command: &RuntimeCommand) -> &'static str {
    match command {
        RuntimeCommand::Invoke { .. } => "invoke",
        RuntimeCommand::McpServe { .. } => "mcp serve",
        RuntimeCommand::AiInfo { .. } => "ai info",
        RuntimeCommand::Ai { .. } => "ai",
        RuntimeCommand::PluginsList { .. } => "plugins list",
        RuntimeCommand::PluginsEnable { .. } => "plugins enable",
        RuntimeCommand::PluginsDisable { .. } => "plugins disable",
        RuntimeCommand::PluginsReset { .. } => "plugins reset",
        RuntimeCommand::Secrets { .. } => "secrets",
        RuntimeCommand::Upgrade { .. } => "upgrade",
    }
}

/// A vault key that exists only for the harness, so a test never reaches the
/// developer's keyring. Fixed rather than random, because nothing here is
/// protecting anything.
struct HarnessKeyProvider;

impl ah_secrets::KeyProvider for HarnessKeyProvider {
    fn load_or_create(&self) -> Result<[u8; 32], ah_secrets::VaultError> {
        Ok([0_u8; 32])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_domain_renders_text_into_the_harness_rather_than_the_process() {
        let temp = TempDir::new().expect("a temporary dir should be created");
        let file = temp.path().join("app.txt");
        std::fs::write(&file, "alpha\nbeta\n").expect("the file should be written");

        let run = Harness::new().run(&["file", "read", &file.to_string_lossy()]);

        assert_eq!(run.expect_success(), "alpha\nbeta\n");
        assert!(run.stderr().is_empty());
        assert!(run.error().is_none());
    }

    #[test]
    fn the_same_command_answers_json_when_asked() {
        let temp = TempDir::new().expect("a temporary dir should be created");
        let file = temp.path().join("app.txt");
        std::fs::write(&file, "alpha\n").expect("the file should be written");

        let payload = Harness::new()
            .run(&["--json", "file", "read", &file.to_string_lossy()])
            .json();

        assert_eq!(payload["command"], "file.read");
        assert_eq!(payload["content"], "alpha");
        assert_eq!(payload["line_count"], 1);
    }

    /// A refusal is a value too, with the code and the detail a `contains`
    /// assertion on stderr was reading.
    #[test]
    fn a_refusal_carries_its_code_and_its_detail() {
        let run = Harness::new().run(&["file", "read", "no-such-file.txt"]);

        assert_eq!(run.expect_failure(), "FILE_NOT_FOUND");
        assert!(
            run.detail().contains("no-such-file.txt"),
            "the detail should name the file: {}",
            run.detail()
        );
        assert!(run.stdout().is_empty());
    }

    /// `--cwd` is what the request directory is, and the harness applies it to
    /// every command so a test does not repeat it.
    #[test]
    fn a_relative_path_resolves_against_the_harness_directory() {
        let temp = TempDir::new().expect("a temporary dir should be created");
        std::fs::write(temp.path().join("app.txt"), "in place\n").expect("write");

        let run = Harness::new()
            .in_directory(temp.path())
            .run(&["file", "read", "app.txt"]);

        assert_eq!(run.expect_success(), "in place\n");
    }

    /// Two harnesses never share state, which is what lets tests run in
    /// parallel.
    #[test]
    fn each_harness_owns_its_configuration_directory() {
        let first = Harness::new();
        let second = Harness::new();

        assert_ne!(first.config_dir(), second.config_dir());
        assert!(first.config_dir().exists());
    }

    /// What the harness cannot run says so, rather than asserting about
    /// something else.
    #[test]
    #[should_panic(expected = "needs the process-level suite")]
    fn a_command_the_harness_cannot_run_names_itself() {
        let _ = Harness::new().run(&["plugins", "list"]);
    }
}
