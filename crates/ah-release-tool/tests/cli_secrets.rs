use assert_cmd::Command;

#[test]
fn missing_or_malformed_secrets_are_not_echoed() {
    let secret = "this-value-must-never-appear-in-output";
    let mut command = Command::cargo_bin("ah-release-tool").unwrap();
    let output = command
        .args([
            "sign-release",
            "--assets-dir",
            "missing-assets",
            "--output-dir",
            "missing-output",
            "--repository",
            "example/aihelper",
            "--tag",
            "v1.2.0",
            "--minimum-updater-version",
            "1.0.0",
        ])
        .env("AIHELPER_RELEASE_ED25519_SEED_B64URL", secret)
        .env("AIHELPER_RELEASE_ED25519_PUBLIC_KEY_B64URL", secret)
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(!stdout.contains(secret));
    assert!(!stderr.contains(secret));
    assert!(stderr.contains("release signing configuration is invalid"));
}
