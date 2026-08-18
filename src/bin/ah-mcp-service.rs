#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(windows)]
fn main() {
    let exit_code = windows_launcher::run().unwrap_or(1);
    std::process::exit(exit_code as i32);
}

#[cfg(not(windows))]
fn main() {
    eprintln!("ah-mcp-service is only supported on Windows");
    std::process::exit(1);
}

#[cfg(windows)]
mod windows_launcher {
    use std::{
        env,
        ffi::{OsStr, OsString, c_void},
        io,
        mem::{self, size_of},
        os::windows::ffi::OsStrExt,
        path::{Path, PathBuf},
        ptr::{null, null_mut},
        thread,
        time::Duration,
    };

    use windows_sys::Win32::{
        Foundation::{CloseHandle, HANDLE, WAIT_FAILED, WAIT_OBJECT_0},
        System::{
            JobObjects::{
                CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
                JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
                SetInformationJobObject,
            },
            Threading::{
                CREATE_NO_WINDOW, CreateProcessW, DeleteProcThreadAttributeList,
                EXTENDED_STARTUPINFO_PRESENT, GetExitCodeProcess, INFINITE,
                InitializeProcThreadAttributeList, PROC_THREAD_ATTRIBUTE_JOB_LIST,
                PROCESS_INFORMATION, STARTUPINFOEXW, UpdateProcThreadAttribute,
                WaitForSingleObject,
            },
        },
    };

    const SUPERVISOR_PID_ENV: &str = "AH_MCP_SERVICE_SUPERVISOR_PID";
    const RETRY_COUNT: usize = 3;
    const RETRY_INTERVAL: Duration = Duration::from_secs(60);

    pub(super) fn run() -> io::Result<u32> {
        let executable = sibling_ah(&env::current_exe()?);
        if !executable.is_file() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!(
                    "managed MCP executable '{}' does not exist",
                    executable.display()
                ),
            ));
        }

        let arguments = env::args_os().skip(1).collect::<Vec<_>>();
        run_with_retry(
            || run_once(&executable, &arguments),
            |delay| thread::sleep(delay),
        )
    }

    fn run_with_retry(
        mut attempt: impl FnMut() -> io::Result<u32>,
        mut wait: impl FnMut(Duration),
    ) -> io::Result<u32> {
        for index in 0..=RETRY_COUNT {
            match attempt() {
                Ok(0) => return Ok(0),
                result if index == RETRY_COUNT => return result,
                Ok(_) | Err(_) => wait(RETRY_INTERVAL),
            }
        }
        unreachable!("bounded retry loop always returns")
    }

    fn run_once(executable: &Path, arguments: &[OsString]) -> io::Result<u32> {
        let mut command_line = command_line(&executable, &arguments)?;
        let application = wide_null(executable.as_os_str())?;
        let job = create_job()?;
        let mut attributes = AttributeList::new(1)?;
        let job_handle = job.raw();
        attributes.set(
            PROC_THREAD_ATTRIBUTE_JOB_LIST as usize,
            (&job_handle as *const HANDLE).cast_mut().cast(),
            size_of::<HANDLE>(),
        )?;

        let mut startup: STARTUPINFOEXW = unsafe { mem::zeroed() };
        startup.StartupInfo.cb =
            u32::try_from(size_of::<STARTUPINFOEXW>()).expect("STARTUPINFOEXW size should fit");
        startup.lpAttributeList = attributes.raw();
        let mut process_info: PROCESS_INFORMATION = unsafe { mem::zeroed() };

        // SAFETY: the launcher is single-threaded and the child inherits the
        // environment immediately below. The server removes this private value
        // during managed preflight before it can start worker threads.
        unsafe { env::set_var(SUPERVISOR_PID_ENV, std::process::id().to_string()) };
        let created = unsafe {
            CreateProcessW(
                application.as_ptr(),
                command_line.as_mut_ptr(),
                null(),
                null(),
                0,
                EXTENDED_STARTUPINFO_PRESENT | CREATE_NO_WINDOW,
                null(),
                null(),
                &startup.StartupInfo,
                &mut process_info,
            )
        };
        win32_bool(created)?;

        let process = OwnedHandle::new(process_info.hProcess)?;
        let thread = OwnedHandle::new(process_info.hThread)?;
        drop(thread);
        match unsafe { WaitForSingleObject(process.raw(), INFINITE) } {
            WAIT_OBJECT_0 => {}
            WAIT_FAILED => return Err(io::Error::last_os_error()),
            result => {
                return Err(io::Error::other(format!(
                    "unexpected process wait result {result}"
                )));
            }
        }
        let mut exit_code = 0;
        win32_bool(unsafe { GetExitCodeProcess(process.raw(), &mut exit_code) })?;
        Ok(exit_code)
    }

    fn sibling_ah(launcher: &Path) -> PathBuf {
        launcher.with_file_name("ah.exe")
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
                    .expect("job information size should fit"),
            )
        })?;
        Ok(job)
    }

    fn command_line(program: &Path, arguments: &[OsString]) -> io::Result<Vec<u16>> {
        let mut output = Vec::new();
        append_quoted(&mut output, program.as_os_str())?;
        for argument in arguments {
            output.push(b' ' as u16);
            append_quoted(&mut output, argument)?;
        }
        output.push(0);
        Ok(output)
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

    fn wide_null(value: &OsStr) -> io::Result<Vec<u16>> {
        let mut value = value.encode_wide().collect::<Vec<_>>();
        if value.contains(&0) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "process path contains a NUL character",
            ));
        }
        value.push(0);
        Ok(value)
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
                UpdateProcThreadAttribute(
                    self.raw,
                    0,
                    attribute,
                    value,
                    bytes,
                    null_mut(),
                    null_mut(),
                )
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
        use super::*;

        #[test]
        fn retry_runner_stops_immediately_after_success() {
            let mut attempts = 0;
            let mut delays = Vec::new();

            let exit = run_with_retry(
                || {
                    attempts += 1;
                    Ok(0)
                },
                |delay| delays.push(delay),
            )
            .unwrap();

            assert_eq!(exit, 0);
            assert_eq!(attempts, 1);
            assert!(delays.is_empty());
        }

        #[test]
        fn retry_runner_allows_three_retries_after_the_initial_failure() {
            let mut attempts = 0;
            let mut delays = Vec::new();

            let exit = run_with_retry(
                || {
                    attempts += 1;
                    Ok(if attempts == 4 { 0 } else { 1 })
                },
                |delay| delays.push(delay),
            )
            .unwrap();

            assert_eq!(exit, 0);
            assert_eq!(attempts, 4);
            assert_eq!(delays, vec![RETRY_INTERVAL; 3]);
        }

        #[test]
        fn retry_runner_stops_after_the_fourth_failure() {
            let mut attempts = 0;
            let mut delays = Vec::new();

            let exit = run_with_retry(
                || {
                    attempts += 1;
                    Ok(1)
                },
                |delay| delays.push(delay),
            )
            .unwrap();

            assert_eq!(exit, 1);
            assert_eq!(attempts, 4);
            assert_eq!(delays, vec![RETRY_INTERVAL; 3]);
        }

        #[test]
        fn resolves_sibling_ah() {
            assert_eq!(
                sibling_ah(Path::new(r"C:\tools\ah-mcp-service.exe")),
                Path::new(r"C:\tools\ah.exe")
            );
        }

        #[test]
        fn command_line_preserves_windows_arguments() {
            let arguments = vec![
                OsString::from(""),
                OsString::from("two words"),
                OsString::from("quote\"inside"),
                OsString::from("ends with slash\\"),
            ];
            let encoded = command_line(Path::new("ah.exe"), &arguments).unwrap();
            let decoded = String::from_utf16(&encoded[..encoded.len() - 1]).unwrap();
            assert_eq!(
                decoded,
                "ah.exe \"\" \"two words\" \"quote\\\"inside\" \"ends with slash\\\\\""
            );
        }
    }
}
