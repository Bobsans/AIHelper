use std::path::PathBuf;

use windows::{
    Win32::{
        Foundation::VARIANT_BOOL,
        System::{
            Com::{
                CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx,
                CoUninitialize,
            },
            TaskScheduler::{
                IAction, IExecAction, ILogonTrigger, IRegisteredTask, IRunningTask,
                ITaskDefinition, ITaskFolder, ITaskService, ITrigger, TASK_ACTION_EXEC,
                TASK_CREATE_OR_UPDATE, TASK_INSTANCES_IGNORE_NEW, TASK_INSTANCES_PARALLEL,
                TASK_INSTANCES_QUEUE, TASK_INSTANCES_STOP_EXISTING, TASK_LOGON_INTERACTIVE_TOKEN,
                TASK_RUNLEVEL_LUA, TASK_STATE_DISABLED, TASK_STATE_QUEUED, TASK_STATE_READY,
                TASK_STATE_RUNNING, TASK_TRIGGER_LOGON, TaskScheduler,
            },
            Variant::VARIANT,
        },
    },
    core::{BSTR, Interface},
};

use crate::error::AppError;

use super::{
    model::{TaskMarker, hresult_hex, validate_uuid_json_fields},
    output::SchedulerState,
    scheduler::{
        DesiredTaskSpec, ExpectedTaskOwnership, MultipleInstancesPolicy, ObservedTask,
        SchedulerAdapter, SchedulerDeleteReceipt, SchedulerInstance, SchedulerRunReceipt,
        SchedulerStopReceipt, SchedulerStopTarget, TASK_SOURCE, TaskObservation, semantic_drift,
    },
};

#[derive(Debug, Default, Clone, Copy)]
pub struct WindowsTaskScheduler;

impl SchedulerAdapter for WindowsTaskScheduler {
    fn inspect(&self, task_path: &str) -> Result<TaskObservation, AppError> {
        with_root_folder("inspect", |folder| {
            let name = task_path.trim_start_matches('\\');
            let registered = match unsafe { folder.GetTask(&BSTR::from(name)) } {
                Ok(task) => task,
                Err(error) if is_task_missing(error.code().0) => {
                    return Ok(TaskObservation::Missing);
                }
                Err(error) => return Err(scheduler_error("get task", error)),
            };
            observe_registered(&registered)
        })
    }

    fn register(&self, desired: &DesiredTaskSpec) -> Result<ObservedTask, AppError> {
        with_root_folder("register", |folder| {
            let service = connected_service()?;
            let definition = unsafe { service.NewTask(0) }
                .map_err(|error| scheduler_error("create task definition", error))?;
            populate_definition(&definition, desired)?;
            let empty = VARIANT::default();
            let user = VARIANT::from(desired.user_sid.as_str());
            let registered = unsafe {
                folder.RegisterTaskDefinition(
                    &BSTR::from(desired.task_name.as_str()),
                    &definition,
                    TASK_CREATE_OR_UPDATE.0,
                    &user,
                    &empty,
                    TASK_LOGON_INTERACTIVE_TOKEN,
                    &empty,
                )
            }
            .map_err(|error| scheduler_error("register task definition", error))?;
            match observe_registered(&registered)? {
                TaskObservation::Owned(observed) => Ok(observed),
                TaskObservation::Missing => Err(AppError::external(
                    "MCP_SERVICE_SCHEDULER_FAILED",
                    "registered task disappeared during readback",
                )),
                TaskObservation::Foreign { .. } => Err(AppError::external(
                    "MCP_SERVICE_TASK_CONFLICT",
                    "registered task does not retain AIHelper ownership markers",
                )),
            }
        })
    }

    fn run(&self, task_path: &str) -> Result<SchedulerRunReceipt, AppError> {
        with_root_folder("run", |folder| {
            let name = task_path.trim_start_matches('\\');
            let registered = unsafe { folder.GetTask(&BSTR::from(name)) }
                .map_err(|error| scheduler_error("get task for run", error))?;
            let empty = VARIANT::default();
            unsafe { registered.RunEx(&empty, 0, 0, &BSTR::new()) }
                .map_err(|error| scheduler_error("submit task run", error))?;
            Ok(SchedulerRunReceipt { submitted: true })
        })
    }

    fn instances(&self, expected: &DesiredTaskSpec) -> Result<Vec<SchedulerInstance>, AppError> {
        with_root_folder("enumerate instances", |folder| {
            let registered = get_task(folder, &expected.task_path, "get task for instances")?;
            require_owned_registration(&registered, &ExpectedTaskOwnership::from(expected))?;
            enumerate_instances(&registered)
        })
    }

    fn stop_instance(
        &self,
        expected: &DesiredTaskSpec,
        target: &SchedulerStopTarget,
    ) -> Result<SchedulerStopReceipt, AppError> {
        with_root_folder("stop instance", |folder| {
            let registered = get_task(folder, &expected.task_path, "get task for stop")?;
            require_safe_task(&registered, expected)?;
            let running = enumerate_running_tasks(&registered)?;
            let selected = select_stop_target(&running, target)?;
            unsafe { selected.Stop() }
                .map_err(|error| scheduler_error("stop task instance", error))?;
            Ok(SchedulerStopReceipt { stopped: true })
        })
    }

    fn delete_owned(
        &self,
        expected: &ExpectedTaskOwnership,
    ) -> Result<SchedulerDeleteReceipt, AppError> {
        with_root_folder("delete owned task", |folder| {
            let name = expected.task_path.trim_start_matches('\\');
            let registered = match unsafe { folder.GetTask(&BSTR::from(name)) } {
                Ok(task) => task,
                Err(error) if is_task_missing(error.code().0) => {
                    return Ok(SchedulerDeleteReceipt { deleted: false });
                }
                Err(error) => return Err(scheduler_error("get task for delete", error)),
            };
            require_owned_registration(&registered, expected)?;
            unsafe { folder.DeleteTask(&BSTR::from(name), 0) }
                .map_err(|error| scheduler_error("delete owned task", error))?;
            match unsafe { folder.GetTask(&BSTR::from(name)) } {
                Err(error) if is_task_missing(error.code().0) => {
                    Ok(SchedulerDeleteReceipt { deleted: true })
                }
                Ok(_) => Err(AppError::external(
                    "MCP_SERVICE_UNINSTALL_INCOMPLETE",
                    "deleted task is still present after Task Scheduler readback",
                )),
                Err(error) => Err(AppError::external(
                    "MCP_SERVICE_UNINSTALL_INCOMPLETE",
                    scheduler_error("confirm task deletion", error).detail_message(),
                )),
            }
        })
    }
}

fn get_task(
    folder: &ITaskFolder,
    task_path: &str,
    operation: &str,
) -> Result<IRegisteredTask, AppError> {
    let name = task_path.trim_start_matches('\\');
    unsafe { folder.GetTask(&BSTR::from(name)) }.map_err(|error| scheduler_error(operation, error))
}

fn require_owned_registration(
    registered: &IRegisteredTask,
    expected: &ExpectedTaskOwnership,
) -> Result<ObservedTask, AppError> {
    let observation = observe_registered(registered)?;
    let TaskObservation::Owned(observed) = observation else {
        return Err(AppError::external(
            "MCP_SERVICE_TASK_CHANGED",
            "Task Scheduler registration ownership changed before mutation",
        ));
    };
    if observed.spec.task_path != expected.task_path
        || observed.spec.source != expected.source
        || observed.spec.uri != expected.uri
        || observed.spec.marker != expected.marker
    {
        return Err(AppError::external(
            "MCP_SERVICE_TASK_CHANGED",
            "Task Scheduler ownership marker changed before mutation",
        ));
    }
    Ok(observed)
}

fn require_safe_task(
    registered: &IRegisteredTask,
    expected: &DesiredTaskSpec,
) -> Result<ObservedTask, AppError> {
    let observed = require_owned_registration(registered, &ExpectedTaskOwnership::from(expected))?;
    let drift = semantic_drift(expected, &observed.spec);
    if !drift.is_empty() {
        return Err(AppError::external(
            "MCP_SERVICE_TASK_CHANGED",
            format!(
                "Task Scheduler safe execution properties changed before mutation ({} fields)",
                drift.len()
            ),
        ));
    }
    Ok(observed)
}

fn enumerate_instances(registered: &IRegisteredTask) -> Result<Vec<SchedulerInstance>, AppError> {
    Ok(enumerate_running_tasks(registered)?
        .into_iter()
        .map(|(_, instance)| instance)
        .collect())
}

fn enumerate_running_tasks(
    registered: &IRegisteredTask,
) -> Result<Vec<(IRunningTask, SchedulerInstance)>, AppError> {
    let collection = unsafe { registered.GetInstances(0) }
        .map_err(|error| scheduler_error("enumerate task instances", error))?;
    let count = unsafe { collection.Count() }
        .map_err(|error| scheduler_error("read task instance count", error))?;
    let mut instances = Vec::with_capacity(count.max(0) as usize);
    for index in 1..=count {
        let key = VARIANT::from(index);
        let running = unsafe { collection.get_Item(&key) }
            .map_err(|error| scheduler_error("read task instance", error))?;
        let instance_id = unsafe { running.InstanceGuid() }
            .map_err(|error| scheduler_error("read task instance ID", error))?
            .to_string();
        let instance_id = parse_scheduler_instance_id(&instance_id)?;
        let state = unsafe { running.State() }
            .map_err(|error| scheduler_error("read task instance state", error))?;
        let engine_pid = unsafe { running.EnginePID() }.ok().filter(|pid| *pid != 0);
        instances.push((
            running,
            SchedulerInstance {
                instance_id,
                state: scheduler_state(state),
                engine_pid,
            },
        ));
    }
    Ok(instances)
}

fn parse_scheduler_instance_id(value: &str) -> Result<uuid::Uuid, AppError> {
    uuid::Uuid::parse_str(value.trim_matches(&['{', '}'][..])).map_err(|error| {
        AppError::external(
            "MCP_SERVICE_SCHEDULER_FAILED",
            format!("Task Scheduler returned an invalid instance ID: {error}"),
        )
    })
}

fn select_stop_target(
    instances: &[(IRunningTask, SchedulerInstance)],
    target: &SchedulerStopTarget,
) -> Result<IRunningTask, AppError> {
    let selected = instances.iter().find(|(_, instance)| match target {
        SchedulerStopTarget::Running {
            instance_id,
            expected_pid,
        } => {
            instance.instance_id == *instance_id
                && instance.state == SchedulerState::Running
                && instance.engine_pid == Some(*expected_pid)
        }
        SchedulerStopTarget::Queued { instance_id } => {
            instance.instance_id == *instance_id
                && instance.state == SchedulerState::Queued
                && instance.engine_pid.is_none()
                && !instances.iter().any(|(_, candidate)| {
                    candidate.state == SchedulerState::Running || candidate.engine_pid.is_some()
                })
        }
    });
    selected.map(|(running, _)| running.clone()).ok_or_else(|| {
        AppError::external(
            "MCP_SERVICE_TASK_CHANGED",
            "Task Scheduler instance identity changed before stop",
        )
    })
}

fn with_root_folder<T>(
    operation: &str,
    callback: impl FnOnce(&ITaskFolder) -> Result<T, AppError>,
) -> Result<T, AppError> {
    with_com_session(operation, || {
        let service = connected_service()?;
        let folder = unsafe { service.GetFolder(&BSTR::from("\\")) }
            .map_err(|error| scheduler_error("open Task Scheduler root folder", error))?;
        let result = callback(&folder);
        drop(folder);
        drop(service);
        result
    })
}

fn with_com_session<T>(
    operation: &str,
    callback: impl FnOnce() -> Result<T, AppError>,
) -> Result<T, AppError> {
    // SAFETY: lifecycle commands execute synchronously on their calling thread.
    // COM is initialized and uninitialized on that same thread, and all COM
    // interface values are dropped before CoUninitialize.
    unsafe {
        CoInitializeEx(None, COINIT_MULTITHREADED)
            .ok()
            .map_err(|error| scheduler_error(&format!("initialize COM for {operation}"), error))?;
        let result = callback();
        CoUninitialize();
        result
    }
}

fn connected_service() -> Result<ITaskService, AppError> {
    // SAFETY: COM is initialized by with_root_folder on the current thread.
    unsafe {
        let service: ITaskService = CoCreateInstance(&TaskScheduler, None, CLSCTX_INPROC_SERVER)
            .map_err(|error| scheduler_error("create Task Scheduler service", error))?;
        let empty = VARIANT::default();
        service
            .Connect(&empty, &empty, &empty, &empty)
            .map_err(|error| scheduler_error("connect Task Scheduler service", error))?;
        Ok(service)
    }
}

fn populate_definition(
    definition: &ITaskDefinition,
    desired: &DesiredTaskSpec,
) -> Result<(), AppError> {
    // SAFETY: every interface originates from the typed Task Scheduler object
    // model and all BSTR/VARIANT values remain alive for each setter call.
    unsafe {
        let registration = definition
            .RegistrationInfo()
            .map_err(|error| scheduler_error("get registration info", error))?;
        registration
            .SetSource(&BSTR::from(desired.source.as_str()))
            .map_err(|error| scheduler_error("set registration source", error))?;
        registration
            .SetURI(&BSTR::from(desired.uri.as_str()))
            .map_err(|error| scheduler_error("set registration URI", error))?;
        let marker = serde_json::to_string(&desired.marker)?;
        definition
            .SetData(&BSTR::from(marker.as_str()))
            .map_err(|error| scheduler_error("set task marker", error))?;

        let principal = definition
            .Principal()
            .map_err(|error| scheduler_error("get task principal", error))?;
        principal
            .SetUserId(&BSTR::from(desired.user_sid.as_str()))
            .map_err(|error| scheduler_error("set principal user", error))?;
        principal
            .SetLogonType(TASK_LOGON_INTERACTIVE_TOKEN)
            .map_err(|error| scheduler_error("set principal logon type", error))?;
        principal
            .SetRunLevel(TASK_RUNLEVEL_LUA)
            .map_err(|error| scheduler_error("set principal run level", error))?;

        let triggers = definition
            .Triggers()
            .map_err(|error| scheduler_error("get trigger collection", error))?;
        triggers
            .Clear()
            .map_err(|error| scheduler_error("clear trigger collection", error))?;
        let trigger: ILogonTrigger = triggers
            .Create(TASK_TRIGGER_LOGON)
            .and_then(|trigger| trigger.cast())
            .map_err(|error| scheduler_error("create logon trigger", error))?;
        trigger
            .SetUserId(&BSTR::from(desired.trigger_user_sid.as_str()))
            .map_err(|error| scheduler_error("set logon trigger user", error))?;
        trigger
            .SetEnabled(VARIANT_BOOL::from(desired.trigger_enabled))
            .map_err(|error| scheduler_error("enable logon trigger", error))?;

        let actions = definition
            .Actions()
            .map_err(|error| scheduler_error("get action collection", error))?;
        actions
            .Clear()
            .map_err(|error| scheduler_error("clear action collection", error))?;
        let action: IExecAction = actions
            .Create(TASK_ACTION_EXEC)
            .and_then(|action| action.cast())
            .map_err(|error| scheduler_error("create exec action", error))?;
        action
            .SetPath(&BSTR::from(
                desired.executable_path.to_string_lossy().as_ref(),
            ))
            .map_err(|error| scheduler_error("set action path", error))?;
        action
            .SetArguments(&BSTR::from(desired.arguments.as_str()))
            .map_err(|error| scheduler_error("set action arguments", error))?;
        action
            .SetWorkingDirectory(&BSTR::from(
                desired.working_directory.to_string_lossy().as_ref(),
            ))
            .map_err(|error| scheduler_error("set action working directory", error))?;

        let settings = definition
            .Settings()
            .map_err(|error| scheduler_error("get task settings", error))?;
        settings
            .SetAllowDemandStart(VARIANT_BOOL::from(desired.allow_demand_start))
            .map_err(|error| scheduler_error("set AllowDemandStart", error))?;
        settings
            .SetStartWhenAvailable(VARIANT_BOOL::from(desired.start_when_available))
            .map_err(|error| scheduler_error("set StartWhenAvailable", error))?;
        settings
            .SetMultipleInstances(TASK_INSTANCES_IGNORE_NEW)
            .map_err(|error| scheduler_error("set MultipleInstances", error))?;
        settings
            .SetExecutionTimeLimit(&BSTR::from(desired.execution_time_limit.as_str()))
            .map_err(|error| scheduler_error("set ExecutionTimeLimit", error))?;
        settings
            .SetDisallowStartIfOnBatteries(VARIANT_BOOL::from(desired.disallow_start_on_batteries))
            .map_err(|error| scheduler_error("set DisallowStartIfOnBatteries", error))?;
        settings
            .SetStopIfGoingOnBatteries(VARIANT_BOOL::from(desired.stop_if_going_on_batteries))
            .map_err(|error| scheduler_error("set StopIfGoingOnBatteries", error))?;
        settings
            .SetRunOnlyIfIdle(VARIANT_BOOL::from(desired.run_only_if_idle))
            .map_err(|error| scheduler_error("set RunOnlyIfIdle", error))?;
        settings
            .SetRunOnlyIfNetworkAvailable(VARIANT_BOOL::from(desired.run_only_if_network_available))
            .map_err(|error| scheduler_error("set RunOnlyIfNetworkAvailable", error))?;
        settings
            .SetRestartCount(desired.restart_count)
            .map_err(|error| scheduler_error("set RestartCount", error))?;
        settings
            .SetRestartInterval(&BSTR::from(desired.restart_interval.as_str()))
            .map_err(|error| scheduler_error("set RestartInterval", error))?;
        settings
            .SetEnabled(VARIANT_BOOL::from(desired.enabled))
            .map_err(|error| scheduler_error("enable task definition", error))?;
        Ok(())
    }
}

fn observe_registered(registered: &IRegisteredTask) -> Result<TaskObservation, AppError> {
    // SAFETY: getters populate initialized typed out parameters owned by this
    // stack frame. Casts are restricted to the advertised action/trigger type.
    unsafe {
        let definition = registered
            .Definition()
            .map_err(|error| scheduler_error("read registered task definition", error))?;
        let registration = definition
            .RegistrationInfo()
            .map_err(|error| scheduler_error("read registration info", error))?;
        let source = read_bstr(|value| registration.Source(value))?;
        let uri = read_bstr(|value| registration.URI(value))?;
        let marker_json = read_bstr(|value| definition.Data(value))?;
        let marker_value = serde_json::from_str::<serde_json::Value>(&marker_json).ok();
        let marker = marker_value
            .filter(|value| validate_uuid_json_fields(value).is_ok())
            .and_then(|value| serde_json::from_value::<TaskMarker>(value).ok())
            .filter(|marker| marker.validate().is_ok());
        let Some(marker) = marker else {
            return Ok(TaskObservation::Foreign {
                source: Some(source),
                uri: Some(uri),
            });
        };
        let expected_uri = format!("urn:aihelper:managed-mcp:v1:{}", marker.service_id);
        if source != TASK_SOURCE || uri != expected_uri {
            return Ok(TaskObservation::Foreign {
                source: Some(source),
                uri: Some(uri),
            });
        }

        let principal = definition
            .Principal()
            .map_err(|error| scheduler_error("read task principal", error))?;
        let user_sid = read_bstr(|value| principal.UserId(value))?;
        let mut logon_type = Default::default();
        principal
            .LogonType(&mut logon_type)
            .map_err(|error| scheduler_error("read principal logon type", error))?;
        let mut run_level = Default::default();
        principal
            .RunLevel(&mut run_level)
            .map_err(|error| scheduler_error("read principal run level", error))?;

        let triggers = definition
            .Triggers()
            .map_err(|error| scheduler_error("read trigger collection", error))?;
        let mut trigger_count = 0;
        triggers
            .Count(&mut trigger_count)
            .map_err(|error| scheduler_error("read trigger count", error))?;
        let (trigger_type, trigger_user_sid, trigger_enabled) = if trigger_count == 1 {
            let base: ITrigger = triggers
                .get_Item(1)
                .map_err(|error| scheduler_error("read logon trigger", error))?;
            let mut kind = Default::default();
            base.Type(&mut kind)
                .map_err(|error| scheduler_error("read trigger type", error))?;
            let mut enabled = VARIANT_BOOL::default();
            base.Enabled(&mut enabled)
                .map_err(|error| scheduler_error("read trigger enabled", error))?;
            let user = if kind == TASK_TRIGGER_LOGON {
                let logon: ILogonTrigger = base
                    .cast()
                    .map_err(|error| scheduler_error("cast logon trigger", error))?;
                read_bstr(|value| logon.UserId(value))?
            } else {
                String::new()
            };
            (
                if kind == TASK_TRIGGER_LOGON {
                    "logon"
                } else {
                    "other"
                }
                .to_owned(),
                user,
                enabled.0 != 0,
            )
        } else {
            ("other".to_owned(), String::new(), false)
        };

        let actions = definition
            .Actions()
            .map_err(|error| scheduler_error("read action collection", error))?;
        let mut action_count = 0;
        actions
            .Count(&mut action_count)
            .map_err(|error| scheduler_error("read action count", error))?;
        let (action_type, executable_path, arguments, working_directory) = if action_count == 1 {
            let base: IAction = actions
                .get_Item(1)
                .map_err(|error| scheduler_error("read exec action", error))?;
            let mut kind = Default::default();
            base.Type(&mut kind)
                .map_err(|error| scheduler_error("read action type", error))?;
            if kind == TASK_ACTION_EXEC {
                let exec: IExecAction = base
                    .cast()
                    .map_err(|error| scheduler_error("cast exec action", error))?;
                (
                    "exec".to_owned(),
                    PathBuf::from(read_bstr(|value| exec.Path(value))?),
                    read_bstr(|value| exec.Arguments(value))?,
                    PathBuf::from(read_bstr(|value| exec.WorkingDirectory(value))?),
                )
            } else {
                (
                    "other".to_owned(),
                    PathBuf::new(),
                    String::new(),
                    PathBuf::new(),
                )
            }
        } else {
            (
                "other".to_owned(),
                PathBuf::new(),
                String::new(),
                PathBuf::new(),
            )
        };

        let settings = definition
            .Settings()
            .map_err(|error| scheduler_error("read task settings", error))?;
        let allow_demand_start = read_bool(|value| settings.AllowDemandStart(value))?;
        let start_when_available = read_bool(|value| settings.StartWhenAvailable(value))?;
        let mut multiple_instances = Default::default();
        settings
            .MultipleInstances(&mut multiple_instances)
            .map_err(|error| scheduler_error("read MultipleInstances", error))?;
        let execution_time_limit = read_bstr(|value| settings.ExecutionTimeLimit(value))?;
        let disallow_start_on_batteries =
            read_bool(|value| settings.DisallowStartIfOnBatteries(value))?;
        let stop_if_going_on_batteries = read_bool(|value| settings.StopIfGoingOnBatteries(value))?;
        let run_only_if_idle = read_bool(|value| settings.RunOnlyIfIdle(value))?;
        let run_only_if_network_available =
            read_bool(|value| settings.RunOnlyIfNetworkAvailable(value))?;
        let mut restart_count = 0;
        settings
            .RestartCount(&mut restart_count)
            .map_err(|error| scheduler_error("read RestartCount", error))?;
        let restart_interval = read_bstr(|value| settings.RestartInterval(value))?;
        let enabled = read_bool(|value| settings.Enabled(value))?;

        let task_path = registered
            .Path()
            .map_err(|error| scheduler_error("read task path", error))?
            .to_string();
        let task_name = registered
            .Name()
            .map_err(|error| scheduler_error("read task name", error))?
            .to_string();
        let state = registered
            .State()
            .map_err(|error| scheduler_error("read task state", error))?;
        let last_result = registered.LastTaskResult().ok();
        let spec = DesiredTaskSpec {
            task_path,
            task_name,
            user_sid,
            source,
            uri,
            marker,
            executable_path,
            arguments,
            working_directory,
            principal_logon_type: if logon_type == TASK_LOGON_INTERACTIVE_TOKEN {
                "interactive_token"
            } else {
                "other"
            }
            .to_owned(),
            principal_run_level: if run_level == TASK_RUNLEVEL_LUA {
                "lua"
            } else {
                "other"
            }
            .to_owned(),
            trigger_count,
            trigger_type,
            trigger_user_sid,
            trigger_enabled,
            action_count,
            action_type,
            allow_demand_start,
            start_when_available,
            multiple_instances: if multiple_instances == TASK_INSTANCES_IGNORE_NEW {
                MultipleInstancesPolicy::IgnoreNew
            } else if multiple_instances == TASK_INSTANCES_PARALLEL {
                MultipleInstancesPolicy::Parallel
            } else if multiple_instances == TASK_INSTANCES_QUEUE {
                MultipleInstancesPolicy::Queue
            } else if multiple_instances == TASK_INSTANCES_STOP_EXISTING {
                MultipleInstancesPolicy::StopExisting
            } else {
                MultipleInstancesPolicy::Unknown
            },
            execution_time_limit,
            disallow_start_on_batteries,
            stop_if_going_on_batteries,
            run_only_if_idle,
            run_only_if_network_available,
            restart_count,
            restart_interval,
            enabled,
        };
        Ok(TaskObservation::Owned(ObservedTask {
            spec,
            scheduler_state: scheduler_state(state),
            last_result,
            last_run_at: None,
        }))
    }
}

fn scheduler_state(state: windows::Win32::System::TaskScheduler::TASK_STATE) -> SchedulerState {
    if state == TASK_STATE_DISABLED {
        SchedulerState::Disabled
    } else if state == TASK_STATE_QUEUED {
        SchedulerState::Queued
    } else if state == TASK_STATE_READY {
        SchedulerState::Ready
    } else if state == TASK_STATE_RUNNING {
        SchedulerState::Running
    } else {
        SchedulerState::Unknown
    }
}

fn read_bstr(
    callback: impl FnOnce(*mut BSTR) -> windows::core::Result<()>,
) -> Result<String, AppError> {
    let mut value = BSTR::new();
    callback(&mut value).map_err(|error| scheduler_error("read Task Scheduler string", error))?;
    Ok(value.to_string())
}

fn read_bool(
    callback: impl FnOnce(*mut VARIANT_BOOL) -> windows::core::Result<()>,
) -> Result<bool, AppError> {
    let mut value = VARIANT_BOOL::default();
    callback(&mut value).map_err(|error| scheduler_error("read Task Scheduler boolean", error))?;
    Ok(value.0 != 0)
}

fn is_task_missing(value: i32) -> bool {
    matches!(value as u32, 0x8007_0002 | 0x8007_0003 | 0x8004_130F)
}

fn scheduler_error(operation: &str, error: windows::core::Error) -> AppError {
    let value = error.code().0;
    AppError::external(
        "MCP_SERVICE_SCHEDULER_FAILED",
        format!(
            "Task Scheduler operation '{operation}' failed (hresult={} {})",
            value,
            hresult_hex(value)
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp_service::{
        paths::{current_user_sid, task_path},
        scheduler::{MANAGED_RESTART_COUNT, MANAGED_RESTART_INTERVAL},
    };
    use tempfile::TempDir;
    use uuid::Uuid;

    #[test]
    fn task_not_found_codes_are_classified_without_localized_messages() {
        assert!(is_task_missing(0x8007_0002u32 as i32));
        assert!(is_task_missing(0x8004_130Fu32 as i32));
        assert!(!is_task_missing(0x8007_0005u32 as i32));
    }

    #[test]
    fn scheduler_instance_guids_and_states_are_normalized_deterministically() {
        let expected = Uuid::parse_str("550e8400-e29b-41d4-a716-446655440000").unwrap();
        assert_eq!(
            parse_scheduler_instance_id("{550E8400-E29B-41D4-A716-446655440000}").unwrap(),
            expected
        );
        assert_eq!(scheduler_state(TASK_STATE_QUEUED), SchedulerState::Queued);
        assert_eq!(scheduler_state(TASK_STATE_RUNNING), SchedulerState::Running);
    }

    #[test]
    fn task_service_can_create_typed_definition_without_registration() {
        let temp = TempDir::new().unwrap();
        let user_sid = current_user_sid().unwrap();
        let marker = TaskMarker {
            schema_version: 1,
            owner: "AIHelper".to_owned(),
            kind: "managed_mcp".to_owned(),
            service_id: Uuid::new_v4(),
            configuration_id: Uuid::new_v4(),
            definition_path: temp.path().join("definition.json"),
        };
        let desired = DesiredTaskSpec::canonical(
            task_path(&user_sid),
            user_sid,
            marker,
            std::env::current_exe().unwrap(),
            temp.path().to_path_buf(),
        );
        let result = with_com_session("smoke test", || {
            let service = connected_service()?;
            let definition = unsafe { service.NewTask(0) }
                .map_err(|error| scheduler_error("create smoke definition", error))?;
            populate_definition(&definition, &desired)?;
            let settings = unsafe { definition.Settings() }
                .map_err(|error| scheduler_error("read smoke settings", error))?;
            let mut restart_count = 0;
            unsafe { settings.RestartCount(&mut restart_count) }
                .map_err(|error| scheduler_error("read smoke restart count", error))?;
            let restart_interval = read_bstr(|value| unsafe { settings.RestartInterval(value) })?;
            let triggers = unsafe { definition.Triggers() }
                .map_err(|error| scheduler_error("read smoke triggers", error))?;
            let mut trigger_count = 0;
            unsafe { triggers.Count(&mut trigger_count) }
                .map_err(|error| scheduler_error("read smoke trigger count", error))?;
            let actions = unsafe { definition.Actions() }
                .map_err(|error| scheduler_error("read smoke actions", error))?;
            let mut action_count = 0;
            unsafe { actions.Count(&mut action_count) }
                .map_err(|error| scheduler_error("read smoke action count", error))?;
            assert_eq!(restart_count, MANAGED_RESTART_COUNT);
            assert_eq!(restart_interval, MANAGED_RESTART_INTERVAL);
            assert_eq!(trigger_count, 1);
            assert_eq!(action_count, 1);
            Ok(())
        });
        result.unwrap();
    }
}
