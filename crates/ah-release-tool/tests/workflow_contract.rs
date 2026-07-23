use serde_yaml::{Mapping, Value};

const WORKFLOW: &str = include_str!("../../../.github/workflows/release.yml");

#[test]
fn dispatch_validates_without_referencing_signing_or_publication() {
    let jobs = jobs();
    let build = job(jobs, "build");
    let validate = job(jobs, "validate");
    assert!(!serialized(build).contains("AIHELPER_RELEASE_ED25519"));
    assert!(!serialized(validate).contains("AIHELPER_RELEASE_ED25519"));
    assert!(job_if(jobs, "sign").contains("github.event_name == 'release'"));
    assert!(job_if(jobs, "publish").contains("github.event_name == 'release'"));
    assert_eq!(job_needs(jobs, "validate"), vec!["build"]);
}

#[test]
fn signing_is_protected_and_publication_has_no_private_key() {
    let jobs = jobs();
    let sign = job(jobs, "sign");
    let publish = job(jobs, "publish");
    assert_eq!(job_needs(jobs, "sign"), vec!["validate"]);
    assert_eq!(job_needs(jobs, "publish"), vec!["sign"]);
    assert_eq!(
        sign["environment"]["name"].as_str(),
        Some("release-signing")
    );
    let sign_text = serialized(sign);
    assert!(sign_text.contains("secrets.AIHELPER_RELEASE_ED25519_SEED_B64URL"));
    assert!(sign_text.contains("vars.AIHELPER_RELEASE_ED25519_PUBLIC_KEY_B64URL"));
    let signing_step = sign["steps"]
        .as_sequence()
        .expect("sign job must define steps")
        .iter()
        .find(|step| step["name"].as_str() == Some("Generate and sign release assets"))
        .expect("sign job must generate signed release assets");
    assert_eq!(
        signing_step["env"]["AIHELPER_MINIMUM_UPDATER_VERSION"].as_str(),
        Some("1.1.0")
    );
    assert!(
        signing_step["run"]
            .as_str()
            .unwrap_or_default()
            .contains("--minimum-updater-version")
    );
    assert!(!serialized(publish).contains("AIHELPER_RELEASE_ED25519"));
    assert_eq!(publish["permissions"]["contents"].as_str(), Some("write"));
}

#[test]
fn publish_step_lists_exactly_three_complete_triplets() {
    let publish = serialized(job(jobs(), "publish"));
    for platform in ["linux-x64", "macos-arm64", "windows-x64"] {
        for suffix in [".zip", ".manifest.json", ".manifest.sig"] {
            assert!(publish.contains(&format!("ah-{platform}{suffix}")));
        }
    }
    assert!(publish.contains("fail_on_unmatched_files: true"));
    assert!(publish.contains("overwrite_files: false"));
}

#[test]
fn windows_archive_builds_packages_and_smokes_update_helper() {
    let build = serialized(job(jobs(), "build"));
    assert!(build.contains("cargo build --release --locked -p ah-update-helper"));
    assert!(build.contains("target/release/ah-update-helper.exe"));
    assert!(build.contains("dist/ah-update-helper.exe"));
    assert!(build.contains("scripts/release_smoke.py"));
}

fn jobs() -> &'static Mapping {
    let workflow = serde_yaml::from_str::<Value>(WORKFLOW).expect("release workflow must be YAML");
    Box::leak(Box::new(workflow))["jobs"]
        .as_mapping()
        .expect("release workflow must define jobs")
}

fn job<'a>(jobs: &'a Mapping, name: &str) -> &'a Value {
    jobs.get(Value::String(name.to_owned()))
        .unwrap_or_else(|| panic!("release workflow must define '{name}'"))
}

fn job_if(jobs: &Mapping, name: &str) -> String {
    job(jobs, name)["if"]
        .as_str()
        .unwrap_or_default()
        .to_owned()
}

fn job_needs(jobs: &Mapping, name: &str) -> Vec<String> {
    job(jobs, name)["needs"]
        .as_sequence()
        .expect("job needs must be a sequence")
        .iter()
        .map(|value| value.as_str().expect("need must be a job name").to_owned())
        .collect()
}

fn serialized(value: &Value) -> String {
    serde_yaml::to_string(value).expect("workflow section must serialize")
}
