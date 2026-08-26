//! One shape for every command module, checked rather than described.
//!
//! The intended layout is `<domain>.rs` beside a `<domain>/` directory holding
//! `domain.rs` (pure), `io.rs` (effects) and `output.rs` (rendering). It had
//! drifted twice over: two domains put the same two files under an `adapters/`
//! subdirectory, and eight modules carried an alias module that made the two
//! layouts *look* identical instead of making them identical.
//!
//! Prose in a contributing guide did not hold that line. This does.

#[cfg(test)]
mod tests {
    use std::{
        collections::BTreeSet,
        fs,
        path::{Path, PathBuf},
    };

    /// Files under `commands/` that are not command modules at all, so the
    /// layout does not apply to them.
    const FLAT_MODULES: &[&str] = &["layout", "mod"];

    /// Files a command directory may hold beyond the three required ones, each
    /// because it is a genuine third concern rather than a layering variant.
    const EXTRA_FILES: &[(&str, &str)] = &[
        // Symbol extraction: a pure library with no effects of its own, used
        // only by this domain.
        ("ctx", "symbols.rs"),
        // The classification table, which is data rather than logic.
        ("project", "rules.rs"),
        // Windows job objects, which only one platform compiles.
        ("run", "windows_job.rs"),
    ];

    const REQUIRED: &[&str] = &["domain.rs", "io.rs", "output.rs"];

    fn commands_dir() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src")
            .join("commands")
    }

    #[test]
    fn every_command_module_has_the_same_shape() {
        let root = commands_dir();
        let mut problems = Vec::new();

        for entry in fs::read_dir(&root).expect("commands directory should exist") {
            let path = entry.expect("directory entry should be readable").path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("rs") {
                continue;
            }
            let stem = path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .expect("module file should have a name")
                .to_owned();
            if FLAT_MODULES.contains(&stem.as_str()) {
                continue;
            }

            let directory = root.join(&stem);
            if !directory.is_dir() {
                problems.push(format!("{stem}: no {stem}/ directory"));
                continue;
            }

            let present: BTreeSet<String> = fs::read_dir(&directory)
                .expect("command directory should be readable")
                .map(|entry| {
                    entry
                        .expect("directory entry should be readable")
                        .file_name()
                        .to_string_lossy()
                        .into_owned()
                })
                .collect();

            for required in REQUIRED {
                if !present.contains(*required) {
                    problems.push(format!("{stem}: missing {required}"));
                }
            }

            for name in &present {
                if REQUIRED.contains(&name.as_str()) {
                    continue;
                }
                // A `<file>/` of its own is a further split of one layer, which
                // is fine; `adapters/` re-nesting the same two layers is not.
                if name == "adapters" {
                    problems.push(format!("{stem}: adapters/ is not the layout"));
                    continue;
                }
                if directory.join(name).is_dir() {
                    continue;
                }
                if !EXTRA_FILES.contains(&(stem.as_str(), name.as_str())) {
                    problems.push(format!("{stem}: unexpected {name}"));
                }
            }
        }

        assert!(problems.is_empty(), "command layout drifted: {problems:#?}");
    }

    #[test]
    fn no_module_aliases_its_own_adapters() {
        let root = commands_dir();
        let mut offenders = Vec::new();

        for entry in fs::read_dir(&root).expect("commands directory should exist") {
            let path = entry.expect("directory entry should be readable").path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("rs") {
                continue;
            }
            // This file names the pattern it forbids.
            if path.file_stem().and_then(|stem| stem.to_str()) == Some("layout") {
                continue;
            }
            if fs::read_to_string(&path)
                .expect("module should be readable")
                .contains("mod adapters")
            {
                offenders.push(path.display().to_string());
            }
        }

        assert!(
            offenders.is_empty(),
            "an alias module makes two layouts look identical instead of being \
             identical: {offenders:#?}"
        );
    }
}
