use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    time::Duration,
};

use ah_updater_core::{FilePurpose, UpdaterError, UpdaterErrorCode};

use crate::{restart_manager, transaction::LoadedTransaction};

const POLL_INTERVAL: Duration = Duration::from_millis(100);

pub fn quiesce_transaction_blockers(
    transaction: &LoadedTransaction,
    grace: Duration,
) -> Result<(), UpdaterError> {
    #[cfg(windows)]
    {
        let managed_paths = managed_paths(transaction);
        let executable_paths = executable_paths(transaction);
        quiesce(
            &managed_paths,
            &executable_paths,
            grace,
            &windows::WindowsProcessControl,
        )
    }
    #[cfg(not(windows))]
    {
        let _ = (transaction, grace);
        Err(UpdaterError::new(
            UpdaterErrorCode::UnsupportedPlatform,
            "update blocker coordination requires Windows",
        ))
    }
}

#[cfg(windows)]
fn managed_paths(transaction: &LoadedTransaction) -> Vec<PathBuf> {
    transaction
        .plan()
        .operations
        .iter()
        .map(|operation| {
            transaction
                .paths()
                .installation_root()
                .join(operation.path())
        })
        .collect()
}

#[cfg(windows)]
fn executable_paths(transaction: &LoadedTransaction) -> BTreeSet<String> {
    transaction
        .old_manifest()
        .files
        .iter()
        .chain(&transaction.new_manifest().files)
        .filter(|file| file.purpose == FilePurpose::Executable)
        .map(|file| {
            normalize_path(
                &transaction
                    .paths()
                    .installation_root()
                    .join(file.path.split('/').collect::<PathBuf>()),
            )
        })
        .collect()
}

#[cfg(windows)]
fn quiesce(
    managed_paths: &[PathBuf],
    executable_paths: &BTreeSet<String>,
    grace: Duration,
    control: &impl ProcessControl,
) -> Result<(), UpdaterError> {
    quiesce_with(executable_paths, grace, control, || {
        restart_manager::inspect_blockers(managed_paths)
    })
}

fn quiesce_with(
    executable_paths: &BTreeSet<String>,
    grace: Duration,
    control: &impl ProcessControl,
    mut inspect: impl FnMut() -> Result<restart_manager::BlockerObservation, UpdaterError>,
) -> Result<(), UpdaterError> {
    let deadline = std::time::Instant::now() + grace;
    loop {
        let observation = inspect()?;
        let classified = classify(&observation.processes, executable_paths, control)?;
        reject_foreign(&classified)?;
        if classified.same_installation.is_empty() {
            return Ok(());
        }
        if std::time::Instant::now() >= deadline {
            for process in classified.same_installation {
                control.terminate(&process)?;
            }
            let remaining = inspect()?;
            let classified = classify(&remaining.processes, executable_paths, control)?;
            reject_foreign(&classified)?;
            return if classified.same_installation.is_empty() {
                Ok(())
            } else {
                Err(blocker(
                    "same-installation AIHelper process still blocks managed files",
                ))
            };
        }
        std::thread::sleep(
            POLL_INTERVAL.min(deadline.saturating_duration_since(std::time::Instant::now())),
        );
    }
}

#[derive(Debug)]
struct Classified {
    same_installation: Vec<restart_manager::BlockingProcess>,
    foreign: Vec<restart_manager::BlockingProcess>,
}

fn classify(
    processes: &[restart_manager::BlockingProcess],
    executable_paths: &BTreeSet<String>,
    control: &impl ProcessControl,
) -> Result<Classified, UpdaterError> {
    let mut classified = Classified {
        same_installation: Vec::new(),
        foreign: Vec::new(),
    };
    for process in processes {
        match control.inspect(process)? {
            ProcessIdentity::ExitedOrReused => {}
            ProcessIdentity::Running { executable } if executable_paths.contains(&executable) => {
                classified.same_installation.push(process.clone());
            }
            ProcessIdentity::Running { .. } | ProcessIdentity::Uninspectable => {
                classified.foreign.push(process.clone());
            }
        }
    }
    Ok(classified)
}

fn reject_foreign(classified: &Classified) -> Result<(), UpdaterError> {
    if classified.foreign.is_empty() {
        Ok(())
    } else {
        Err(blocker("foreign process blocks managed update files"))
    }
}

trait ProcessControl {
    fn inspect(
        &self,
        process: &restart_manager::BlockingProcess,
    ) -> Result<ProcessIdentity, UpdaterError>;

    fn terminate(&self, process: &restart_manager::BlockingProcess) -> Result<(), UpdaterError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ProcessIdentity {
    ExitedOrReused,
    Running { executable: String },
    Uninspectable,
}

fn normalize_path(path: &Path) -> String {
    let value = path.to_string_lossy();
    value
        .strip_prefix(r"\\?\UNC\")
        .map(|path| format!(r"\\{path}"))
        .or_else(|| value.strip_prefix(r"\\?\").map(str::to_owned))
        .unwrap_or_else(|| value.into_owned())
        .replace('/', r"\")
        .trim_end_matches('\u{5c}')
        .to_lowercase()
}

fn blocker(detail: &'static str) -> UpdaterError {
    UpdaterError::new(UpdaterErrorCode::Blocker, detail)
}

#[cfg(windows)]
mod windows {
    use std::{os::windows::ffi::OsStringExt as _, ptr::null_mut};

    use windows_sys::Win32::{
        Foundation::{
            CloseHandle, ERROR_INVALID_PARAMETER, FILETIME, GetLastError, HANDLE, WAIT_OBJECT_0,
        },
        Storage::FileSystem::SYNCHRONIZE,
        System::Threading::{
            GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_TERMINATE,
            QueryFullProcessImageNameW, TerminateProcess, WaitForSingleObject,
        },
    };

    use super::*;

    pub(super) struct WindowsProcessControl;

    impl ProcessControl for WindowsProcessControl {
        fn inspect(
            &self,
            process: &restart_manager::BlockingProcess,
        ) -> Result<ProcessIdentity, UpdaterError> {
            let Some(handle) = open(process.pid, PROCESS_QUERY_LIMITED_INFORMATION | SYNCHRONIZE)?
            else {
                return Ok(ProcessIdentity::ExitedOrReused);
            };
            if unsafe { WaitForSingleObject(handle.raw(), 0) } == WAIT_OBJECT_0 {
                return Ok(ProcessIdentity::ExitedOrReused);
            }
            let start = process_start_time(handle.raw())?;
            if start != process.start_time_100ns {
                return Ok(ProcessIdentity::ExitedOrReused);
            }
            match process_path(handle.raw()) {
                Ok(path) => Ok(ProcessIdentity::Running {
                    executable: normalize_path(&path),
                }),
                Err(_) => Ok(ProcessIdentity::Uninspectable),
            }
        }

        fn terminate(
            &self,
            process: &restart_manager::BlockingProcess,
        ) -> Result<(), UpdaterError> {
            let Some(handle) = open(
                process.pid,
                PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_TERMINATE | SYNCHRONIZE,
            )?
            else {
                return Ok(());
            };
            if process_start_time(handle.raw())? != process.start_time_100ns {
                return Ok(());
            }
            if unsafe { TerminateProcess(handle.raw(), 1) } == 0 {
                return Err(blocker(
                    "failed to terminate same-installation AIHelper blocker",
                ));
            }
            Ok(())
        }
    }

    fn open(pid: u32, access: u32) -> Result<Option<OwnedHandle>, UpdaterError> {
        let handle = unsafe { OpenProcess(access, 0, pid) };
        if handle.is_null() {
            let code = unsafe { GetLastError() };
            if code == ERROR_INVALID_PARAMETER {
                Ok(None)
            } else {
                Ok(Some(OwnedHandle::uninspectable()))
            }
        } else {
            Ok(Some(OwnedHandle(handle)))
        }
    }

    fn process_start_time(handle: HANDLE) -> Result<u64, UpdaterError> {
        if handle.is_null() {
            return Err(blocker("blocking process identity cannot be inspected"));
        }
        let mut created = FILETIME::default();
        let mut exited = FILETIME::default();
        let mut kernel = FILETIME::default();
        let mut user = FILETIME::default();
        if unsafe { GetProcessTimes(handle, &mut created, &mut exited, &mut kernel, &mut user) }
            == 0
        {
            return Err(blocker("failed to verify blocking process identity"));
        }
        Ok(((created.dwHighDateTime as u64) << 32) | created.dwLowDateTime as u64)
    }

    fn process_path(handle: HANDLE) -> Result<PathBuf, UpdaterError> {
        if handle.is_null() {
            return Err(blocker("blocking process path cannot be inspected"));
        }
        let mut buffer = vec![0_u16; 32_768];
        let mut length = buffer.len() as u32;
        if unsafe { QueryFullProcessImageNameW(handle, 0, buffer.as_mut_ptr(), &mut length) } == 0 {
            return Err(blocker("failed to inspect blocking process path"));
        }
        buffer.truncate(length as usize);
        Ok(std::ffi::OsString::from_wide(&buffer).into())
    }

    struct OwnedHandle(HANDLE);

    impl OwnedHandle {
        fn uninspectable() -> Self {
            Self(null_mut())
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
}

#[cfg(test)]
mod tests {
    use std::{cell::RefCell, collections::BTreeMap, collections::VecDeque};

    use super::*;

    #[test]
    fn classifies_only_exact_path_and_start_identity_as_same_installation() {
        let same = process(1, 10);
        let foreign = process(2, 20);
        let stale = process(3, 30);
        let control = FakeControl::new([
            (
                1,
                ProcessIdentity::Running {
                    executable: r"c:\aihelper\ah.exe".to_owned(),
                },
            ),
            (
                2,
                ProcessIdentity::Running {
                    executable: r"c:\other\ah.exe".to_owned(),
                },
            ),
            (3, ProcessIdentity::ExitedOrReused),
        ]);
        let executables = BTreeSet::from([r"c:\aihelper\ah.exe".to_owned()]);

        let result = classify(
            &[same.clone(), foreign.clone(), stale],
            &executables,
            &control,
        )
        .unwrap();

        assert_eq!(result.same_installation, vec![same]);
        assert_eq!(result.foreign, vec![foreign]);
    }

    #[test]
    fn uninspectable_process_fails_closed_as_foreign() {
        let process = process(4, 40);
        let control = FakeControl::new([(4, ProcessIdentity::Uninspectable)]);
        let result = classify(&[process.clone()], &BTreeSet::new(), &control).unwrap();
        assert_eq!(result.foreign, vec![process]);
        assert_eq!(
            reject_foreign(&result).unwrap_err().code(),
            UpdaterErrorCode::Blocker
        );
    }

    #[test]
    fn grace_expiry_terminates_only_reverified_same_installation_process() {
        let same = process(5, 50);
        let control = FakeControl::new([(
            5,
            ProcessIdentity::Running {
                executable: r"c:\aihelper\ah.exe".to_owned(),
            },
        )]);
        let executables = BTreeSet::from([r"c:\aihelper\ah.exe".to_owned()]);
        let mut observations = VecDeque::from([
            restart_manager::BlockerObservation {
                processes: vec![same],
                reboot_reasons: 0,
            },
            restart_manager::BlockerObservation {
                processes: Vec::new(),
                reboot_reasons: 0,
            },
        ]);

        quiesce_with(&executables, Duration::ZERO, &control, || {
            Ok(observations.pop_front().unwrap())
        })
        .unwrap();

        assert_eq!(*control.terminated.borrow(), [5]);
    }

    fn process(pid: u32, start_time_100ns: u64) -> restart_manager::BlockingProcess {
        restart_manager::BlockingProcess {
            pid,
            start_time_100ns,
            application_name: format!("process-{pid}"),
            service_short_name: None,
            application_type: 0,
            app_status: 0,
            terminal_session_id: 0,
            restartable: false,
        }
    }

    struct FakeControl {
        identities: BTreeMap<u32, ProcessIdentity>,
        terminated: RefCell<Vec<u32>>,
    }

    impl FakeControl {
        fn new(values: impl IntoIterator<Item = (u32, ProcessIdentity)>) -> Self {
            Self {
                identities: values.into_iter().collect(),
                terminated: RefCell::new(Vec::new()),
            }
        }
    }

    impl ProcessControl for FakeControl {
        fn inspect(
            &self,
            process: &restart_manager::BlockingProcess,
        ) -> Result<ProcessIdentity, UpdaterError> {
            Ok(self.identities[&process.pid].clone())
        }

        fn terminate(
            &self,
            process: &restart_manager::BlockingProcess,
        ) -> Result<(), UpdaterError> {
            self.terminated.borrow_mut().push(process.pid);
            Ok(())
        }
    }
}
