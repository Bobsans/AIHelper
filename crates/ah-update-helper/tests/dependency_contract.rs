use std::{path::Path, process::Command};

const MANIFEST: &str = include_str!("../Cargo.toml");
const SOURCE: &str = include_str!("../src/main.rs");

#[test]
fn helper_has_no_network_plugin_or_general_cli_dependencies() {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let output = Command::new("cargo")
        .args([
            "tree",
            "-p",
            "ah-update-helper",
            "--edges",
            "normal",
            "--prefix",
            "none",
            "--locked",
        ])
        .current_dir(workspace)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "cargo tree failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let dependency_tree = String::from_utf8(output.stdout).unwrap();
    for forbidden in [
        "ah-mcp ",
        "ah-plugin-api ",
        "ah-runtime ",
        "clap ",
        "libloading ",
        "reqwest ",
        "tokio ",
    ] {
        assert!(
            !dependency_tree
                .lines()
                .any(|line| line.starts_with(forbidden)),
            "found forbidden runtime dependency: {forbidden}"
        );
    }
    for forbidden in ["std::net", "reqwest", "ah_runtime", "libloading"] {
        assert!(
            !SOURCE.contains(forbidden),
            "found forbidden helper source token: {forbidden}"
        );
    }
    assert!(MANIFEST.contains("publish = false"));
}
