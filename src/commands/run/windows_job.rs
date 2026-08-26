use std::{
    env,
    ffi::{OsStr, OsString, c_void},
    fs::File,
    io,
    mem::{self, size_of},
    os::windows::{
        ffi::{OsStrExt, OsStringExt},
        io::{FromRawHandle, RawHandle},
        process::ExitStatusExt,
    },
    path::Path,
    process::ExitStatus,
    ptr::{null, null_mut},
    thread,
    time::Duration,
};

use windows_sys::Win32::{
    Foundation::{
        CloseHandle, HANDLE, HANDLE_FLAG_INHERIT, SetHandleInformation, WAIT_FAILED, WAIT_OBJECT_0,
        WAIT_TIMEOUT,
    },
    Security::SECURITY_ATTRIBUTES,
    System::{
        JobObjects::{
            CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
            JOBOBJECT_BASIC_ACCOUNTING_INFORMATION, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
            JobObjectBasicAccountingInformation, JobObjectExtendedLimitInformation,
            QueryInformationJobObject, SetInformationJobObject, TerminateJobObject,
        },
        Pipes::CreatePipe,
        SystemInformation::GetSystemDirectoryW,
        Threading::{
            CREATE_NO_WINDOW, CREATE_UNICODE_ENVIRONMENT, CreateProcessW,
            DeleteProcThreadAttributeList, EXTENDED_STARTUPINFO_PRESENT, GetExitCodeProcess,
            INFINITE, InitializeProcThreadAttributeList, PROC_THREAD_ATTRIBUTE_HANDLE_LIST,
            PROC_THREAD_ATTRIBUTE_JOB_LIST, PROCESS_INFORMATION, STARTF_USESTDHANDLES,
            STARTUPINFOEXW, UpdateProcThreadAttribute, WaitForSingleObject,
        },
    },
};

use super::io::EnvironmentOverride;

pub(super) struct Child {
    process: OwnedHandle,
    job: OwnedHandle,
    stdout: Option<File>,
    stderr: Option<File>,
    root_status: Option<ExitStatus>,
}

impl Child {
    pub(super) fn take_stdout(&mut self) -> Option<File> {
        self.stdout.take()
    }

    pub(super) fn take_stderr(&mut self) -> Option<File> {
        self.stderr.take()
    }

    pub(super) fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        self.refresh_root_status()?;
        if self.root_status.is_some() && active_processes(self.job.raw())? == 0 {
            Ok(self.root_status)
        } else {
            Ok(None)
        }
    }

    pub(super) fn kill(&mut self) -> io::Result<()> {
        if active_processes(self.job.raw())? == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "process group has already exited",
            ));
        }
        win32_bool(unsafe { TerminateJobObject(self.job.raw(), 1) })
    }

    pub(super) fn wait(&mut self) -> io::Result<ExitStatus> {
        if self.root_status.is_none() {
            let result = unsafe { WaitForSingleObject(self.process.raw(), INFINITE) };
            if result == WAIT_FAILED {
                return Err(io::Error::last_os_error());
            }
            if result != WAIT_OBJECT_0 {
                return Err(io::Error::other(format!(
                    "unexpected process wait result {result}"
                )));
            }
            self.refresh_root_status()?;
        }
        while active_processes(self.job.raw())? != 0 {
            thread::sleep(Duration::from_millis(1));
        }
        self.root_status
            .ok_or_else(|| io::Error::other("process exited without an exit status"))
    }

    fn refresh_root_status(&mut self) -> io::Result<()> {
        if self.root_status.is_some() {
            return Ok(());
        }
        match unsafe { WaitForSingleObject(self.process.raw(), 0) } {
            WAIT_TIMEOUT => return Ok(()),
            WAIT_OBJECT_0 => {}
            WAIT_FAILED => return Err(io::Error::last_os_error()),
            result => {
                return Err(io::Error::other(format!(
                    "unexpected process wait result {result}"
                )));
            }
        }
        let mut code = 0;
        win32_bool(unsafe { GetExitCodeProcess(self.process.raw(), &mut code) })?;
        self.root_status = Some(ExitStatus::from_raw(code));
        Ok(())
    }
}

pub(super) fn spawn(
    program: &Path,
    args: &[String],
    cwd: Option<&Path>,
    environment: &[EnvironmentOverride<'_>],
) -> io::Result<Child> {
    let command_line = command_line(program.as_os_str(), args)?;
    spawn_prepared(program, command_line, cwd, environment)
}

pub(super) fn spawn_batch(
    command_prompt: &Path,
    script: &Path,
    args: &[String],
    cwd: Option<&Path>,
    environment: &[EnvironmentOverride<'_>],
) -> io::Result<Child> {
    let command_line = batch_command_line(script, args)?;
    spawn_prepared(command_prompt, command_line, cwd, environment)
}

pub(super) fn system_command_prompt() -> io::Result<std::path::PathBuf> {
    let mut buffer = vec![0u16; 260];
    loop {
        let length = unsafe {
            GetSystemDirectoryW(
                buffer.as_mut_ptr(),
                u32::try_from(buffer.len()).unwrap_or(u32::MAX),
            )
        };
        if length == 0 {
            return Err(io::Error::last_os_error());
        }
        let length = usize::try_from(length).expect("system directory length should fit usize");
        if length < buffer.len() {
            let directory = std::path::PathBuf::from(OsString::from_wide(&buffer[..length]));
            return Ok(directory.join("cmd.exe"));
        }
        buffer.resize(length + 1, 0);
    }
}

fn spawn_prepared(
    program: &Path,
    mut command_line: Vec<u16>,
    cwd: Option<&Path>,
    environment_overrides: &[EnvironmentOverride<'_>],
) -> io::Result<Child> {
    let job = create_job()?;
    let (stdin_read, stdin_write) = create_pipe()?;
    let (stdout_read, stdout_write) = create_pipe()?;
    let (stderr_read, stderr_write) = create_pipe()?;

    let inherited_handles = [stdin_read.raw(), stdout_write.raw(), stderr_write.raw()];
    let mut attributes = AttributeList::new(2)?;
    let job_handle = job.raw();
    attributes.set(
        PROC_THREAD_ATTRIBUTE_JOB_LIST as usize,
        (&job_handle as *const HANDLE).cast_mut().cast(),
        size_of::<HANDLE>(),
    )?;
    attributes.set(
        PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize,
        inherited_handles.as_ptr().cast_mut().cast(),
        mem::size_of_val(&inherited_handles),
    )?;

    let mut startup: STARTUPINFOEXW = unsafe { mem::zeroed() };
    startup.StartupInfo.cb =
        u32::try_from(size_of::<STARTUPINFOEXW>()).expect("STARTUPINFOEXW size should fit in u32");
    startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
    startup.StartupInfo.hStdInput = stdin_read.raw();
    startup.StartupInfo.hStdOutput = stdout_write.raw();
    startup.StartupInfo.hStdError = stderr_write.raw();
    startup.lpAttributeList = attributes.raw();

    let application = application_name(program)?;
    let current_directory = cwd.map(wide_null).transpose()?;
    let environment = environment_block(environment_overrides)?;
    let mut process_info: PROCESS_INFORMATION = unsafe { mem::zeroed() };
    // Windows requires HANDLE_LIST entries to be inheritable. Keep that global
    // state enabled only across CreateProcessW and serialize this backend's
    // spawns to avoid cross-request handle inheritance.
    let spawn_guard = ah_platform::exec::hold_create_process_lock();
    let inherit_guard = InheritGuard::new(&inherited_handles)?;
    let created = unsafe {
        CreateProcessW(
            application.as_ref().map_or(null(), |value| value.as_ptr()),
            command_line.as_mut_ptr(),
            null(),
            null(),
            1,
            EXTENDED_STARTUPINFO_PRESENT | CREATE_UNICODE_ENVIRONMENT | CREATE_NO_WINDOW,
            environment
                .as_ref()
                .map_or(null(), |value| value.as_ptr().cast()),
            current_directory
                .as_ref()
                .map_or(null(), |value| value.as_ptr()),
            &startup.StartupInfo,
            &mut process_info,
        )
    };
    drop(inherit_guard);
    drop(spawn_guard);
    win32_bool(created)?;

    let process = OwnedHandle::new(process_info.hProcess)?;
    let thread_handle = OwnedHandle::new(process_info.hThread)?;
    drop(thread_handle);
    drop(stdin_read);
    drop(stdin_write);
    drop(stdout_write);
    drop(stderr_write);

    Ok(Child {
        process,
        job,
        stdout: Some(owned_file(stdout_read)),
        stderr: Some(owned_file(stderr_read)),
        root_status: None,
    })
}

fn environment_block(overrides: &[EnvironmentOverride<'_>]) -> io::Result<Option<Vec<u16>>> {
    validate_environment_overrides(overrides)?;
    let mut entries = env::vars_os()
        .filter(|(name, _)| {
            !name
                .to_string_lossy()
                .eq_ignore_ascii_case(ah_plugin_api::AH_VAULT_MASTER_KEY_ENV)
                && !overrides.iter().any(|entry| {
                    name.to_string_lossy()
                        .eq_ignore_ascii_case(&entry.name.to_string_lossy())
                })
        })
        .collect::<Vec<_>>();
    entries.extend(
        overrides
            .iter()
            .filter(|entry| {
                !entry
                    .name
                    .to_string_lossy()
                    .eq_ignore_ascii_case(ah_plugin_api::AH_VAULT_MASTER_KEY_ENV)
            })
            .map(|entry| (entry.name.to_os_string(), entry.value.to_os_string())),
    );
    entries.sort_by(|left, right| {
        left.0
            .to_string_lossy()
            .to_ascii_uppercase()
            .cmp(&right.0.to_string_lossy().to_ascii_uppercase())
    });

    let mut block = Vec::new();
    for (name, value) in entries {
        let name = name.encode_wide().collect::<Vec<_>>();
        let value = value.encode_wide().collect::<Vec<_>>();
        if name.contains(&0) || value.contains(&0) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "process environment contains a NUL character",
            ));
        }
        block.extend(name);
        block.push(b'=' as u16);
        block.extend(value);
        block.push(0);
    }
    block.push(0);
    Ok(Some(block))
}

fn validate_environment_overrides(overrides: &[EnvironmentOverride<'_>]) -> io::Result<()> {
    for (index, entry) in overrides.iter().enumerate() {
        let name = entry.name.encode_wide().collect::<Vec<_>>();
        if name.is_empty() || name.contains(&0) || name.contains(&(b'=' as u16)) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid process environment variable name",
            ));
        }
        if overrides[..index].iter().any(|previous| {
            previous
                .name
                .to_string_lossy()
                .eq_ignore_ascii_case(&entry.name.to_string_lossy())
        }) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "duplicate process environment variable override",
            ));
        }
    }
    Ok(())
}

fn create_job() -> io::Result<OwnedHandle> {
    let job = OwnedHandle::new(unsafe { CreateJobObjectW(null(), null()) })?;
    let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { mem::zeroed() };
    limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
    win32_bool(unsafe {
        SetInformationJobObject(
            job.raw(),
            JobObjectExtendedLimitInformation,
            (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION)
                .cast_mut()
                .cast(),
            u32::try_from(size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>())
                .expect("job information size should fit in u32"),
        )
    })?;
    Ok(job)
}

fn active_processes(job: HANDLE) -> io::Result<u32> {
    let mut accounting: JOBOBJECT_BASIC_ACCOUNTING_INFORMATION = unsafe { mem::zeroed() };
    win32_bool(unsafe {
        QueryInformationJobObject(
            job,
            JobObjectBasicAccountingInformation,
            (&mut accounting as *mut JOBOBJECT_BASIC_ACCOUNTING_INFORMATION).cast(),
            u32::try_from(size_of::<JOBOBJECT_BASIC_ACCOUNTING_INFORMATION>())
                .expect("job accounting size should fit in u32"),
            null_mut(),
        )
    })?;
    Ok(accounting.ActiveProcesses)
}

fn create_pipe() -> io::Result<(OwnedHandle, OwnedHandle)> {
    let security = SECURITY_ATTRIBUTES {
        nLength: u32::try_from(size_of::<SECURITY_ATTRIBUTES>())
            .expect("security attributes size should fit in u32"),
        lpSecurityDescriptor: null_mut(),
        bInheritHandle: 0,
    };
    let mut read = null_mut();
    let mut write = null_mut();
    win32_bool(unsafe { CreatePipe(&mut read, &mut write, &security, 0) })?;
    Ok((OwnedHandle::new(read)?, OwnedHandle::new(write)?))
}

fn application_name(program: &Path) -> io::Result<Option<Vec<u16>>> {
    if program.is_absolute()
        || program
            .as_os_str()
            .to_string_lossy()
            .chars()
            .any(|character| matches!(character, '/' | '\\'))
    {
        wide_null(program.as_os_str()).map(Some)
    } else {
        Ok(None)
    }
}

fn command_line(program: &OsStr, args: &[String]) -> io::Result<Vec<u16>> {
    let mut result = Vec::new();
    append_quoted(&mut result, program)?;
    for argument in args {
        result.push(b' ' as u16);
        append_quoted(&mut result, OsStr::new(argument))?;
    }
    result.push(0);
    Ok(result)
}

fn batch_command_line(script: &Path, args: &[String]) -> io::Result<Vec<u16>> {
    let script = batch_script_user_path(script);
    if script.contains(&0)
        || script.contains(&(b'"' as u16))
        || script.last() == Some(&(b'\\' as u16))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid Windows batch script path",
        ));
    }

    let mut result = "cmd.exe /e:ON /v:OFF /d /c \"\""
        .encode_utf16()
        .collect::<Vec<_>>();
    result.extend(script);
    result.push(b'"' as u16);
    for argument in args {
        result.push(b' ' as u16);
        append_batch_arg(&mut result, argument)?;
    }
    result.push(b'"' as u16);
    result.push(0);
    Ok(result)
}

fn batch_script_user_path(script: &Path) -> Vec<u16> {
    let script = script.as_os_str().encode_wide().collect::<Vec<_>>();
    let verbatim = [b'\\' as u16, b'\\' as u16, b'?' as u16, b'\\' as u16];
    let unc = [b'U' as u16, b'N' as u16, b'C' as u16, b'\\' as u16];
    if script.starts_with(&verbatim)
        && script
            .get(verbatim.len()..verbatim.len() + unc.len())
            .is_some_and(|prefix| {
                prefix.iter().zip(unc).all(|(actual, expected)| {
                    *actual == expected
                        || ((b'a' as u16..=b'z' as u16).contains(actual)
                            && *actual - 32 == expected)
                })
            })
    {
        let mut user_path = vec![b'\\' as u16, b'\\' as u16];
        user_path.extend_from_slice(&script[verbatim.len() + unc.len()..]);
        user_path
    } else if let Some(user_path) = script.strip_prefix(&verbatim) {
        user_path.to_vec()
    } else {
        script
    }
}

fn append_batch_arg(output: &mut Vec<u16>, value: &str) -> io::Result<()> {
    if value.contains(['\0', '\r', '\n']) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "batch file arguments contain an invalid character",
        ));
    }
    let unquoted = r"#$*+-./:?@\_";
    let mut quote = value.is_empty()
        || value.ends_with('\\')
        || value.chars().any(|character| {
            (character.is_ascii()
                && !(character.is_ascii_alphanumeric() || unquoted.contains(character)))
                || character.is_control()
        });
    if value.contains('"') {
        quote = true;
    }
    if quote {
        output.push(b'"' as u16);
    }

    let mut backslashes = 0usize;
    for character in value.encode_utf16() {
        if character == b'\\' as u16 {
            backslashes += 1;
        } else {
            if character == b'"' as u16 {
                output.extend(std::iter::repeat_n(b'\\' as u16, backslashes));
                output.push(b'"' as u16);
            } else if character == b'%' as u16 {
                output.extend("%%cd:~,".encode_utf16());
            }
            backslashes = 0;
        }
        output.push(character);
    }
    if quote {
        output.extend(std::iter::repeat_n(b'\\' as u16, backslashes));
        output.push(b'"' as u16);
    }
    Ok(())
}

fn append_quoted(output: &mut Vec<u16>, value: &OsStr) -> io::Result<()> {
    let value = value.encode_wide().collect::<Vec<_>>();
    if value.contains(&0) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "process argument contains a NUL character",
        ));
    }
    let needs_quotes = value.is_empty()
        || value.iter().any(|character| {
            *character == b' ' as u16 || *character == b'\t' as u16 || *character == b'"' as u16
        });
    if !needs_quotes {
        output.extend(value);
        return Ok(());
    }

    output.push(b'"' as u16);
    let mut backslashes = 0usize;
    for character in value {
        if character == b'\\' as u16 {
            backslashes += 1;
            continue;
        }
        if character == b'"' as u16 {
            output.extend(std::iter::repeat_n(b'\\' as u16, backslashes * 2 + 1));
        } else {
            output.extend(std::iter::repeat_n(b'\\' as u16, backslashes));
        }
        backslashes = 0;
        output.push(character);
    }
    output.extend(std::iter::repeat_n(b'\\' as u16, backslashes * 2));
    output.push(b'"' as u16);
    Ok(())
}

fn wide_null(value: impl AsRef<OsStr>) -> io::Result<Vec<u16>> {
    let mut wide = value.as_ref().encode_wide().collect::<Vec<_>>();
    if wide.contains(&0) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "process path contains a NUL character",
        ));
    }
    wide.push(0);
    Ok(wide)
}

fn owned_file(handle: OwnedHandle) -> File {
    let raw = handle.into_raw();
    unsafe { File::from_raw_handle(raw as RawHandle) }
}

fn win32_bool(result: i32) -> io::Result<()> {
    if result == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

struct AttributeList {
    _storage: Vec<usize>,
    raw: *mut c_void,
}

impl AttributeList {
    fn new(attribute_count: u32) -> io::Result<Self> {
        let mut bytes = 0usize;
        unsafe {
            InitializeProcThreadAttributeList(null_mut(), attribute_count, 0, &mut bytes);
        }
        if bytes == 0 {
            return Err(io::Error::last_os_error());
        }
        let words = bytes.div_ceil(size_of::<usize>());
        let mut storage = vec![0usize; words];
        let raw = storage.as_mut_ptr().cast();
        win32_bool(unsafe {
            InitializeProcThreadAttributeList(raw, attribute_count, 0, &mut bytes)
        })?;
        Ok(Self {
            _storage: storage,
            raw,
        })
    }

    fn set(&mut self, attribute: usize, value: *mut c_void, bytes: usize) -> io::Result<()> {
        win32_bool(unsafe {
            UpdateProcThreadAttribute(self.raw, 0, attribute, value, bytes, null_mut(), null_mut())
        })
    }

    fn raw(&self) -> *mut c_void {
        self.raw
    }
}

impl Drop for AttributeList {
    fn drop(&mut self) {
        unsafe { DeleteProcThreadAttributeList(self.raw) };
    }
}

struct OwnedHandle(HANDLE);

impl OwnedHandle {
    fn new(raw: HANDLE) -> io::Result<Self> {
        if raw.is_null() {
            Err(io::Error::last_os_error())
        } else {
            Ok(Self(raw))
        }
    }

    fn raw(&self) -> HANDLE {
        self.0
    }

    fn into_raw(mut self) -> HANDLE {
        let raw = self.0;
        self.0 = null_mut();
        raw
    }
}

struct InheritGuard<'a> {
    handles: &'a [HANDLE],
}

impl<'a> InheritGuard<'a> {
    fn new(handles: &'a [HANDLE]) -> io::Result<Self> {
        for (index, handle) in handles.iter().copied().enumerate() {
            if let Err(error) = win32_bool(unsafe {
                SetHandleInformation(handle, HANDLE_FLAG_INHERIT, HANDLE_FLAG_INHERIT)
            }) {
                for inherited in &handles[..index] {
                    unsafe { SetHandleInformation(*inherited, HANDLE_FLAG_INHERIT, 0) };
                }
                return Err(error);
            }
        }
        Ok(Self { handles })
    }
}

impl Drop for InheritGuard<'_> {
    fn drop(&mut self) {
        for handle in self.handles {
            unsafe { SetHandleInformation(*handle, HANDLE_FLAG_INHERIT, 0) };
        }
    }
}

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { CloseHandle(self.0) };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{command_line, environment_block};
    use crate::commands::run::io::EnvironmentOverride;
    use std::ffi::OsStr;

    #[test]
    fn command_line_quotes_empty_spaces_quotes_and_trailing_backslashes() {
        let args = vec![
            String::new(),
            "two words".to_owned(),
            "quote\"inside".to_owned(),
            "ends with slash\\".to_owned(),
        ];
        let encoded = command_line(OsStr::new("tool.exe"), &args).unwrap();
        let decoded = String::from_utf16(&encoded[..encoded.len() - 1]).unwrap();
        assert_eq!(
            decoded,
            "tool.exe \"\" \"two words\" \"quote\\\"inside\" \"ends with slash\\\\\""
        );
    }

    #[test]
    fn environment_block_applies_case_insensitive_override() {
        let value = OsStr::new("isolated");
        let block = environment_block(&[EnvironmentOverride {
            name: OsStr::new("AH_CONFIG_DIR"),
            value,
        }])
        .unwrap()
        .unwrap();
        let decoded = String::from_utf16_lossy(&block);
        let matches = decoded
            .split('\0')
            .filter(|entry| {
                entry
                    .split_once('=')
                    .is_some_and(|(name, _)| name.eq_ignore_ascii_case("AH_CONFIG_DIR"))
            })
            .collect::<Vec<_>>();
        assert_eq!(matches, vec!["AH_CONFIG_DIR=isolated"]);
        assert!(block.ends_with(&[0, 0]));
    }

    #[test]
    fn environment_block_drops_vault_master_key_override() {
        let block = environment_block(&[EnvironmentOverride {
            name: OsStr::new("ah_vault_master_key"),
            value: OsStr::new("sentinel"),
        }])
        .unwrap()
        .unwrap();
        let decoded = String::from_utf16_lossy(&block);
        assert!(
            !decoded
                .to_ascii_uppercase()
                .contains("AH_VAULT_MASTER_KEY=")
        );
    }
}
