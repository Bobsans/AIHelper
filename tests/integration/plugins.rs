use std::fs;

use assert_cmd::Command;
use predicates::str::contains;
use tempfile::TempDir;

#[test]
fn plugins_list_reports_builtin_domains() {
    let mut cmd = Command::cargo_bin("ah").expect("binary should compile");
    cmd.args(["plugins", "list", "--json"])
        .assert()
        .success()
        .stdout(contains("\"domain\": \"file\""))
        .stdout(contains("\"domain\": \"search\""))
        .stdout(contains("\"domain\": \"ctx\""))
        .stdout(contains("\"domain\": \"git\""))
        .stdout(contains("\"domain\": \"task\""));
}

#[test]
fn startup_skips_invalid_dynamic_plugin_and_runs_builtin_command() {
    let temp_dir = TempDir::new().expect("temporary dir should be created");
    let source_binary = assert_cmd::cargo::cargo_bin("ah");
    let binary_name = if cfg!(windows) { "ah.exe" } else { "ah" };
    let copied_binary = temp_dir.path().join(binary_name);
    fs::copy(&source_binary, &copied_binary).expect("binary should be copied for isolated runtime");

    let plugins_dir = temp_dir.path().join("plugins");
    fs::create_dir_all(&plugins_dir).expect("plugin directory should be created");
    let extension = if cfg!(windows) {
        "dll"
    } else if cfg!(target_os = "macos") {
        "dylib"
    } else {
        "so"
    };
    fs::write(
        plugins_dir.join(format!("broken.{extension}")),
        "not a dynamic library",
    )
    .expect("broken plugin file should be written");

    fs::write(temp_dir.path().join("sample.txt"), "alpha\nbeta\n")
        .expect("sample file should be written");

    let cwd = temp_dir.path().to_string_lossy().to_string();
    let mut cmd = Command::new(copied_binary);
    cmd.args(["--cwd", &cwd, "file", "head", "sample.txt", "--lines", "1"])
        .assert()
        .success()
        .stdout(contains("alpha"));
}
