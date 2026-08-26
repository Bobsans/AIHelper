//! Finding an executable, which Windows does differently from everywhere else.

use std::path::{Path, PathBuf};

/// The extensions an executable may carry, in the order the platform tries
/// them.
///
/// Windows reads `PATHEXT`; a program named on the command line without one is
/// only found by appending these. Everywhere else there is exactly one
/// candidate, the name as written, which is why the list holds one empty
/// string rather than being empty.
///
/// Order matters and is `PATHEXT`'s own: a `.CMD` shim beside a `.EXE` must not
/// win merely because `c` sorts before `e`.
#[must_use]
pub fn executable_extensions() -> Vec<String> {
    #[cfg(windows)]
    {
        std::env::var_os("PATHEXT")
            .map(|raw| {
                raw.to_string_lossy()
                    .split(';')
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(|value| {
                        if value.starts_with('.') {
                            value.to_owned()
                        } else {
                            format!(".{value}")
                        }
                    })
                    .collect::<Vec<_>>()
            })
            .filter(|values| !values.is_empty())
            .unwrap_or_else(|| {
                [".COM", ".EXE", ".BAT", ".CMD"]
                    .into_iter()
                    .map(str::to_owned)
                    .collect()
            })
    }
    #[cfg(not(windows))]
    {
        vec![String::new()]
    }
}

/// The first file that exists at `candidate` once an executable extension is
/// appended.
///
/// `candidate` is a path without an extension: `C:\tools\psql`, not
/// `C:\tools\psql.exe`.
#[must_use]
pub fn with_executable_extension(candidate: &Path, extensions: &[String]) -> Option<PathBuf> {
    for extension in extensions {
        let extended = if extension.is_empty() {
            candidate.to_path_buf()
        } else {
            let mut extended = candidate.as_os_str().to_owned();
            extended.push(extension);
            PathBuf::from(extended)
        };
        if extended.is_file() {
            return Some(extended);
        }
    }
    None
}

/// Search `directories`, then `PATH`, for an executable named `program`.
///
/// `program` is a bare name; a path is the caller's to resolve, because what a
/// relative path means depends on what the caller is doing.
#[must_use]
pub fn find_executable(program: &str, directories: &[PathBuf]) -> Option<PathBuf> {
    let extensions = executable_extensions();
    for directory in directories {
        if let Some(found) = with_executable_extension(&directory.join(program), &extensions) {
            return Some(found);
        }
    }
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .find_map(|directory| with_executable_extension(&directory.join(program), &extensions))
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    /// Every platform offers at least one candidate, so a caller never has to
    /// special-case an empty list.
    #[test]
    fn there_is_always_at_least_one_candidate_extension() {
        assert!(!executable_extensions().is_empty());
    }

    #[test]
    fn a_file_is_found_by_appending_a_candidate_extension() {
        let temp = tempfile::TempDir::new().expect("temporary dir should be created");
        let extensions = executable_extensions();
        let extension = extensions.first().expect("at least one candidate");
        let path = temp.path().join(format!("tool{extension}"));
        fs::write(&path, "#!/bin/sh\n").expect("file should be written");

        assert_eq!(
            with_executable_extension(&temp.path().join("tool"), &extensions),
            Some(path)
        );
    }

    /// The list is searched in order, not sorted: a shim must not overtake a
    /// real executable because its extension happens to sort earlier.
    #[test]
    fn the_first_listed_extension_wins() {
        let temp = tempfile::TempDir::new().expect("temporary dir should be created");
        let extensions = vec![".zzz".to_owned(), ".aaa".to_owned()];
        for extension in &extensions {
            fs::write(temp.path().join(format!("tool{extension}")), "body")
                .expect("file should be written");
        }

        assert_eq!(
            with_executable_extension(&temp.path().join("tool"), &extensions),
            Some(temp.path().join("tool.zzz"))
        );
    }

    #[test]
    fn a_supplied_directory_is_searched_before_path() {
        let temp = tempfile::TempDir::new().expect("temporary dir should be created");
        let extensions = executable_extensions();
        let extension = extensions.first().expect("at least one candidate");
        let path = temp.path().join(format!("only-here{extension}"));
        fs::write(&path, "body").expect("file should be written");

        assert_eq!(
            find_executable("only-here", &[temp.path().to_path_buf()]),
            Some(path)
        );
    }

    #[test]
    fn a_program_that_is_nowhere_is_not_found() {
        assert_eq!(
            find_executable("ah-no-such-program-anywhere", &[]),
            None,
            "an absent program resolves to nothing rather than to its own name"
        );
    }
}
