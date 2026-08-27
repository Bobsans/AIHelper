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

use ah_error::AppError;

use super::{
    model::{DriftEntry, TaskMarker, hresult_hex, validate_uuid_json_fields},
    output::SchedulerState,
    paths::{account_sid, current_account},
    scheduler::{
        ObservedService, SchedulerDeleteReceipt, SchedulerInstance, SchedulerRunReceipt,
        SchedulerStopReceipt, SchedulerStopTarget, ServiceObservation, ServiceScheduler,
    },
    spec::{ServiceId, ServiceOwnership, ServiceSpec},
    windows_task::{
        MultipleInstancesPolicy, TASK_SOURCE, TaskSpec, has_canonical_restart_policy,
        semantic_drift, service_id,
    },
};

#[derive(Debug, Default, Clone, Copy)]
pub struct WindowsTaskScheduler;

impl ServiceScheduler for WindowsTaskScheduler {
    type Native = TaskSpec;

    fn identity(&self) -> Result<ServiceId, AppError> {
        Ok(service_id(current_account()?))
    }

    fn inspect(&self, id: &ServiceId) -> Result<ServiceObservation<TaskSpec>, AppError> {
        with_root_folder("inspect", |folder| {
            let name = id.path.trim_start_matches('\\');
            let registered = match unsafe { folder.GetTask(&BSTR::from(name)) } {
                Ok(task) => task,
                Err(error) if is_task_missing(error.code().0) => {
                    return Ok(ServiceObservation::Missing);
                }
                Err(error) => return Err(scheduler_error("get task", error)),
            };
            observe_registered(&registered)
        })
    }

    fn register(&self, desired: &ServiceSpec) -> Result<ObservedService<TaskSpec>, AppError> {
        let desired = &TaskSpec::from(desired);
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
                ServiceObservation::Owned(observed) => Ok(*observed),
                ServiceObservation::Missing => Err(AppError::external(
                    "MCP_SERVICE_SCHEDULER_FAILED",
                    "registered task disappeared during readback",
                )),
                ServiceObservation::Foreign => Err(AppError::external(
                    "MCP_SERVICE_TASK_CONFLICT",
                    "registered task does not retain AIHelper ownership markers",
                )),
            }
        })
    }

    fn run(&self, id: &ServiceId) -> Result<SchedulerRunReceipt, AppError> {
        with_root_folder("run", |folder| {
            let name = id.path.trim_start_matches('\\');
            let registered = unsafe { folder.GetTask(&BSTR::from(name)) }
                .map_err(|error| scheduler_error("get task for run", error))?;
            let empty = VARIANT::default();
            unsafe { registered.RunEx(&empty, 0, 0, &BSTR::new()) }
                .map_err(|error| scheduler_error("submit task run", error))?;
            Ok(SchedulerRunReceipt { submitted: true })
        })
    }

    fn instances(&self, expected: &ServiceOwnership) -> Result<Vec<SchedulerInstance>, AppError> {
        with_root_folder("enumerate instances", |folder| {
            let registered = get_task(folder, &expected.id.path, "get task for instances")?;
            require_owned_registration(&registered, expected)?;
            enumerate_instances(&registered)
        })
    }

    fn stop_instance(
        &self,
        expected: &ServiceSpec,
        target: &SchedulerStopTarget,
    ) -> Result<SchedulerStopReceipt, AppError> {
        let expected = &TaskSpec::from(expected);
        with_root_folder("stop instance", |folder| {
            let registered = get_task(folder, &expected.task_path, "get task for stop")?;
            require_safe_task(&registered, expected)?;
            let running = enumerate_running_tasks(&registered)?;
            let instances = running
                .iter()
                .map(|(_, instance)| instance.clone())
                .collect::<Vec<_>>();
            let index = match classify_stop_target(&instances, target) {
                StopTargetMatch::Exact(index) => index,
                StopTargetMatch::Missing => {
                    return Ok(SchedulerStopReceipt { stopped: false });
                }
                StopTargetMatch::Changed => {
                    return Err(AppError::external(
                        "MCP_SERVICE_TASK_CHANGED",
                        "Task Scheduler instance identity changed before stop",
                    ));
                }
            };
            match unsafe { running[index].0.Stop() } {
                Ok(()) => Ok(SchedulerStopReceipt { stopped: true }),
                Err(error) if is_task_instance_gone(error.code().0) => {
                    Ok(SchedulerStopReceipt { stopped: false })
                }
                Err(error) => Err(scheduler_error("stop task instance", error)),
            }
        })
    }

    fn delete_owned(
        &self,
        expected: &ServiceOwnership,
    ) -> Result<SchedulerDeleteReceipt, AppError> {
        with_root_folder("delete owned task", |folder| {
            let name = expected.id.path.trim_start_matches('\\');
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

    fn drift(
        &self,
        desired: &ServiceSpec,
        observed: &ObservedService<TaskSpec>,
    ) -> Vec<DriftEntry> {
        semantic_drift(&TaskSpec::from(desired), &observed.native)
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

/// The registration is still the one this installation wrote.
///
/// `observe_registered` has already refused anything whose registration source
/// is not AIHelper's or whose URI is not its own path, so what is left to prove
/// here is the path and the ownership marker.
fn require_owned_registration(
    registered: &IRegisteredTask,
    expected: &ServiceOwnership,
) -> Result<ObservedService<TaskSpec>, AppError> {
    let observation = observe_registered(registered)?;
    let ServiceObservation::Owned(observed) = observation else {
        return Err(AppError::external(
            "MCP_SERVICE_TASK_CHANGED",
            "Task Scheduler registration ownership changed before mutation",
        ));
    };
    if observed.id.path != expected.id.path || observed.marker != expected.marker {
        return Err(AppError::external(
            "MCP_SERVICE_TASK_CHANGED",
            "Task Scheduler ownership marker changed before mutation",
        ));
    }
    Ok(*observed)
}

fn require_safe_task(
    registered: &IRegisteredTask,
    expected: &TaskSpec,
) -> Result<ObservedService<TaskSpec>, AppError> {
    let observed = require_owned_registration(
        registered,
        &ServiceOwnership {
            id: ServiceId {
                owner: expected.user_sid.clone(),
                path: expected.task_path.clone(),
            },
            marker: expected.marker.clone(),
        },
    )?;
    let drift = semantic_drift(expected, &observed.native);
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
        let state = match unsafe { running.State() } {
            Ok(state) => state,
            Err(error) if is_task_instance_gone(error.code().0) => continue,
            Err(error) => return Err(scheduler_error("read task instance state", error)),
        };
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StopTargetMatch {
    Exact(usize),
    Missing,
    Changed,
}

fn classify_stop_target(
    instances: &[SchedulerInstance],
    target: &SchedulerStopTarget,
) -> StopTargetMatch {
    let expected_instance_id = match target {
        SchedulerStopTarget::Running { instance_id, .. }
        | SchedulerStopTarget::Queued { instance_id } => *instance_id,
    };
    let selected = instances
        .iter()
        .enumerate()
        .find(|(_, instance)| match target {
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
                    && !instances.iter().any(|candidate| {
                        candidate.state == SchedulerState::Running || candidate.engine_pid.is_some()
                    })
            }
        });
    if let Some((index, _)) = selected {
        StopTargetMatch::Exact(index)
    } else if instances
        .iter()
        .any(|instance| instance.instance_id == expected_instance_id)
    {
        StopTargetMatch::Changed
    } else {
        StopTargetMatch::Missing
    }
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

fn populate_definition(definition: &ITaskDefinition, desired: &TaskSpec) -> Result<(), AppError> {
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
        if !desired.restart_interval.is_empty() {
            settings
                .SetRestartInterval(&BSTR::from(desired.restart_interval.as_str()))
                .map_err(|error| scheduler_error("set RestartInterval", error))?;
        }
        settings
            .SetEnabled(VARIANT_BOOL::from(desired.enabled))
            .map_err(|error| scheduler_error("enable task definition", error))?;
        Ok(())
    }
}

fn observe_registered(
    registered: &IRegisteredTask,
) -> Result<ServiceObservation<TaskSpec>, AppError> {
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
        let task_path = registered
            .Path()
            .map_err(|error| scheduler_error("read task path", error))?
            .to_string();
        let marker_json = read_bstr(|value| definition.Data(value))?;
        let marker_value = serde_json::from_str::<serde_json::Value>(&marker_json).ok();
        let marker = marker_value
            .filter(|value| validate_uuid_json_fields(value).is_ok())
            .and_then(|value| serde_json::from_value::<TaskMarker>(value).ok())
            .filter(|marker| marker.validate().is_ok());
        let Some(marker) = marker else {
            return Ok(ServiceObservation::Foreign);
        };
        if source != TASK_SOURCE || uri != task_path {
            return Ok(ServiceObservation::Foreign);
        }

        let principal = definition
            .Principal()
            .map_err(|error| scheduler_error("read task principal", error))?;
        let user_sid = account_sid(&read_bstr(|value| principal.UserId(value))?)?;
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
                account_sid(&read_bstr(|value| logon.UserId(value))?)?
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

        let task_name = registered
            .Name()
            .map_err(|error| scheduler_error("read task name", error))?
            .to_string();
        let state = registered
            .State()
            .map_err(|error| scheduler_error("read task state", error))?;
        let last_result = registered.LastTaskResult().ok();
        let spec = TaskSpec {
            task_path: task_path.clone(),
            task_name,
            user_sid: user_sid.clone(),
            source,
            uri,
            marker: marker.clone(),
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
        Ok(ServiceObservation::Owned(Box::new(ObservedService {
            id: ServiceId {
                owner: user_sid,
                path: task_path,
            },
            marker,
            state: scheduler_state(state),
            last_result,
            last_run_at: None,
            canonical_restart_policy: has_canonical_restart_policy(&spec),
            native: spec,
        })))
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

fn is_task_instance_gone(value: i32) -> bool {
    value as u32 == 0x8004_130B
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
    use crate::windows_task::{MANAGED_RESTART_COUNT, MANAGED_RESTART_INTERVAL};
    use tempfile::TempDir;
    use uuid::Uuid;

    #[test]
    fn task_not_found_codes_are_classified_without_localized_messages() {
        assert!(is_task_missing(0x8007_0002u32 as i32));
        assert!(is_task_missing(0x8004_130Fu32 as i32));
        assert!(!is_task_missing(0x8007_0005u32 as i32));
    }

    #[test]
    fn task_instance_gone_codes_are_classified_without_hiding_other_failures() {
        assert!(is_task_instance_gone(0x8004_130Bu32 as i32));
        assert!(!is_task_instance_gone(0x8004_130Fu32 as i32));
        assert!(!is_task_instance_gone(0x8007_0005u32 as i32));
    }

    #[test]
    fn stop_target_distinguishes_exact_changed_and_disappeared_instances() {
        let expected = Uuid::new_v4();
        let target = SchedulerStopTarget::Running {
            instance_id: expected,
            expected_pid: 42,
        };
        let instance = |instance_id, state, engine_pid| SchedulerInstance {
            instance_id,
            state,
            engine_pid,
        };

        assert_eq!(
            classify_stop_target(
                &[instance(expected, SchedulerState::Running, Some(42))],
                &target,
            ),
            StopTargetMatch::Exact(0)
        );
        assert_eq!(
            classify_stop_target(&[instance(expected, SchedulerState::Queued, None)], &target,),
            StopTargetMatch::Changed
        );
        assert_eq!(
            classify_stop_target(
                &[instance(Uuid::new_v4(), SchedulerState::Running, Some(7))],
                &target,
            ),
            StopTargetMatch::Missing
        );
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
        let user_sid = current_account().unwrap();
        let marker = TaskMarker {
            schema_version: 1,
            owner: "AIHelper".to_owned(),
            kind: "managed_mcp".to_owned(),
            service_id: Uuid::new_v4(),
            configuration_id: Uuid::new_v4(),
            definition_path: temp.path().join("definition.json"),
        };
        let desired = TaskSpec::from(&ServiceSpec::managed_mcp(
            service_id(user_sid),
            marker,
            std::env::current_exe().unwrap(),
            temp.path().to_path_buf(),
        ));
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
            assert_eq!(restart_count, 0);
            assert!(restart_interval.is_empty());
            assert_eq!(trigger_count, 1);
            assert_eq!(action_count, 1);
            Ok(())
        });
        result.unwrap();
    }
}
