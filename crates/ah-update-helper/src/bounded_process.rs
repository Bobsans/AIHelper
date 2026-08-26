#![cfg(windows)]

use std::{
    env,
    ffi::{OsStr, c_void},
    fs::File,
    io::{self, ErrorKind, Read},
    mem::{self, size_of},
    os::windows::{
        ffi::OsStrExt as _,
        io::{FromRawHandle as _, RawHandle},
        process::ExitStatusExt as _,
    },
    path::Path,
    process::ExitStatus,
    ptr::{null, null_mut},
    thread,
    time::{Duration, Instant},
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
        Threading::{
            CREATE_NO_WINDOW, CREATE_UNICODE_ENVIRONMENT, CreateProcessW,
            DeleteProcThreadAttributeList, EXTENDED_STARTUPINFO_PRESENT, GetExitCodeProcess,
            INFINITE, InitializeProcThreadAttributeList, PROC_THREAD_ATTRIBUTE_HANDLE_LIST,
            PROC_THREAD_ATTRIBUTE_JOB_LIST, PROCESS_INFORMATION, STARTF_USESTDHANDLES,
            STARTUPINFOEXW, UpdateProcThreadAttribute, WaitForSingleObject,
        },
    },
};

const POLL_INTERVAL: Duration = Duration::from_millis(5);
pub(crate) struct EnvironmentOverride<'a> {
    pub(crate) name: &'a OsStr,
    pub(crate) value: Option<&'a OsStr>,
}

pub(crate) struct Output {
    pub(crate) status: ExitStatus,
    pub(crate) timed_out: bool,
    pub(crate) stdout: Vec<u8>,
    pub(crate) stderr: Vec<u8>,
    pub(crate) stdout_truncated: bool,
    pub(crate) stderr_truncated: bool,
}

pub(crate) fn run(
    program: &Path,
    arguments: &[&str],
    cwd: &Path,
    environment: &[EnvironmentOverride<'_>],
    timeout: Duration,
    maximum_output: usize,
) -> io::Result<Output> {
    let started = Instant::now();
    let mut child = spawn(program, arguments, cwd, environment)?;
    let stdout = child
        .take_stdout()
        .map(|reader| thread::spawn(move || capture(reader, maximum_output)));
    let stderr = child
        .take_stderr()
        .map(|reader| thread::spawn(move || capture(reader, maximum_output)));

    let mut timed_out = false;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if started.elapsed() >= timeout {
            timed_out = true;
            if let Err(error) = child.kill()
                && error.kind() != ErrorKind::InvalidInput
            {
                return Err(error);
            }
            break child.wait()?;
        }
        thread::sleep(POLL_INTERVAL.min(timeout.saturating_sub(started.elapsed())));
    };

    let stdout = join_capture(stdout)?;
    let stderr = join_capture(stderr)?;
    Ok(Output {
        status,
        timed_out,
        stdout: stdout.bytes,
        stderr: stderr.bytes,
        stdout_truncated: stdout.truncated,
        stderr_truncated: stderr.truncated,
    })
}

fn join_capture(handle: Option<thread::JoinHandle<io::Result<Capture>>>) -> io::Result<Capture> {
    match handle {
        Some(handle) => handle
            .join()
            .map_err(|_| io::Error::other("process output reader panicked"))?,
        None => Ok(Capture {
            bytes: Vec::new(),
            truncated: false,
        }),
    }
}

fn capture(mut reader: File, maximum: usize) -> io::Result<Capture> {
    let mut bytes = Vec::with_capacity(maximum.min(8 * 1024));
    let mut buffer = [0_u8; 8 * 1024];
    let mut total = 0usize;
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        total = total.saturating_add(read);
        if bytes.len() < maximum {
            let remaining = maximum - bytes.len();
            bytes.extend_from_slice(&buffer[..read.min(remaining)]);
        }
    }
    Ok(Capture {
        bytes,
        truncated: total > maximum,
    })
}

struct Capture {
    bytes: Vec<u8>,
    truncated: bool,
}

struct Child {
    process: OwnedHandle,
    job: OwnedHandle,
    stdout: Option<File>,
    stderr: Option<File>,
    root_status: Option<ExitStatus>,
}

impl Child {
    fn take_stdout(&mut self) -> Option<File> {
        self.stdout.take()
    }

    fn take_stderr(&mut self) -> Option<File> {
        self.stderr.take()
    }

    fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        self.refresh_root_status()?;
        if self.root_status.is_some() && active_processes(self.job.raw())? == 0 {
            Ok(self.root_status)
        } else {
            Ok(None)
        }
    }

    fn kill(&mut self) -> io::Result<()> {
        if active_processes(self.job.raw())? == 0 {
            return Err(io::Error::new(
                ErrorKind::InvalidInput,
                "process tree has already exited",
            ));
        }
        win32_bool(unsafe { TerminateJobObject(self.job.raw(), 1) })
    }

    fn wait(&mut self) -> io::Result<ExitStatus> {
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

fn spawn(
    program: &Path,
    arguments: &[&str],
    cwd: &Path,
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

    let application = wide_null(program.as_os_str())?;
    let current_directory = wide_null(cwd.as_os_str())?;
    let environment = environment_block(environment_overrides)?;
    let mut command_line = command_line(program.as_os_str(), arguments)?;
    let mut process_info: PROCESS_INFORMATION = unsafe { mem::zeroed() };
    let spawn_guard = ah_platform::exec::hold_create_process_lock();
    let inherit_guard = InheritGuard::new(&inherited_handles)?;
    let created = unsafe {
        CreateProcessW(
            application.as_ptr(),
            command_line.as_mut_ptr(),
            null(),
            null(),
            1,
            EXTENDED_STARTUPINFO_PRESENT | CREATE_UNICODE_ENVIRONMENT | CREATE_NO_WINDOW,
            environment.as_ptr().cast(),
            current_directory.as_ptr(),
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

fn environment_block(overrides: &[EnvironmentOverride<'_>]) -> io::Result<Vec<u16>> {
    validate_environment_overrides(overrides)?;
    let mut entries = env::vars_os()
        .filter(|(name, _)| {
            !overrides.iter().any(|entry| {
                name.to_string_lossy()
                    .eq_ignore_ascii_case(&entry.name.to_string_lossy())
            })
        })
        .collect::<Vec<_>>();
    entries.extend(overrides.iter().filter_map(|entry| {
        entry
            .value
            .map(|value| (entry.name.to_os_string(), value.to_os_string()))
    }));
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
                ErrorKind::InvalidInput,
                "process environment contains a NUL character",
            ));
        }
        block.extend(name);
        block.push(b'=' as u16);
        block.extend(value);
        block.push(0);
    }
    block.push(0);
    Ok(block)
}

fn validate_environment_overrides(overrides: &[EnvironmentOverride<'_>]) -> io::Result<()> {
    for (index, entry) in overrides.iter().enumerate() {
        let name = entry.name.encode_wide().collect::<Vec<_>>();
        if name.is_empty() || name.contains(&0) || name.contains(&(b'=' as u16)) {
            return Err(io::Error::new(
                ErrorKind::InvalidInput,
                "invalid process environment variable name",
            ));
        }
        if entry
            .value
            .is_some_and(|value| value.encode_wide().any(|unit| unit == 0))
        {
            return Err(io::Error::new(
                ErrorKind::InvalidInput,
                "process environment variable contains a NUL character",
            ));
        }
        if overrides[..index].iter().any(|previous| {
            previous
                .name
                .to_string_lossy()
                .eq_ignore_ascii_case(&entry.name.to_string_lossy())
        }) {
            return Err(io::Error::new(
                ErrorKind::InvalidInput,
                "duplicate process environment variable override",
            ));
        }
    }
    Ok(())
}

fn command_line(program: &OsStr, arguments: &[&str]) -> io::Result<Vec<u16>> {
    let mut output = Vec::new();
    append_quoted(&mut output, program)?;
    for argument in arguments {
        output.push(b' ' as u16);
        append_quoted(&mut output, OsStr::new(argument))?;
    }
    output.push(0);
    Ok(output)
}

fn append_quoted(output: &mut Vec<u16>, value: &OsStr) -> io::Result<()> {
    let units = value.encode_wide().collect::<Vec<_>>();
    if units.contains(&0) {
        return Err(io::Error::new(
            ErrorKind::InvalidInput,
            "process argument contains an embedded NUL",
        ));
    }
    output.push(b'"' as u16);
    let mut backslashes = 0usize;
    for unit in units {
        if unit == b'\\' as u16 {
            backslashes += 1;
        } else if unit == b'"' as u16 {
            output.extend(std::iter::repeat_n(b'\\' as u16, backslashes * 2 + 1));
            output.push(unit);
            backslashes = 0;
        } else {
            output.extend(std::iter::repeat_n(b'\\' as u16, backslashes));
            output.push(unit);
            backslashes = 0;
        }
    }
    output.extend(std::iter::repeat_n(b'\\' as u16, backslashes * 2));
    output.push(b'"' as u16);
    Ok(())
}

fn wide_null(value: &OsStr) -> io::Result<Vec<u16>> {
    let mut wide = value.encode_wide().collect::<Vec<_>>();
    if wide.contains(&0) {
        return Err(io::Error::new(
            ErrorKind::InvalidInput,
            "process path contains an embedded NUL",
        ));
    }
    wide.push(0);
    Ok(wide)
}

struct AttributeList {
    _storage: Vec<usize>,
    raw: *mut c_void,
}

impl AttributeList {
    fn new(count: u32) -> io::Result<Self> {
        let mut bytes = 0usize;
        unsafe { InitializeProcThreadAttributeList(null_mut(), count, 0, &mut bytes) };
        if bytes == 0 {
            return Err(io::Error::last_os_error());
        }
        let mut storage = vec![0usize; bytes.div_ceil(size_of::<usize>())];
        let raw = storage.as_mut_ptr().cast();
        win32_bool(unsafe { InitializeProcThreadAttributeList(raw, count, 0, &mut bytes) })?;
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

struct InheritGuard<'a>(&'a [HANDLE]);

impl<'a> InheritGuard<'a> {
    fn new(handles: &'a [HANDLE]) -> io::Result<Self> {
        for (index, handle) in handles.iter().enumerate() {
            if unsafe { SetHandleInformation(*handle, HANDLE_FLAG_INHERIT, HANDLE_FLAG_INHERIT) }
                == 0
            {
                for inherited in &handles[..index] {
                    unsafe { SetHandleInformation(*inherited, HANDLE_FLAG_INHERIT, 0) };
                }
                return Err(io::Error::last_os_error());
            }
        }
        Ok(Self(handles))
    }
}

impl Drop for InheritGuard<'_> {
    fn drop(&mut self) {
        for handle in self.0 {
            unsafe { SetHandleInformation(*handle, HANDLE_FLAG_INHERIT, 0) };
        }
    }
}

struct OwnedHandle(HANDLE);

impl OwnedHandle {
    fn new(handle: HANDLE) -> io::Result<Self> {
        if handle.is_null() {
            Err(io::Error::last_os_error())
        } else {
            Ok(Self(handle))
        }
    }

    fn raw(&self) -> HANDLE {
        self.0
    }

    fn into_raw(mut self) -> HANDLE {
        let handle = self.0;
        self.0 = null_mut();
        handle
    }
}

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { CloseHandle(self.0) };
        }
    }
}

fn owned_file(handle: OwnedHandle) -> File {
    unsafe { File::from_raw_handle(handle.into_raw() as RawHandle) }
}

fn win32_bool(result: i32) -> io::Result<()> {
    if result == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_drains_but_bounds_output() {
        let temp = tempfile::tempfile().unwrap();
        temp.set_len(16).unwrap();
        let captured = capture(temp, 4).unwrap();
        assert_eq!(captured.bytes.len(), 4);
        assert!(captured.truncated);
    }

    #[test]
    fn timeout_terminates_the_job() {
        let system_root = env::var_os("SystemRoot").unwrap();
        let command = Path::new(&system_root).join("System32/ping.exe");
        let cwd = tempfile::tempdir().unwrap();
        let started = Instant::now();

        let output = run(
            &command,
            &["-n", "30", "127.0.0.1"],
            cwd.path(),
            &[],
            Duration::from_millis(100),
            1024,
        )
        .unwrap();

        assert!(output.timed_out);
        assert!(started.elapsed() < Duration::from_secs(5));
    }
}
