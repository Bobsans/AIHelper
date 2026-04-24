use assert_cmd::Command;
use predicates::str::contains;
use tempfile::TempDir;

fn task_echo_command() -> &'static str {
    if cfg!(target_os = "windows") {
        "Write-Output task-ok"
    } else {
        "echo task-ok"
    }
}

#[test]
fn task_save_and_list_json() {
    let temp_dir = TempDir::new().expect("temporary dir should be created");
    let cwd = temp_dir.path().to_string_lossy().to_string();

    let mut save_cmd = Command::cargo_bin("ah").expect("binary should compile");
    save_cmd
        .args(["--cwd", &cwd, "task", "save", "hello", task_echo_command()])
        .assert()
        .success()
        .stdout(contains("saved task 'hello'"));

    let mut list_cmd = Command::cargo_bin("ah").expect("binary should compile");
    list_cmd
        .args(["--json", "--cwd", &cwd, "task", "list"])
        .assert()
        .success()
        .stdout(contains("\"command\": \"task.list\""))
        .stdout(contains("\"name\": \"hello\""));
}

#[test]
fn task_run_executes_saved_command() {
    let temp_dir = TempDir::new().expect("temporary dir should be created");
    let cwd = temp_dir.path().to_string_lossy().to_string();

    let mut save_cmd = Command::cargo_bin("ah").expect("binary should compile");
    save_cmd
        .args(["--cwd", &cwd, "task", "save", "echo", task_echo_command()])
        .assert()
        .success();

    let mut run_cmd = Command::cargo_bin("ah").expect("binary should compile");
    run_cmd
        .args(["--cwd", &cwd, "task", "run", "echo"])
        .assert()
        .success()
        .stdout(contains("task-ok"));
}

#[test]
fn task_run_unknown_task_fails() {
    let temp_dir = TempDir::new().expect("temporary dir should be created");
    let cwd = temp_dir.path().to_string_lossy().to_string();

    let mut run_cmd = Command::cargo_bin("ah").expect("binary should compile");
    run_cmd
        .args(["--cwd", &cwd, "task", "run", "missing"])
        .assert()
        .failure()
        .stderr(contains("task not found: missing"));
}
