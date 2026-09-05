use std::path::{Component, Path, PathBuf};

use uuid::Uuid;

use ah_error::AppError;

pub const MANAGED_SERVICE_EXECUTABLE: &str = "ah-mcp-service.exe";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServicePaths {
    pub base_dir: PathBuf,
    pub definitions_dir: PathBuf,
    pub current: PathBuf,
    pub runtime: PathBuf,
    pub lifecycle: PathBuf,
    pub lifecycle_lock: PathBuf,
    pub instance_lock: PathBuf,
}

impl ServicePaths {
    pub fn from_base(base_dir: PathBuf) -> Result<Self, AppError> {
        let base_dir = normalize_absolute_path(&base_dir, None)?;
        Ok(Self {
            definitions_dir: base_dir.join("definitions"),
            current: base_dir.join("current.json"),
            runtime: base_dir.join("runtime.json"),
            lifecycle: base_dir.join("lifecycle.json"),
            lifecycle_lock: base_dir.join("lifecycle.lock"),
            instance_lock: base_dir.join("instance.lock"),
            base_dir,
        })
    }

    /// Where this machine keeps the managed service's state.
    ///
    /// Neither platform reads the environment for it, and that is the point: a
    /// service must not take its state directory from a variable a caller can
    /// set, or `ah mcp service status` could be pointed at someone else's
    /// answer. Windows asks for the known folder, Unix asks the password
    /// database for the account's home rather than trusting `$HOME`.
    ///
    /// This is why group 06 excludes the service from `ah-paths`, which
    /// deliberately does resolve environment overrides.
    pub fn discover() -> Result<Self, AppError> {
        #[cfg(windows)]
        {
            let local_app_data = windows_local_app_data()?;
            Self::from_base(local_app_data.join("AIHelper").join("managed-mcp"))
        }

        #[cfg(unix)]
        {
            Self::from_base(
                account_home()?
                    .join(".local")
                    .join("state")
                    .join("aihelper")
                    .join("managed-mcp"),
            )
        }

        #[cfg(not(any(windows, unix)))]
        {
            Err(crate::scheduler::unsupported_platform())
        }
    }

    pub fn definition(&self, configuration_id: Uuid) -> PathBuf {
        self.definitions_dir
            .join(format!("{configuration_id}.json"))
    }
}

pub fn current_executable_path() -> Result<PathBuf, AppError> {
    normalize_absolute_path(
        &std::env::current_exe().map_err(|error| {
            AppError::external(
                "MCP_SERVICE_PATH_INVALID",
                format!("failed to resolve current executable: {error}"),
            )
        })?,
        None,
    )
}

pub fn managed_service_executable_path(executable: &Path) -> PathBuf {
    if cfg!(windows) {
        executable.with_file_name(MANAGED_SERVICE_EXECUTABLE)
    } else {
        executable.to_owned()
    }
}

pub fn require_managed_service_executable(executable: &Path) -> Result<(), AppError> {
    let worker = managed_service_executable_path(executable);
    if !worker.is_file() {
        return Err(AppError::external(
            "MCP_SERVICE_WORKER_MISSING",
            format!("managed MCP worker is missing: '{}'", worker.display()),
        ));
    }
    Ok(())
}

pub fn normalize_absolute_path(path: &Path, cwd: Option<&Path>) -> Result<PathBuf, AppError> {
    if path.to_str().is_none() {
        return Err(path_invalid("path is not valid Unicode"));
    }
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        let base = cwd.ok_or_else(|| path_invalid("relative path has no working directory"))?;
        base.join(path)
    };
    if !absolute.is_absolute() {
        return Err(path_invalid(format!(
            "path '{}' is not absolute",
            absolute.display()
        )));
    }
    let existing = absolute
        .ancestors()
        .find(|path| path.exists())
        .ok_or_else(|| path_invalid("path has no accessible existing ancestor"))?;
    let mut normalized = std::fs::canonicalize(existing)
        .map_err(|error| path_invalid(format!("failed to canonicalize path: {error}")))?;
    normalized.push(
        absolute
            .strip_prefix(existing)
            .map_err(|_| path_invalid("failed to resolve path from its existing ancestor"))?,
    );
    let normalized = normalize_components(&normalized)?;
    Ok(strip_verbatim_prefix(normalized))
}

pub fn paths_equal(left: &Path, right: &Path) -> bool {
    let (Some(left), Some(right)) = (identity_string(left), identity_string(right)) else {
        return false;
    };

    #[cfg(windows)]
    {
        use windows::Win32::Globalization::{CSTR_EQUAL, CompareStringOrdinal};

        let left = left.encode_utf16().collect::<Vec<_>>();
        let right = right.encode_utf16().collect::<Vec<_>>();
        // SAFETY: both slices contain initialized UTF-16 code units and remain
        // alive for the duration of the call.
        unsafe { CompareStringOrdinal(&left, &right, true) == CSTR_EQUAL }
    }

    #[cfg(not(windows))]
    {
        left == right
    }
}

/// The account the managed service belongs to, spelled the way this platform
/// spells accounts: a security identifier on Windows, a numeric user id on
/// Unix.
///
/// The lifecycle does not call this - it asks its scheduler adapter for an
/// identity, which is what lets it run against a fake on any platform. The
/// callers left are the adapters themselves and [`crate::runner`], the managed
/// worker process.
pub fn current_account() -> Result<String, AppError> {
    #[cfg(windows)]
    {
        windows_current_user_sid()
    }

    #[cfg(unix)]
    {
        // SAFETY: `getuid` reads the calling process's real user id and cannot
        // fail.
        Ok(unsafe { libc::getuid() }.to_string())
    }

    #[cfg(not(any(windows, unix)))]
    {
        Err(crate::scheduler::unsupported_platform())
    }
}

/// The account's home directory according to the password database.
///
/// `$HOME` is not consulted, for the same reason the Windows side asks for the
/// known folder rather than reading `%LOCALAPPDATA%`: a caller must not be able
/// to move the service's state directory by setting a variable.
#[cfg(unix)]
fn account_home() -> Result<PathBuf, AppError> {
    use std::{
        ffi::{CStr, OsStr},
        os::unix::ffi::OsStrExt,
    };

    const MAXIMUM_BUFFER: usize = 64 * 1024;

    // SAFETY: `getuid` reads the calling process's real user id and cannot
    // fail.
    let uid = unsafe { libc::getuid() };
    let mut buffer = vec![0u8; 1024];
    loop {
        // SAFETY: `libc::passwd` is a plain C struct whose all-zero value is
        // valid; `getpwuid_r` fills it and writes the strings it points at into
        // `buffer`, whose real length is the one passed. Both outlive the call.
        let mut entry: libc::passwd = unsafe { std::mem::zeroed() };
        let mut found: *mut libc::passwd = std::ptr::null_mut();
        let code = unsafe {
            libc::getpwuid_r(
                uid,
                &mut entry,
                buffer.as_mut_ptr().cast(),
                buffer.len(),
                &mut found,
            )
        };
        if code == libc::ERANGE && buffer.len() < MAXIMUM_BUFFER {
            buffer.resize(buffer.len() * 2, 0);
            continue;
        }
        if code != 0 {
            return Err(path_invalid(format!(
                "failed to read the password entry for uid {uid}: {}",
                std::io::Error::from_raw_os_error(code)
            )));
        }
        if found.is_null() {
            return Err(path_invalid(format!("uid {uid} has no password entry")));
        }
        // SAFETY: a successful call with a non-null result leaves `entry`
        // initialized, and `pw_dir` NUL-terminated inside `buffer`.
        let home = unsafe { CStr::from_ptr(entry.pw_dir) };
        let home = PathBuf::from(OsStr::from_bytes(home.to_bytes()));
        if !home.is_absolute() {
            return Err(path_invalid(format!(
                "the home directory of uid {uid} is '{}', which is not absolute",
                home.display()
            )));
        }
        return Ok(home);
    }
}

#[cfg(windows)]
pub fn account_sid(account: &str) -> Result<String, AppError> {
    use windows::{
        Win32::Security::{LookupAccountNameW, PSID, SID_NAME_USE},
        core::{PCWSTR, PWSTR},
    };

    if account.starts_with("S-1-") {
        return Ok(account.to_owned());
    }

    let account = account.encode_utf16().chain([0]).collect::<Vec<_>>();
    // SAFETY: both calls use initialized, correctly sized buffers. The first
    // call only determines their required sizes.
    unsafe {
        let mut sid_size = 0;
        let mut domain_size = 0;
        let mut sid_kind = SID_NAME_USE::default();
        let _ = LookupAccountNameW(
            PCWSTR::null(),
            PCWSTR(account.as_ptr()),
            None,
            &mut sid_size,
            None,
            &mut domain_size,
            &mut sid_kind,
        );
        if sid_size == 0 {
            return Err(AppError::external(
                "MCP_SERVICE_STATE_INVALID",
                format!(
                    "failed to resolve task account SID: {}",
                    windows::core::Error::from_thread()
                ),
            ));
        }

        let mut sid = vec![0u8; sid_size as usize];
        let mut domain = vec![0u16; domain_size as usize];
        LookupAccountNameW(
            PCWSTR::null(),
            PCWSTR(account.as_ptr()),
            Some(PSID(sid.as_mut_ptr().cast())),
            &mut sid_size,
            (!domain.is_empty()).then_some(PWSTR(domain.as_mut_ptr())),
            &mut domain_size,
            &mut sid_kind,
        )
        .map_err(|error| {
            AppError::external(
                "MCP_SERVICE_STATE_INVALID",
                format!("failed to resolve task account SID: {error}"),
            )
        })?;
        sid_string(PSID(sid.as_mut_ptr().cast()), "task account")
    }
}

fn normalize_components(path: &Path) -> Result<PathBuf, AppError> {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !normalized.pop() {
                    return Err(path_invalid(format!(
                        "path '{}' escapes its root",
                        path.display()
                    )));
                }
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    Ok(normalized)
}

fn identity_string(path: &Path) -> Option<String> {
    let normalized = strip_verbatim_prefix(path.to_path_buf());
    let mut value = normalized.to_str()?.replace('/', "\\");
    while value.ends_with('\\') && !is_windows_root(&value) {
        value.pop();
    }
    Some(value)
}

fn is_windows_root(value: &str) -> bool {
    (value.len() == 3 && value.as_bytes()[1] == b':' && value.ends_with('\\')) || value == "\\"
}

fn strip_verbatim_prefix(path: PathBuf) -> PathBuf {
    let Some(value) = path.to_str() else {
        return path;
    };
    if let Some(stripped) = value.strip_prefix(r"\\?\UNC\") {
        PathBuf::from(format!(r"\\{stripped}"))
    } else if let Some(stripped) = value.strip_prefix(r"\\?\") {
        PathBuf::from(stripped)
    } else {
        path
    }
}

fn path_invalid(message: impl Into<String>) -> AppError {
    AppError::external("MCP_SERVICE_PATH_INVALID", message)
}

#[cfg(windows)]
fn windows_local_app_data() -> Result<PathBuf, AppError> {
    use windows::Win32::{
        System::Com::CoTaskMemFree,
        UI::Shell::{FOLDERID_LocalAppData, KF_FLAG_DEFAULT, SHGetKnownFolderPath},
    };

    // SAFETY: the known-folder identifier is valid and the returned allocation
    // is released with CoTaskMemFree as required by SHGetKnownFolderPath.
    unsafe {
        let value = SHGetKnownFolderPath(&FOLDERID_LocalAppData, KF_FLAG_DEFAULT, None)
            .map_err(|error| path_invalid(format!("failed to resolve LocalAppData: {error}")))?;
        let result = value
            .to_string()
            .map(PathBuf::from)
            .map_err(|error| path_invalid(format!("LocalAppData is not valid Unicode: {error}")));
        CoTaskMemFree(Some(value.0.cast()));
        result
    }
}

#[cfg(windows)]
fn windows_current_user_sid() -> Result<String, AppError> {
    use windows::Win32::{
        Foundation::{CloseHandle, HANDLE},
        Security::{GetTokenInformation, TOKEN_QUERY, TOKEN_USER, TokenUser},
        System::Threading::{GetCurrentProcess, OpenProcessToken},
    };

    // SAFETY: token and SID buffers are sized through the documented probe
    // calls. Every allocated handle and SID string is released on all paths.
    unsafe {
        let mut token = HANDLE::default();
        OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).map_err(|error| {
            AppError::external(
                "MCP_SERVICE_STATE_INVALID",
                format!("failed to open current process token: {error}"),
            )
        })?;
        let result = (|| {
            let mut required = 0;
            let _ = GetTokenInformation(token, TokenUser, None, 0, &mut required);
            if required == 0 {
                return Err(AppError::external(
                    "MCP_SERVICE_STATE_INVALID",
                    "failed to size current user token information",
                ));
            }
            let mut buffer = vec![0u8; required as usize];
            GetTokenInformation(
                token,
                TokenUser,
                Some(buffer.as_mut_ptr().cast()),
                required,
                &mut required,
            )
            .map_err(|error| {
                AppError::external(
                    "MCP_SERVICE_STATE_INVALID",
                    format!("failed to read current user token: {error}"),
                )
            })?;
            let token_user = &*(buffer.as_ptr().cast::<TOKEN_USER>());
            sid_string(token_user.User.Sid, "current user")
        })();
        let _ = CloseHandle(token);
        result
    }
}

#[cfg(windows)]
fn sid_string(sid: windows::Win32::Security::PSID, description: &str) -> Result<String, AppError> {
    use windows::{
        Win32::{
            Foundation::{HLOCAL, LocalFree},
            Security::Authorization::ConvertSidToStringSidW,
        },
        core::PWSTR,
    };

    // SAFETY: ConvertSidToStringSidW allocates the returned string with
    // LocalAlloc; it is converted before being released with LocalFree.
    unsafe {
        let mut value = PWSTR::null();
        ConvertSidToStringSidW(sid, &mut value).map_err(|error| {
            AppError::external(
                "MCP_SERVICE_STATE_INVALID",
                format!("failed to convert {description} SID: {error}"),
            )
        })?;
        let result = value.to_string().map_err(|error| {
            AppError::external(
                "MCP_SERVICE_STATE_INVALID",
                format!("{description} SID is not valid Unicode: {error}"),
            )
        });
        LocalFree(Some(HLOCAL(value.0.cast())));
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn managed_service_worker_is_a_sibling_on_windows() {
        let executable = if cfg!(windows) {
            PathBuf::from(r"C:\AIHelper\ah.exe")
        } else {
            PathBuf::from("/opt/aihelper/ah")
        };
        let expected = if cfg!(windows) {
            PathBuf::from(r"C:\AIHelper\ah-mcp-service.exe")
        } else {
            executable.clone()
        };
        assert_eq!(managed_service_executable_path(&executable), expected);
    }

    #[cfg(windows)]
    #[test]
    fn managed_service_worker_must_exist() {
        let temp = tempfile::tempdir().unwrap();
        let executable = temp.path().join("ah.exe");
        let error = require_managed_service_executable(&executable).unwrap_err();
        assert_eq!(error.code(), "MCP_SERVICE_WORKER_MISSING");

        std::fs::write(managed_service_executable_path(&executable), b"worker").unwrap();
        require_managed_service_executable(&executable).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn account_names_normalize_to_sids() {
        let account = std::env::var("USERNAME").unwrap();
        let sid = account_sid(&account).unwrap();
        assert!(sid.starts_with("S-1-"));
        assert_eq!(account_sid(&sid).unwrap(), sid);
    }

    #[test]
    fn paths_derive_the_complete_store_layout() {
        let temp = tempfile::tempdir().unwrap();
        let base = normalize_absolute_path(temp.path(), None)
            .unwrap()
            .join("managed-mcp");
        let paths = ServicePaths::from_base(base.clone()).unwrap();
        assert_eq!(paths.current, base.join("current.json"));
        assert_eq!(paths.runtime, base.join("runtime.json"));
        assert_eq!(paths.lifecycle_lock, base.join("lifecycle.lock"));
        assert!(
            paths
                .definition(Uuid::nil())
                .ends_with("00000000-0000-0000-0000-000000000000.json")
        );
    }

    #[cfg(windows)]
    #[test]
    fn path_identity_is_separator_and_case_insensitive() {
        assert!(paths_equal(
            Path::new(r"C:\AIHelper\AH.EXE"),
            Path::new("c:/aihelper/ah.exe/")
        ));
    }
}
