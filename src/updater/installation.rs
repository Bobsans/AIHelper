use std::{
    fs,
    path::{Path, PathBuf},
};

use ah_updater_core::{UpdaterError, UpdaterErrorCode};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PortableInstallation {
    executable: PathBuf,
    root: PathBuf,
}

impl PortableInstallation {
    pub(crate) fn executable(&self) -> &Path {
        &self.executable
    }

    pub(crate) fn root(&self) -> &Path {
        &self.root
    }
}

pub(crate) fn inspect_current_portable_installation() -> Result<PortableInstallation, UpdaterError>
{
    let executable = std::env::current_exe()
        .map_err(|_| installation("failed to resolve the running AIHelper executable"))?;
    inspect_portable_installation(&executable)
}

fn inspect_portable_installation(executable: &Path) -> Result<PortableInstallation, UpdaterError> {
    ensure_direct_executable(executable)?;
    let executable = fs::canonicalize(executable)
        .map_err(|_| installation("failed to canonicalize the running AIHelper executable"))?;
    ensure_direct_executable(&executable)?;
    if executable.to_str().is_none() {
        return Err(installation(
            "the running AIHelper executable path is not valid Unicode",
        ));
    }
    if !is_expected_executable_name(&executable) {
        return Err(installation(
            "the running executable does not use the supported AIHelper filename",
        ));
    }
    let root = executable
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .ok_or_else(|| installation("failed to derive the AIHelper installation root"))?
        .to_path_buf();

    if cargo_install_root(&root).is_some() {
        return Err(installation(
            "cargo-managed AIHelper cannot self-update; run `cargo install aihelper --locked --force`",
        ));
    }

    Ok(PortableInstallation { executable, root })
}

fn ensure_direct_executable(path: &Path) -> Result<(), UpdaterError> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|_| installation("failed to inspect the running AIHelper executable"))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(installation(
            "the running AIHelper executable is not a direct regular file",
        ));
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;

        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
        if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(installation(
                "the running AIHelper executable is a reparse point",
            ));
        }
    }
    Ok(())
}

fn is_expected_executable_name(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    #[cfg(windows)]
    {
        name.eq_ignore_ascii_case("ah.exe")
    }
    #[cfg(not(windows))]
    {
        name == "ah"
    }
}

fn cargo_install_root(installation_root: &Path) -> Option<PathBuf> {
    if !installation_root
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.eq_ignore_ascii_case("bin"))
    {
        return None;
    }
    let cargo_root = installation_root.parent()?;
    [".crates2.json", ".crates.toml"]
        .into_iter()
        .any(|marker| is_direct_regular_file(&cargo_root.join(marker)))
        .then(|| cargo_root.to_path_buf())
}

fn is_direct_regular_file(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|metadata| {
        !metadata.file_type().is_symlink() && metadata.is_file() && {
            #[cfg(windows)]
            {
                use std::os::windows::fs::MetadataExt;

                const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
                metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT == 0
            }
            #[cfg(not(windows))]
            {
                true
            }
        }
    })
}

fn installation(detail: &'static str) -> UpdaterError {
    UpdaterError::new(UpdaterErrorCode::Installation, detail)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::TempDir;

    use super::*;

    #[test]
    fn derives_canonical_root_without_mutating_portable_installation() {
        let temp = TempDir::new().unwrap();
        let root = temp.path().join("Portable AIHelper Юникод");
        fs::create_dir(&root).unwrap();
        let executable = root.join(executable_name());
        fs::write(&executable, b"binary").unwrap();
        let sentinel = root.join("user-owned.txt");
        fs::write(&sentinel, b"unchanged").unwrap();

        let installation = inspect_portable_installation(&executable).unwrap();

        assert_eq!(
            installation.executable(),
            fs::canonicalize(&executable).unwrap()
        );
        assert_eq!(installation.root(), fs::canonicalize(&root).unwrap());
        assert_eq!(fs::read(sentinel).unwrap(), b"unchanged");
        assert_eq!(fs::read_dir(root).unwrap().count(), 2);
    }

    #[test]
    fn refuses_cargo_managed_layout_without_mutation() {
        for marker in [".crates2.json", ".crates.toml"] {
            let temp = TempDir::new().unwrap();
            let bin = temp.path().join("bin");
            fs::create_dir(&bin).unwrap();
            let executable = bin.join(executable_name());
            fs::write(&executable, b"binary").unwrap();
            fs::write(temp.path().join(marker), b"cargo metadata").unwrap();
            let sentinel = temp.path().join("sentinel.txt");
            fs::write(&sentinel, b"unchanged").unwrap();

            let error = inspect_portable_installation(&executable).unwrap_err();

            assert_eq!(error.code(), UpdaterErrorCode::Installation);
            assert!(error.detail().contains("cargo install"));
            assert_eq!(fs::read(sentinel).unwrap(), b"unchanged");
            assert_eq!(fs::read(&executable).unwrap(), b"binary");
        }
    }

    #[test]
    fn accepts_portable_bin_directory_without_cargo_metadata() {
        let temp = TempDir::new().unwrap();
        let bin = temp.path().join("bin");
        fs::create_dir(&bin).unwrap();
        let executable = bin.join(executable_name());
        fs::write(&executable, b"binary").unwrap();

        let installation = inspect_portable_installation(&executable).unwrap();

        assert_eq!(installation.root(), fs::canonicalize(bin).unwrap());
    }

    #[test]
    fn rejects_directory_and_unexpected_executable_name() {
        let temp = TempDir::new().unwrap();
        assert_eq!(
            inspect_portable_installation(temp.path())
                .unwrap_err()
                .code(),
            UpdaterErrorCode::Installation
        );

        let unexpected = temp.path().join(if cfg!(windows) {
            "renamed.exe"
        } else {
            "renamed"
        });
        fs::write(&unexpected, b"binary").unwrap();
        assert_eq!(
            inspect_portable_installation(&unexpected)
                .unwrap_err()
                .code(),
            UpdaterErrorCode::Installation
        );
    }

    fn executable_name() -> &'static str {
        if cfg!(windows) { "ah.exe" } else { "ah" }
    }
}
