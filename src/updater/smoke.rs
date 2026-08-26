use std::{collections::BTreeSet, fs, path::Path};

use ah_plugin_api::AH_PLUGIN_ABI_VERSION;
use ah_updater_core::{
    FilePurpose, ReleaseManifest, UpdateHelperSelfCheckV1, UpdaterError, UpdaterErrorCode,
    VerifiedReleaseV1,
};
use serde::Deserialize;

use super::candidate::PreparedCandidate;

const REQUIRED_BUILTINS: [(&str, &str); 8] = [
    ("ctx", "builtin-ctx"),
    ("file", "builtin-file"),
    ("git", "builtin-git"),
    ("http", "builtin-http"),
    ("project", "builtin-project"),
    ("run", "builtin-run"),
    ("search", "builtin-search"),
    ("task", "builtin-task"),
];

#[derive(Debug)]
pub(crate) struct SmokeCheckedCandidate {
    prepared: PreparedCandidate,
}

impl SmokeCheckedCandidate {
    pub(crate) fn root(&self) -> &Path {
        self.prepared.root()
    }

    pub(crate) fn verified_release(&self) -> &VerifiedReleaseV1 {
        self.prepared.verified_release()
    }
}

pub(crate) fn run_offline_smoke(
    prepared: PreparedCandidate,
    runner: &dyn SmokeRunner,
) -> Result<SmokeCheckedCandidate, UpdaterError> {
    perform_offline_smoke(
        runner,
        prepared.root(),
        prepared.staging_root(),
        prepared.verified_release().manifest(),
    )?;
    Ok(SmokeCheckedCandidate { prepared })
}

fn perform_offline_smoke(
    runner: &dyn SmokeRunner,
    candidate_root: &Path,
    staging_root: &Path,
    manifest: &ReleaseManifest,
) -> Result<(), UpdaterError> {
    let executable = main_executable(manifest)?;
    let helper = update_helper(manifest)?;
    let program = candidate_root.join(executable);
    let config_dir = staging_root.join("smoke-config");
    fs::create_dir(&config_dir)
        .map_err(|_| candidate("failed to create isolated candidate smoke configuration"))?;

    let version = runner.run(SmokeRequest {
        program: &program,
        arguments: &["--version"],
        cwd: candidate_root,
        config_dir: &config_dir,
    })?;
    validate_completion(&version)?;
    validate_version_output(&version.stdout, &manifest.release.version)?;

    let helper_check = runner.run(SmokeRequest {
        program: &candidate_root.join(helper),
        arguments: &["--self-check"],
        cwd: candidate_root,
        config_dir: &config_dir,
    })?;
    validate_completion(&helper_check)?;
    validate_helper_output(&helper_check.stdout, manifest)?;

    let catalog = runner.run(SmokeRequest {
        program: &program,
        arguments: &["--json", "plugins", "list"],
        cwd: candidate_root,
        config_dir: &config_dir,
    })?;
    validate_completion(&catalog)?;
    validate_catalog(&catalog.stdout, manifest)
}

fn update_helper(manifest: &ReleaseManifest) -> Result<&str, UpdaterError> {
    let mut helpers = manifest
        .files
        .iter()
        .filter(|file| file.purpose == FilePurpose::UpdateHelper);
    let helper = helpers
        .next()
        .ok_or_else(|| candidate("signed candidate does not contain an update helper"))?;
    if helpers.next().is_some() {
        return Err(candidate(
            "signed candidate contains more than one update helper",
        ));
    }
    if !manifest
        .required
        .executables
        .iter()
        .any(|path| path == &helper.path)
    {
        return Err(candidate(
            "signed candidate update helper is not required by the manifest",
        ));
    }
    Ok(&helper.path)
}

fn main_executable(manifest: &ReleaseManifest) -> Result<&str, UpdaterError> {
    let mut executables = manifest
        .files
        .iter()
        .filter(|file| file.purpose == FilePurpose::Executable);
    let executable = executables
        .next()
        .ok_or_else(|| candidate("signed candidate does not contain a main executable"))?;
    if executables.next().is_some() {
        return Err(candidate(
            "signed candidate contains more than one main executable",
        ));
    }
    if !manifest
        .required
        .executables
        .iter()
        .any(|path| path == &executable.path)
    {
        return Err(candidate(
            "signed candidate main executable is not required by the manifest",
        ));
    }
    Ok(&executable.path)
}

fn validate_completion(output: &SmokeProcessOutput) -> Result<(), UpdaterError> {
    if output.timed_out {
        return Err(candidate("candidate smoke command timed out"));
    }
    if output.stdout_truncated || output.stderr_truncated {
        return Err(candidate(
            "candidate smoke command exceeded the output limit",
        ));
    }
    if output.exit_code != Some(0) {
        return Err(candidate(
            "candidate smoke command did not exit successfully",
        ));
    }
    if !output.stderr.is_empty() {
        return Err(candidate(
            "candidate smoke command wrote unexpected diagnostic output",
        ));
    }
    Ok(())
}

fn validate_version_output(output: &[u8], expected_version: &str) -> Result<(), UpdaterError> {
    let output = std::str::from_utf8(output)
        .map_err(|_| candidate("candidate version output is not valid UTF-8"))?;
    let output = output
        .strip_suffix("\r\n")
        .or_else(|| output.strip_suffix('\n'))
        .unwrap_or(output);
    if output.contains(['\r', '\n']) || output != format!("ah {expected_version}") {
        return Err(candidate(
            "candidate executable version does not match the signed manifest",
        ));
    }
    Ok(())
}

fn validate_helper_output(output: &[u8], manifest: &ReleaseManifest) -> Result<(), UpdaterError> {
    let response = serde_json::from_slice::<UpdateHelperSelfCheckV1>(output)
        .map_err(|_| candidate("update helper self-check is not valid JSON"))?;
    response.validate(
        &manifest.release.version,
        &manifest.release.target,
        &manifest.release.architecture,
    )
}

fn validate_catalog(output: &[u8], manifest: &ReleaseManifest) -> Result<(), UpdaterError> {
    let entries = serde_json::from_slice::<Vec<PluginCatalogEntry>>(output)
        .map_err(|_| candidate("candidate plugin catalog is not valid JSON"))?;
    let mut domains = BTreeSet::new();
    let mut plugin_names = BTreeSet::new();

    for entry in &entries {
        if entry.domain.is_empty()
            || entry.plugin_name.is_empty()
            || !domains.insert(entry.domain.as_str())
            || !plugin_names.insert(entry.plugin_name.as_str())
        {
            return Err(candidate(
                "candidate plugin catalog contains duplicate or empty identities",
            ));
        }
        if !matches!(entry.source.as_str(), "builtin" | "dynamic") {
            return Err(candidate(
                "candidate plugin catalog contains an unknown plugin source",
            ));
        }
        if entry.state != "enabled" {
            return Err(candidate(
                "candidate plugin catalog contains a disabled plugin",
            ));
        }
        if entry.abi_version != AH_PLUGIN_ABI_VERSION {
            return Err(candidate(
                "candidate plugin catalog contains an incompatible ABI",
            ));
        }
        if !entry.mcp_exposed || entry.mcp_omission_reason.is_some() {
            return Err(candidate(
                "candidate plugin catalog contains an invalid typed command catalog",
            ));
        }
    }

    for (domain, plugin_name) in REQUIRED_BUILTINS {
        if !entries.iter().any(|entry| {
            entry.source == "builtin" && entry.domain == domain && entry.plugin_name == plugin_name
        }) {
            return Err(candidate(
                "candidate plugin catalog is missing a required built-in plugin",
            ));
        }
    }

    let expected_dynamic = manifest
        .files
        .iter()
        .filter(|file| file.purpose == FilePurpose::Plugin)
        .count();
    let observed_dynamic = entries
        .iter()
        .filter(|entry| entry.source == "dynamic")
        .count();
    if observed_dynamic != expected_dynamic {
        return Err(candidate(
            "candidate dynamic plugin catalog does not match the signed manifest",
        ));
    }

    Ok(())
}

#[derive(Debug, Deserialize)]
struct PluginCatalogEntry {
    plugin_name: String,
    domain: String,
    abi_version: u32,
    source: String,
    state: String,
    mcp_exposed: bool,
    #[serde(default)]
    mcp_omission_reason: Option<String>,
}

/// One command the smoke check wants run against the candidate.
pub(crate) struct SmokeRequest<'a> {
    pub(crate) program: &'a Path,
    pub(crate) arguments: &'a [&'a str],
    pub(crate) cwd: &'a Path,
    /// An empty directory, so the candidate answers from a clean configuration
    /// rather than the running installation's.
    pub(crate) config_dir: &'a Path,
}

#[derive(Debug)]
pub(crate) struct SmokeProcessOutput {
    pub(crate) exit_code: Option<i32>,
    pub(crate) timed_out: bool,
    pub(crate) stdout: Vec<u8>,
    pub(crate) stderr: Vec<u8>,
    pub(crate) stdout_truncated: bool,
    pub(crate) stderr_truncated: bool,
}

/// Running a bounded, cancellable child process.
///
/// A port rather than a call, because the implementation is the CLI's process
/// runner and the smoke check must not know that. The truncation flags are part
/// of the contract: a candidate that floods stdout has to fail the check rather
/// than be believed.
pub(crate) trait SmokeRunner {
    /// # Errors
    ///
    /// [`UpdaterError`] when the process cannot be launched or waited on. A
    /// process that runs and fails is a successful call with a failing output.
    fn run(&self, request: SmokeRequest<'_>) -> Result<SmokeProcessOutput, UpdaterError>;
}

fn candidate(detail: &'static str) -> UpdaterError {
    UpdaterError::new(UpdaterErrorCode::Candidate, detail)
}

#[cfg(test)]
mod tests {
    use std::{cell::RefCell, collections::VecDeque, fs, path::PathBuf};

    use ah_release_manifest::{
        ArchiveMetadata, ManagedFile, ReleaseMetadata, RequiredFiles, SignatureAlgorithm,
        SigningMetadata,
    };
    use serde_json::{Value, json};
    use tempfile::TempDir;

    use super::*;

    #[test]
    fn validates_version_catalog_and_isolated_configuration() {
        let manifest = manifest();
        let version = success(b"ah 1.2.0\n");
        let helper = success(&helper_output());
        let catalog = success(&serde_json::to_vec(&valid_catalog()).unwrap());
        let runner = FakeRunner::new([Ok(version), Ok(helper), Ok(catalog)]);
        let candidate = TempDir::new().unwrap();
        let staging = TempDir::new().unwrap();
        let user_config = TempDir::new().unwrap();
        let sentinel = user_config.path().join("sentinel.txt");
        fs::write(&sentinel, b"unchanged").unwrap();

        perform_offline_smoke(&runner, candidate.path(), staging.path(), &manifest).unwrap();

        let calls = runner.calls.borrow();
        assert_eq!(calls.len(), 3);
        assert_eq!(calls[0].program, candidate.path().join("ah.exe"));
        assert_eq!(calls[0].arguments, ["--version"]);
        assert_eq!(
            calls[1].program,
            candidate.path().join("ah-update-helper.exe")
        );
        assert_eq!(calls[1].arguments, ["--self-check"]);
        assert_eq!(calls[2].arguments, ["--json", "plugins", "list"]);
        for call in calls.iter() {
            assert_eq!(call.cwd, candidate.path());
            assert_eq!(call.config_dir, staging.path().join("smoke-config"));
            assert!(call.config_dir.is_dir());
        }
        assert_eq!(fs::read(sentinel).unwrap(), b"unchanged");
    }

    #[test]
    fn rejects_launch_timeout_exit_truncation_and_stderr_failures() {
        let launch_error = Err(candidate("simulated launch failure"));
        assert_rejected([launch_error]);

        let mut timed_out = success(b"ah 1.2.0\n");
        timed_out.timed_out = true;
        assert_rejected([Ok(timed_out)]);

        let mut failed = success(b"ah 1.2.0\n");
        failed.exit_code = Some(7);
        assert_rejected([Ok(failed)]);

        let mut truncated = success(b"ah 1.2.0\n");
        truncated.stdout_truncated = true;
        assert_rejected([Ok(truncated)]);

        let mut diagnostic = success(b"ah 1.2.0\n");
        diagnostic.stderr = b"warning".to_vec();
        assert_rejected([Ok(diagnostic)]);
    }

    #[test]
    fn rejects_invalid_or_mismatched_version_output() {
        for output in [
            b"ah 9.9.9\n".as_slice(),
            b"ah 1.2.0\nextra\n".as_slice(),
            b"\xff".as_slice(),
        ] {
            assert_rejected([Ok(success(output))]);
        }
    }

    #[test]
    fn rejects_malformed_or_incompatible_helper_protocol() {
        assert_helper_rejected(b"not-json");

        for response in [
            UpdateHelperSelfCheckV1 {
                protocol_version: 2,
                ..valid_helper()
            },
            UpdateHelperSelfCheckV1 {
                helper_version: "9.9.9".to_owned(),
                ..valid_helper()
            },
            UpdateHelperSelfCheckV1 {
                target: "x86_64-unknown-linux-gnu".to_owned(),
                ..valid_helper()
            },
        ] {
            assert_helper_rejected(&serde_json::to_vec(&response).unwrap());
        }
    }

    #[test]
    fn rejects_malformed_missing_duplicate_disabled_abi_and_catalog_plugins() {
        assert_catalog_rejected(b"not-json");

        let missing_dynamic = valid_catalog()
            .into_iter()
            .filter(|entry| entry["source"] != "dynamic")
            .collect::<Vec<_>>();
        assert_catalog_value_rejected(missing_dynamic);

        let missing_builtin = valid_catalog()
            .into_iter()
            .filter(|entry| entry["domain"] != "ctx")
            .collect::<Vec<_>>();
        assert_catalog_value_rejected(missing_builtin);

        let mut duplicate = valid_catalog();
        duplicate.push(duplicate[0].clone());
        assert_catalog_value_rejected(duplicate);

        let mut disabled = valid_catalog();
        disabled.last_mut().unwrap()["state"] = json!("disabled");
        assert_catalog_value_rejected(disabled);

        let mut incompatible = valid_catalog();
        incompatible.last_mut().unwrap()["abi_version"] = json!(AH_PLUGIN_ABI_VERSION + 1);
        assert_catalog_value_rejected(incompatible);

        let mut invalid_catalog = valid_catalog();
        invalid_catalog.last_mut().unwrap()["mcp_exposed"] = json!(false);
        invalid_catalog.last_mut().unwrap()["mcp_omission_reason"] = json!("missing");
        assert_catalog_value_rejected(invalid_catalog);
    }

    #[test]
    fn rejects_manifest_without_one_required_main_executable() {
        let mut missing = manifest();
        missing.files.remove(0);
        assert_manifest_rejected(missing);

        let mut multiple = manifest();
        multiple.files.push(ManagedFile {
            path: "tools/other.exe".to_owned(),
            size: 1,
            sha256: "3".repeat(64),
            purpose: FilePurpose::Executable,
        });
        assert_manifest_rejected(multiple);

        let mut optional = manifest();
        optional
            .required
            .executables
            .retain(|path| path != "ah.exe");
        assert_manifest_rejected(optional);
    }

    #[test]
    fn rejects_manifest_without_one_required_update_helper() {
        let mut missing = manifest();
        missing
            .files
            .retain(|file| file.purpose != FilePurpose::UpdateHelper);
        assert_manifest_rejected(missing);

        let mut multiple = manifest();
        multiple.files.push(ManagedFile {
            path: "tools/other-helper.exe".to_owned(),
            size: 1,
            sha256: "4".repeat(64),
            purpose: FilePurpose::UpdateHelper,
        });
        assert_manifest_rejected(multiple);

        let mut optional = manifest();
        optional
            .required
            .executables
            .retain(|path| path != "ah-update-helper.exe");
        assert_manifest_rejected(optional);
    }

    fn assert_catalog_value_rejected(catalog: Vec<Value>) {
        assert_catalog_rejected(&serde_json::to_vec(&catalog).unwrap());
    }

    fn assert_catalog_rejected(catalog: &[u8]) {
        assert_rejected([
            Ok(success(b"ah 1.2.0\n")),
            Ok(success(&helper_output())),
            Ok(success(catalog)),
        ]);
    }

    fn assert_helper_rejected(output: &[u8]) {
        assert_rejected([Ok(success(b"ah 1.2.0\n")), Ok(success(output))]);
    }

    fn assert_manifest_rejected(manifest: ReleaseManifest) {
        let runner = FakeRunner::new([]);
        let candidate = TempDir::new().unwrap();
        let staging = TempDir::new().unwrap();
        let error = perform_offline_smoke(&runner, candidate.path(), staging.path(), &manifest)
            .unwrap_err();
        assert_eq!(error.code(), UpdaterErrorCode::Candidate);
        assert!(runner.calls.borrow().is_empty());
    }

    fn assert_rejected<const N: usize>(outputs: [Result<SmokeProcessOutput, UpdaterError>; N]) {
        let runner = FakeRunner::new(outputs);
        let candidate = TempDir::new().unwrap();
        let staging = TempDir::new().unwrap();
        let error = perform_offline_smoke(&runner, candidate.path(), staging.path(), &manifest())
            .unwrap_err();
        assert_eq!(error.code(), UpdaterErrorCode::Candidate);
    }

    fn success(stdout: &[u8]) -> SmokeProcessOutput {
        SmokeProcessOutput {
            exit_code: Some(0),
            timed_out: false,
            stdout: stdout.to_vec(),
            stderr: Vec::new(),
            stdout_truncated: false,
            stderr_truncated: false,
        }
    }

    fn valid_catalog() -> Vec<Value> {
        let mut entries = REQUIRED_BUILTINS
            .iter()
            .map(|(domain, plugin_name)| plugin(domain, plugin_name, "builtin"))
            .collect::<Vec<_>>();
        entries.push(plugin("github", "external-github", "dynamic"));
        entries
    }

    fn helper_output() -> Vec<u8> {
        serde_json::to_vec(&valid_helper()).unwrap()
    }

    fn valid_helper() -> UpdateHelperSelfCheckV1 {
        UpdateHelperSelfCheckV1::new("1.2.0", "x86_64-pc-windows-msvc", "x86_64")
    }

    fn plugin(domain: &str, plugin_name: &str, source: &str) -> Value {
        json!({
            "plugin_name": plugin_name,
            "domain": domain,
            "abi_version": AH_PLUGIN_ABI_VERSION,
            "source": source,
            "state": "enabled",
            "mcp_exposed": true
        })
    }

    fn manifest() -> ReleaseManifest {
        ReleaseManifest {
            schema_version: 1,
            release: ReleaseMetadata {
                version: "1.2.0".to_owned(),
                target: "x86_64-pc-windows-msvc".to_owned(),
                architecture: "x86_64".to_owned(),
            },
            archive: ArchiveMetadata {
                url: "https://example.test/ah-windows-x64.zip".to_owned(),
                size: 2,
                sha256: "0".repeat(64),
            },
            minimum_updater_version: "1.1.0".to_owned(),
            signing: SigningMetadata {
                key_id: "ed25519-sha256-test".to_owned(),
                algorithm: SignatureAlgorithm::Ed25519,
            },
            files: vec![
                ManagedFile {
                    path: "ah-update-helper.exe".to_owned(),
                    size: 1,
                    sha256: "3".repeat(64),
                    purpose: FilePurpose::UpdateHelper,
                },
                ManagedFile {
                    path: "ah.exe".to_owned(),
                    size: 1,
                    sha256: "1".repeat(64),
                    purpose: FilePurpose::Executable,
                },
                ManagedFile {
                    path: "plugins/ah-plugin-github.dll".to_owned(),
                    size: 1,
                    sha256: "2".repeat(64),
                    purpose: FilePurpose::Plugin,
                },
            ],
            required: RequiredFiles {
                executables: vec!["ah-update-helper.exe".to_owned(), "ah.exe".to_owned()],
                plugins: vec!["plugins/ah-plugin-github.dll".to_owned()],
            },
        }
    }

    struct FakeRunner {
        outputs: RefCell<VecDeque<Result<SmokeProcessOutput, UpdaterError>>>,
        calls: RefCell<Vec<RecordedCall>>,
    }

    impl FakeRunner {
        fn new(
            outputs: impl IntoIterator<Item = Result<SmokeProcessOutput, UpdaterError>>,
        ) -> Self {
            Self {
                outputs: RefCell::new(outputs.into_iter().collect()),
                calls: RefCell::new(Vec::new()),
            }
        }
    }

    impl SmokeRunner for FakeRunner {
        fn run(&self, request: SmokeRequest<'_>) -> Result<SmokeProcessOutput, UpdaterError> {
            self.calls.borrow_mut().push(RecordedCall {
                program: request.program.to_path_buf(),
                arguments: request
                    .arguments
                    .iter()
                    .map(|argument| (*argument).to_owned())
                    .collect(),
                cwd: request.cwd.to_path_buf(),
                config_dir: request.config_dir.to_path_buf(),
            });
            self.outputs
                .borrow_mut()
                .pop_front()
                .expect("test runner output should be configured")
        }
    }

    struct RecordedCall {
        program: PathBuf,
        arguments: Vec<String>,
        cwd: PathBuf,
        config_dir: PathBuf,
    }
}
