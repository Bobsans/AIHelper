const CORE_SOURCES: &[&str] = &[
    include_str!("../src/lib.rs"),
    include_str!("../src/check.rs"),
    include_str!("../src/error.rs"),
    include_str!("../src/release.rs"),
    include_str!("../src/trust.rs"),
];

#[test]
fn updater_core_contains_no_private_signing_material_contract() {
    let source = CORE_SOURCES.join("\n").to_ascii_lowercase();
    for forbidden in [
        "aihelper_release_ed25519_seed_b64url",
        "release_ed25519_seed",
        "private_key",
    ] {
        assert!(
            !source.contains(forbidden),
            "found forbidden token: {forbidden}"
        );
    }
}
