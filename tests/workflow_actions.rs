use std::{fs, path::Path};

#[test]
fn third_party_actions_are_pinned_to_full_commit_shas() {
    let workflows = Path::new(env!("CARGO_MANIFEST_DIR")).join(".github/workflows");
    for entry in fs::read_dir(workflows).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().and_then(|value| value.to_str()) != Some("yml") {
            continue;
        }
        let source = fs::read_to_string(&path).unwrap();
        for (index, line) in source.lines().enumerate() {
            let Some(action) = line.trim().strip_prefix("uses: ") else {
                continue;
            };
            if action.starts_with("./") {
                continue;
            }
            let revision = action
                .split_whitespace()
                .next()
                .and_then(|value| value.rsplit_once('@'))
                .map(|(_, revision)| revision)
                .unwrap_or_default();
            assert!(
                revision.len() == 40
                    && revision
                        .bytes()
                        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
                "{}:{} has an unpinned action: {}",
                path.display(),
                index + 1,
                line.trim()
            );
        }
    }
}
