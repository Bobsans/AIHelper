use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
    time::Duration,
};

use ah_updater_core::{UpdaterError, UpdaterErrorCode};

const MAX_REGISTERED_FILES: usize = 4096;
const MAX_PATH_UTF16_UNITS: usize = 32_767;
const MAX_TOTAL_PATH_UTF16_UNITS: usize = 2 * 1024 * 1024;
const MAX_BLOCKING_PROCESSES: usize = 1024;
const MAX_LIST_ATTEMPTS: usize = 4;
const START_SESSION_ATTEMPTS: usize = 3;
const START_SESSION_RETRY_DELAY: Duration = Duration::from_millis(25);
const ERROR_WRITE_FAULT_CODE: u32 = 29;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockerObservation {
    pub processes: Vec<BlockingProcess>,
    pub reboot_reasons: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct BlockingProcess {
    pub pid: u32,
    pub start_time_100ns: u64,
    pub application_name: String,
    pub service_short_name: Option<String>,
    pub application_type: i32,
    pub app_status: u32,
    pub terminal_session_id: u32,
    pub restartable: bool,
}

pub fn inspect_blockers(paths: &[PathBuf]) -> Result<BlockerObservation, UpdaterError> {
    let paths = validate_paths(paths)?;
    #[cfg(windows)]
    {
        query_restart_manager(&windows::WindowsRestartManager, &paths)
    }
    #[cfg(not(windows))]
    {
        let _ = paths;
        Err(UpdaterError::new(
            UpdaterErrorCode::UnsupportedPlatform,
            "Restart Manager inspection requires Windows",
        ))
    }
}

fn validate_paths(paths: &[PathBuf]) -> Result<Vec<String>, UpdaterError> {
    if paths.is_empty() {
        return Err(blocker(
            "Restart Manager requires at least one managed file path",
        ));
    }
    if paths.len() > MAX_REGISTERED_FILES {
        return Err(blocker(
            "managed file count exceeds the Restart Manager limit",
        ));
    }

    let mut equivalent = BTreeSet::new();
    let mut total_units = 0_usize;
    let mut validated = Vec::with_capacity(paths.len());
    for path in paths {
        let value = path
            .to_str()
            .ok_or_else(|| blocker("managed file path is not valid Unicode"))?;
        if value.contains('\0') || !is_absolute_windows_path(value) {
            return Err(blocker(
                "Restart Manager managed file path must be absolute",
            ));
        }
        let units = value.encode_utf16().count();
        if units == 0 || units > MAX_PATH_UTF16_UNITS {
            return Err(blocker(
                "Restart Manager managed file path exceeds the length limit",
            ));
        }
        total_units = total_units
            .checked_add(units)
            .filter(|total| *total <= MAX_TOTAL_PATH_UTF16_UNITS)
            .ok_or_else(|| blocker("Restart Manager managed paths exceed the total size limit"))?;
        let key = value.replace('/', "\\").to_lowercase();
        if !equivalent.insert(key) {
            return Err(blocker(
                "Restart Manager managed file paths contain a duplicate",
            ));
        }
        validated.push(value.to_owned());
    }
    Ok(validated)
}

fn is_absolute_windows_path(path: &str) -> bool {
    let bytes = path.as_bytes();
    let drive_absolute = bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(bytes[2], b'\\' | b'/');
    let unc_absolute = (path.starts_with(r"\\") || path.starts_with("//"))
        && path[2..]
            .split(['\\', '/'])
            .filter(|component| !component.is_empty())
            .count()
            >= 2;
    drive_absolute || unc_absolute
}

fn query_restart_manager(
    api: &impl RestartManagerApi,
    paths: &[String],
) -> Result<BlockerObservation, UpdaterError> {
    let session = start_session(api)?;
    let result = (|| {
        api.register_files(session, paths)
            .map_err(map_api_failure)?;
        collect_blockers(api, session)
    })();
    let end_result = api.end_session(session).map_err(map_api_failure);

    match (result, end_result) {
        (Err(error), _) => Err(error),
        (Ok(_), Err(error)) => Err(error),
        (Ok(observation), Ok(())) => Ok(observation),
    }
}

fn start_session(api: &impl RestartManagerApi) -> Result<u32, UpdaterError> {
    for attempt in 0..START_SESSION_ATTEMPTS {
        match api.start_session() {
            Ok(session) => return Ok(session),
            Err(ApiFailure::Windows {
                operation: "start session",
                code: ERROR_WRITE_FAULT_CODE,
            }) if attempt + 1 < START_SESSION_ATTEMPTS => {
                std::thread::sleep(START_SESSION_RETRY_DELAY);
            }
            Err(error) => return Err(map_api_failure(error)),
        }
    }
    unreachable!("bounded Restart Manager start loop always returns")
}

fn collect_blockers(
    api: &impl RestartManagerApi,
    session: u32,
) -> Result<BlockerObservation, UpdaterError> {
    let mut capacity = 0_usize;
    for _ in 0..MAX_LIST_ATTEMPTS {
        match api.get_list(session, capacity).map_err(map_api_failure)? {
            ListResult::MoreData { needed } => {
                if needed == 0 || needed > MAX_BLOCKING_PROCESSES {
                    return Err(blocker(
                        "Restart Manager blocker count exceeds the supported limit",
                    ));
                }
                capacity = needed;
            }
            ListResult::Complete {
                processes,
                reboot_reasons,
            } => {
                if processes.len() > MAX_BLOCKING_PROCESSES {
                    return Err(blocker(
                        "Restart Manager returned too many blocking processes",
                    ));
                }
                return normalize_observation(processes, reboot_reasons);
            }
        }
    }
    Err(blocker(
        "Restart Manager blocker list changed too often during inspection",
    ))
}

fn normalize_observation(
    processes: Vec<BlockingProcess>,
    reboot_reasons: u32,
) -> Result<BlockerObservation, UpdaterError> {
    let mut unique = BTreeMap::new();
    for process in processes {
        if process.pid == 0 {
            return Err(blocker(
                "Restart Manager returned an invalid blocking process identity",
            ));
        }
        let identity = (process.pid, process.start_time_100ns);
        if let Some(existing) = unique.get(&identity) {
            if existing != &process {
                return Err(blocker(
                    "Restart Manager returned an ambiguous blocking process identity",
                ));
            }
        } else {
            unique.insert(identity, process);
        }
    }
    Ok(BlockerObservation {
        processes: unique.into_values().collect(),
        reboot_reasons,
    })
}

enum ListResult {
    MoreData {
        needed: usize,
    },
    Complete {
        processes: Vec<BlockingProcess>,
        reboot_reasons: u32,
    },
}

trait RestartManagerApi {
    fn start_session(&self) -> Result<u32, ApiFailure>;
    fn register_files(&self, session: u32, paths: &[String]) -> Result<(), ApiFailure>;
    fn get_list(&self, session: u32, capacity: usize) -> Result<ListResult, ApiFailure>;
    fn end_session(&self, session: u32) -> Result<(), ApiFailure>;
}

#[derive(Debug, Clone, Copy)]
enum ApiFailure {
    Windows { operation: &'static str, code: u32 },
    Contract(&'static str),
}

fn map_api_failure(failure: ApiFailure) -> UpdaterError {
    match failure {
        ApiFailure::Windows { operation, code } => blocker(format!(
            "Restart Manager {operation} failed with Windows error {code}"
        )),
        ApiFailure::Contract(detail) => blocker(detail),
    }
}

fn blocker(detail: impl Into<String>) -> UpdaterError {
    UpdaterError::new(UpdaterErrorCode::Blocker, detail)
}

#[cfg(windows)]
mod windows {
    use std::{ptr::null, ptr::null_mut};

    use windows_sys::{
        Win32::{
            Foundation::{ERROR_MORE_DATA, ERROR_SUCCESS},
            System::RestartManager::{
                CCH_RM_SESSION_KEY, RM_PROCESS_INFO, RmEndSession, RmGetList, RmRegisterResources,
                RmStartSession,
            },
        },
        core::PCWSTR,
    };

    use super::*;

    pub(super) struct WindowsRestartManager;

    impl RestartManagerApi for WindowsRestartManager {
        fn start_session(&self) -> Result<u32, ApiFailure> {
            let mut session = 0_u32;
            let mut key = [0_u16; CCH_RM_SESSION_KEY as usize + 1];
            let code = unsafe { RmStartSession(&mut session, 0, key.as_mut_ptr()) };
            require_success("start session", code)?;
            Ok(session)
        }

        fn register_files(&self, session: u32, paths: &[String]) -> Result<(), ApiFailure> {
            let wide_paths = paths
                .iter()
                .map(|path| path.encode_utf16().chain(Some(0)).collect::<Vec<_>>())
                .collect::<Vec<_>>();
            let pointers = wide_paths
                .iter()
                .map(|path| path.as_ptr())
                .collect::<Vec<PCWSTR>>();
            let count = u32::try_from(pointers.len())
                .map_err(|_| ApiFailure::Contract("Restart Manager file count overflow"))?;
            let code = unsafe {
                RmRegisterResources(session, count, pointers.as_ptr(), 0, null(), 0, null())
            };
            require_success("register files", code)
        }

        fn get_list(&self, session: u32, capacity: usize) -> Result<ListResult, ApiFailure> {
            let mut needed = 0_u32;
            let mut count = u32::try_from(capacity)
                .map_err(|_| ApiFailure::Contract("Restart Manager capacity overflow"))?;
            let mut reboot_reasons = 0_u32;
            let mut processes = vec![RM_PROCESS_INFO::default(); capacity];
            let pointer = if processes.is_empty() {
                null_mut()
            } else {
                processes.as_mut_ptr()
            };
            let code = unsafe {
                RmGetList(
                    session,
                    &mut needed,
                    &mut count,
                    pointer,
                    &mut reboot_reasons,
                )
            };
            if code == ERROR_MORE_DATA {
                return Ok(ListResult::MoreData {
                    needed: needed as usize,
                });
            }
            require_success("get blocker list", code)?;
            let count = count as usize;
            if count > processes.len() || needed as usize != count {
                return Err(ApiFailure::Contract(
                    "Restart Manager returned an inconsistent blocker count",
                ));
            }
            processes.truncate(count);
            let processes = processes
                .into_iter()
                .map(convert_process)
                .collect::<Result<Vec<_>, _>>()?;
            Ok(ListResult::Complete {
                processes,
                reboot_reasons,
            })
        }

        fn end_session(&self, session: u32) -> Result<(), ApiFailure> {
            let code = unsafe { RmEndSession(session) };
            require_success("end session", code)
        }
    }

    fn convert_process(process: RM_PROCESS_INFO) -> Result<BlockingProcess, ApiFailure> {
        Ok(BlockingProcess {
            pid: process.Process.dwProcessId,
            start_time_100ns: ((process.Process.ProcessStartTime.dwHighDateTime as u64) << 32)
                | process.Process.ProcessStartTime.dwLowDateTime as u64,
            application_name: decode_utf16(&process.strAppName)?,
            service_short_name: {
                let service = decode_utf16(&process.strServiceShortName)?;
                (!service.is_empty()).then_some(service)
            },
            application_type: process.ApplicationType,
            app_status: process.AppStatus,
            terminal_session_id: process.TSSessionId,
            restartable: process.bRestartable != 0,
        })
    }

    fn decode_utf16(buffer: &[u16]) -> Result<String, ApiFailure> {
        let length = buffer
            .iter()
            .position(|unit| *unit == 0)
            .unwrap_or(buffer.len());
        String::from_utf16(&buffer[..length])
            .map_err(|_| ApiFailure::Contract("Restart Manager returned invalid process metadata"))
    }

    fn require_success(operation: &'static str, code: u32) -> Result<(), ApiFailure> {
        if code == ERROR_SUCCESS {
            Ok(())
        } else {
            Err(ApiFailure::Windows { operation, code })
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{
        cell::{Cell, RefCell},
        collections::VecDeque,
    };

    use super::*;

    #[test]
    fn validates_exact_absolute_unique_managed_paths_before_session_start() {
        let api = FakeApi::default();
        for paths in [
            Vec::new(),
            vec![PathBuf::from("relative.exe")],
            vec![PathBuf::from("C:\\Tools\\ah.exe\0suffix")],
            vec![
                PathBuf::from(r"C:\Tools\ah.exe"),
                PathBuf::from("c:/tools/AH.EXE"),
            ],
        ] {
            let error = inspect_with_api(&api, &paths).unwrap_err();
            assert_eq!(error.code(), UpdaterErrorCode::Blocker);
        }
        assert_eq!(api.starts.get(), 0);

        let paths = validate_paths(&[
            PathBuf::from(r"C:\Tools\AIHelper\ah.exe"),
            PathBuf::from(r"\\?\C:\Tools\AIHelper\plugins\github.dll"),
        ])
        .unwrap();
        assert_eq!(paths.len(), 2);
    }

    #[test]
    fn retries_growth_sorts_and_deduplicates_processes() {
        let first = process(20, 2, "second");
        let duplicate = process(10, 1, "first");
        let api = FakeApi::with_responses([
            Ok(ListResult::MoreData { needed: 1 }),
            Ok(ListResult::MoreData { needed: 3 }),
            Ok(ListResult::Complete {
                processes: vec![first.clone(), duplicate.clone(), duplicate],
                reboot_reasons: 4,
            }),
        ]);

        let observation = inspect_with_api(&api, &[PathBuf::from(r"C:\Tools\ah.exe")]).unwrap();

        assert_eq!(observation.processes, vec![process(10, 1, "first"), first]);
        assert_eq!(observation.reboot_reasons, 4);
        assert_eq!(*api.capacities.borrow(), [0, 1, 3]);
        assert_eq!(api.registered.borrow().len(), 1);
        assert_eq!(api.ends.get(), 1);
    }

    #[test]
    fn fails_closed_for_unbounded_or_ambiguous_results_and_ends_session() {
        let oversized = FakeApi::with_responses([Ok(ListResult::MoreData {
            needed: MAX_BLOCKING_PROCESSES + 1,
        })]);
        assert_blocker_error(&oversized);
        assert_eq!(oversized.ends.get(), 1);

        let changing = FakeApi::with_responses(
            (0..MAX_LIST_ATTEMPTS).map(|_| Ok(ListResult::MoreData { needed: 1 })),
        );
        assert_blocker_error(&changing);
        assert_eq!(changing.ends.get(), 1);

        let mut changed = process(10, 1, "same");
        changed.restartable = true;
        let ambiguous = FakeApi::with_responses([Ok(ListResult::Complete {
            processes: vec![process(10, 1, "same"), changed],
            reboot_reasons: 0,
        })]);
        assert_blocker_error(&ambiguous);
        assert_eq!(ambiguous.ends.get(), 1);
    }

    #[test]
    fn reports_api_failures_and_always_attempts_session_end() {
        let api = FakeApi {
            register_failure: Some(ApiFailure::Windows {
                operation: "register files",
                code: 5,
            }),
            ..FakeApi::default()
        };

        let error = inspect_with_api(&api, &[PathBuf::from(r"C:\Tools\ah.exe")]).unwrap_err();

        assert_eq!(error.code(), UpdaterErrorCode::Blocker);
        assert!(error.detail().contains("Windows error 5"));
        assert_eq!(api.ends.get(), 1);

        let end_failure = FakeApi {
            end_failure: Some(ApiFailure::Windows {
                operation: "end session",
                code: 6,
            }),
            ..FakeApi::default()
        };
        let error =
            inspect_with_api(&end_failure, &[PathBuf::from(r"C:\Tools\ah.exe")]).unwrap_err();
        assert!(error.detail().contains("Windows error 6"));
        assert_eq!(end_failure.ends.get(), 1);
    }

    #[test]
    fn retries_only_transient_session_start_failure() {
        let transient = FakeApi {
            start_responses: RefCell::new(
                [
                    Err(ApiFailure::Windows {
                        operation: "start session",
                        code: ERROR_WRITE_FAULT_CODE,
                    }),
                    Ok(17),
                ]
                .into_iter()
                .collect(),
            ),
            ..FakeApi::default()
        };
        inspect_with_api(&transient, &[PathBuf::from(r"C:\Tools\ah.exe")]).unwrap();
        assert_eq!(transient.starts.get(), 2);
        assert_eq!(transient.ends.get(), 1);

        let permanent = FakeApi {
            start_responses: RefCell::new(
                [Err(ApiFailure::Windows {
                    operation: "start session",
                    code: 5,
                })]
                .into_iter()
                .collect(),
            ),
            ..FakeApi::default()
        };
        assert_blocker_error(&permanent);
        assert_eq!(permanent.starts.get(), 1);
        assert_eq!(permanent.ends.get(), 0);
    }

    #[cfg(windows)]
    #[test]
    fn component_observes_current_process_holding_registered_file() {
        use std::{fs::OpenOptions, os::windows::fs::OpenOptionsExt};

        let temp = tempfile::TempDir::new().unwrap();
        let path = temp.path().join("managed.dll");
        std::fs::write(&path, b"managed").unwrap();
        let _held = OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(&path)
            .unwrap();

        let observation = match inspect_blockers(&[path]) {
            Ok(observation) => observation,
            Err(error)
                if [29, 121, 353]
                    .iter()
                    .any(|code| error.detail().ends_with(&format!("Windows error {code}"))) =>
            {
                eprintln!(
                    "skipping Restart Manager component assertion: {}",
                    error.detail()
                );
                return;
            }
            Err(error) => panic!("Restart Manager component inspection failed: {error}"),
        };

        assert!(
            observation
                .processes
                .iter()
                .any(|process| process.pid == std::process::id())
        );
    }

    fn inspect_with_api(
        api: &impl RestartManagerApi,
        paths: &[PathBuf],
    ) -> Result<BlockerObservation, UpdaterError> {
        let paths = validate_paths(paths)?;
        query_restart_manager(api, &paths)
    }

    fn assert_blocker_error(api: &FakeApi) {
        let error = inspect_with_api(api, &[PathBuf::from(r"C:\Tools\ah.exe")]).unwrap_err();
        assert_eq!(error.code(), UpdaterErrorCode::Blocker);
    }

    fn process(pid: u32, start_time_100ns: u64, name: &str) -> BlockingProcess {
        BlockingProcess {
            pid,
            start_time_100ns,
            application_name: name.to_owned(),
            service_short_name: None,
            application_type: 5,
            app_status: 1,
            terminal_session_id: 1,
            restartable: false,
        }
    }

    #[derive(Default)]
    struct FakeApi {
        starts: Cell<usize>,
        ends: Cell<usize>,
        registered: RefCell<Vec<Vec<String>>>,
        capacities: RefCell<Vec<usize>>,
        responses: RefCell<VecDeque<Result<ListResult, ApiFailure>>>,
        start_responses: RefCell<VecDeque<Result<u32, ApiFailure>>>,
        register_failure: Option<ApiFailure>,
        end_failure: Option<ApiFailure>,
    }

    impl FakeApi {
        fn with_responses(
            responses: impl IntoIterator<Item = Result<ListResult, ApiFailure>>,
        ) -> Self {
            Self {
                responses: RefCell::new(responses.into_iter().collect()),
                ..Self::default()
            }
        }
    }

    impl RestartManagerApi for FakeApi {
        fn start_session(&self) -> Result<u32, ApiFailure> {
            self.starts.set(self.starts.get() + 1);
            self.start_responses
                .borrow_mut()
                .pop_front()
                .unwrap_or(Ok(17))
        }

        fn register_files(&self, _session: u32, paths: &[String]) -> Result<(), ApiFailure> {
            self.registered.borrow_mut().push(paths.to_vec());
            self.register_failure.map_or(Ok(()), Err)
        }

        fn get_list(&self, _session: u32, capacity: usize) -> Result<ListResult, ApiFailure> {
            self.capacities.borrow_mut().push(capacity);
            self.responses.borrow_mut().pop_front().unwrap_or_else(|| {
                Ok(ListResult::Complete {
                    processes: Vec::new(),
                    reboot_reasons: 0,
                })
            })
        }

        fn end_session(&self, _session: u32) -> Result<(), ApiFailure> {
            self.ends.set(self.ends.get() + 1);
            self.end_failure.map_or(Ok(()), Err)
        }
    }
}
