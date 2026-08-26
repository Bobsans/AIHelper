use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use ah_plugin_api::{CommandError, TypedInvocationResponse};
use ah_runtime::{
    RuntimeError,
    executor::{ExecutionHandle, ExecutionLifecycle, Executor},
};

use crate::{
    events::EventDispatcher,
    mapping::run_check_outcome,
    server::{McpCommandEvent, McpCommandStatus},
};

pub(crate) const DEFAULT_JOB_CAPACITY: usize = 128;
pub(crate) const DEFAULT_JOB_TTL: Duration = Duration::from_secs(60 * 60);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum JobStatus {
    Running,
    Succeeded,
    Failed,
    Cancelled,
    TimedOut,
}

impl JobStatus {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::TimedOut => "timed_out",
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct JobSnapshot {
    pub(crate) job_id: String,
    pub(crate) tool: String,
    pub(crate) status: JobStatus,
    pub(crate) draining: bool,
    pub(crate) response: Option<TypedInvocationResponse>,
}

#[derive(Debug)]
pub(crate) enum JobRegistryError {
    CapacityFull { capacity: usize },
    NotFound { job_id: String },
}

pub(crate) struct JobReservation {
    job_id: String,
}

pub(crate) struct JobEventContext {
    pub(crate) dispatcher: Arc<EventDispatcher>,
    pub(crate) command: String,
    pub(crate) tool: String,
    pub(crate) request_id: String,
    pub(crate) parameters: serde_json::Value,
    pub(crate) started: Instant,
}

pub(crate) type JobCompletionHook = Box<dyn FnOnce() + Send + 'static>;

pub(crate) struct JobRegistry {
    records: Mutex<HashMap<String, JobRecord>>,
    max_records: usize,
    ttl: Duration,
    boot_nonce: u64,
    next_sequence: AtomicU64,
}

struct JobRecord {
    sequence: u64,
    tool: String,
    execution_id: String,
    status: JobStatus,
    draining: bool,
    response: Option<TypedInvocationResponse>,
    physical_completed_at: Option<Instant>,
    lifecycle: Option<ExecutionLifecycle>,
}

impl JobRegistry {
    pub(crate) fn standard() -> Arc<Self> {
        Arc::new(Self::new(DEFAULT_JOB_CAPACITY, DEFAULT_JOB_TTL))
    }

    fn new(max_records: usize, ttl: Duration) -> Self {
        Self {
            records: Mutex::new(HashMap::new()),
            max_records,
            ttl,
            boot_nonce: boot_nonce(),
            next_sequence: AtomicU64::new(1),
        }
    }

    pub(crate) fn reserve(
        &self,
        tool: String,
        execution_id: String,
    ) -> Result<JobReservation, JobRegistryError> {
        let sequence = self.next_sequence.fetch_add(1, Ordering::Relaxed);
        let job_id = format!("job-{:016x}-{sequence:016x}", self.boot_nonce);
        let mut records = lock_records(&self.records);
        self.cleanup_locked(&mut records, Instant::now());
        while records.len() >= self.max_records {
            let oldest = oldest_completed(&records);
            let Some(job_id) = oldest else {
                break;
            };
            records.remove(&job_id);
        }
        if records.len() >= self.max_records {
            return Err(JobRegistryError::CapacityFull {
                capacity: self.max_records,
            });
        }
        records.insert(
            job_id.clone(),
            JobRecord {
                sequence,
                tool,
                execution_id,
                status: JobStatus::Running,
                draining: false,
                response: None,
                physical_completed_at: None,
                lifecycle: None,
            },
        );
        Ok(JobReservation { job_id })
    }

    pub(crate) fn rollback(&self, reservation: JobReservation) {
        lock_records(&self.records).remove(&reservation.job_id);
    }

    pub(crate) fn attach(
        self: &Arc<Self>,
        reservation: JobReservation,
        handle: ExecutionHandle,
        map_runtime_error: fn(RuntimeError) -> CommandError,
        event_context: Option<JobEventContext>,
        completion_hook: Option<JobCompletionHook>,
    ) -> JobSnapshot {
        let job_id = reservation.job_id;
        let lifecycle = handle.lifecycle();
        if let Some(record) = lock_records(&self.records).get_mut(&job_id) {
            record.lifecycle = Some(lifecycle.clone());
        }
        let snapshot = self
            .snapshot(&job_id)
            .expect("reserved job must exist before attachment");
        let registry = Arc::clone(self);
        tokio::spawn(async move {
            let observed = handle.observe().await;
            let telemetry = observed.telemetry;
            let (status, response) = match observed.result {
                Ok(response) if response.success => (JobStatus::Succeeded, response),
                Ok(response) => (JobStatus::Failed, response),
                Err(RuntimeError::ExecutionCancelled { .. }) => (
                    JobStatus::Cancelled,
                    TypedInvocationResponse::error(map_runtime_error(
                        RuntimeError::ExecutionCancelled {
                            request_id: lifecycle.request_id().to_owned(),
                        },
                    )),
                ),
                Err(RuntimeError::ExecutionTimeout { .. }) => (
                    JobStatus::TimedOut,
                    TypedInvocationResponse::error(map_runtime_error(
                        RuntimeError::ExecutionTimeout {
                            request_id: lifecycle.request_id().to_owned(),
                        },
                    )),
                ),
                Err(error) => (
                    JobStatus::Failed,
                    TypedInvocationResponse::error(map_runtime_error(error)),
                ),
            };
            let draining = matches!(status, JobStatus::Cancelled | JobStatus::TimedOut)
                && lifecycle.is_draining();
            let event_delivery = event_context.map(|event| {
                let event_status = if status == JobStatus::Succeeded {
                    McpCommandStatus::Success
                } else {
                    McpCommandStatus::Error
                };
                let diagnostic = response.error.clone();
                let outcome = job_run_check_outcome(status, &response, &event.command);
                (
                    event.dispatcher,
                    McpCommandEvent {
                        command: event.command,
                        tool: event.tool,
                        request_id: event.request_id,
                        job_id: Some(job_id.clone()),
                        parameters: event.parameters,
                        status: event_status,
                        duration_ms: u64::try_from(event.started.elapsed().as_millis())
                            .unwrap_or(u64::MAX),
                        diagnostic,
                        outcome,
                    },
                )
            });
            registry.complete_logical(&job_id, status, response, draining);
            if let Some(completion_hook) = completion_hook {
                completion_hook();
            }
            if let Some((dispatcher, event)) = event_delivery {
                dispatcher.dispatch(event, telemetry);
            }
            lifecycle.wait_physical().await;
            registry.complete_physical(&job_id);
        });
        snapshot
    }

    pub(crate) fn snapshot(&self, job_id: &str) -> Result<JobSnapshot, JobRegistryError> {
        let mut records = lock_records(&self.records);
        self.cleanup_locked(&mut records, Instant::now());
        records
            .get(job_id)
            .map(|record| JobSnapshot {
                job_id: job_id.to_owned(),
                tool: record.tool.clone(),
                status: record.status,
                draining: record.draining,
                response: record.response.clone(),
            })
            .ok_or_else(|| JobRegistryError::NotFound {
                job_id: job_id.to_owned(),
            })
    }

    pub(crate) fn cancel(
        &self,
        job_id: &str,
        executor: &dyn Executor,
        map_runtime_error: fn(RuntimeError) -> CommandError,
    ) -> Result<JobSnapshot, JobRegistryError> {
        let target = {
            let mut records = lock_records(&self.records);
            self.cleanup_locked(&mut records, Instant::now());
            let record = records
                .get(job_id)
                .ok_or_else(|| JobRegistryError::NotFound {
                    job_id: job_id.to_owned(),
                })?;
            (record.status == JobStatus::Running)
                .then(|| (record.execution_id.clone(), record.lifecycle.clone()))
        };
        if let Some((execution_id, lifecycle)) = target
            && executor.cancel(&execution_id)
        {
            let mut records = lock_records(&self.records);
            if let Some(record) = records.get_mut(job_id)
                && record.status == JobStatus::Running
            {
                record.status = JobStatus::Cancelled;
                record.response = Some(TypedInvocationResponse::error(map_runtime_error(
                    RuntimeError::ExecutionCancelled {
                        request_id: execution_id,
                    },
                )));
                record.draining = lifecycle.as_ref().is_some_and(|value| value.is_draining());
            }
        }
        self.snapshot(job_id)
    }

    fn complete_logical(
        &self,
        job_id: &str,
        status: JobStatus,
        response: TypedInvocationResponse,
        draining: bool,
    ) {
        let mut records = lock_records(&self.records);
        let Some(record) = records.get_mut(job_id) else {
            return;
        };
        if record.status != JobStatus::Running {
            return;
        }
        record.status = status;
        record.response = Some(response);
        record.draining = draining;
    }

    fn complete_physical(&self, job_id: &str) {
        let mut records = lock_records(&self.records);
        let Some(record) = records.get_mut(job_id) else {
            return;
        };
        record.draining = false;
        record
            .physical_completed_at
            .get_or_insert_with(Instant::now);
        self.cleanup_locked(&mut records, Instant::now());
    }

    fn cleanup_locked(&self, records: &mut HashMap<String, JobRecord>, now: Instant) {
        records.retain(|_, record| {
            record
                .physical_completed_at
                .is_none_or(|completed_at| now.saturating_duration_since(completed_at) < self.ttl)
        });
    }
}

fn job_run_check_outcome(
    status: JobStatus,
    response: &TypedInvocationResponse,
    command: &str,
) -> Option<ah_runtime::InvocationOutcome> {
    if status != JobStatus::Succeeded || !response.success {
        return None;
    }
    run_check_outcome(command, response.data.as_ref())
}

fn oldest_completed(records: &HashMap<String, JobRecord>) -> Option<String> {
    records
        .iter()
        .filter_map(|(job_id, record)| {
            record
                .physical_completed_at
                .map(|completed_at| (job_id.clone(), completed_at, record.sequence))
        })
        .min_by_key(|(_, completed_at, sequence)| (*completed_at, *sequence))
        .map(|(job_id, _, _)| job_id)
}

fn lock_records(
    records: &Mutex<HashMap<String, JobRecord>>,
) -> std::sync::MutexGuard<'_, HashMap<String, JobRecord>> {
    records
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn boot_nonce() -> u64 {
    let time = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64;
    time ^ u64::from(std::process::id()).rotate_left(32)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use ah_plugin_api::TypedInvocationResponse;
    use ah_runtime::{InvocationOutcome, RunCheckOutcome};
    use serde_json::json;

    use super::{JobRegistry, JobRegistryError, JobStatus, job_run_check_outcome};

    #[test]
    fn capacity_rejects_when_every_record_is_active() {
        let registry = JobRegistry::new(2, Duration::from_secs(60));
        registry
            .reserve("ah.one".to_owned(), "one".to_owned())
            .unwrap();
        registry
            .reserve("ah.two".to_owned(), "two".to_owned())
            .unwrap();
        assert!(matches!(
            registry.reserve("ah.three".to_owned(), "three".to_owned()),
            Err(JobRegistryError::CapacityFull { capacity: 2 })
        ));
    }

    #[test]
    fn rollback_removes_hidden_reservation() {
        let registry = JobRegistry::new(1, Duration::from_secs(60));
        let reservation = registry
            .reserve("ah.one".to_owned(), "one".to_owned())
            .unwrap();
        registry.rollback(reservation);
        assert!(
            registry
                .reserve("ah.two".to_owned(), "two".to_owned())
                .is_ok()
        );
    }

    #[test]
    fn succeeded_run_check_job_emits_safe_outcome_only() {
        let response = TypedInvocationResponse::success(
            json!({
                "success": false,
                "timed_out": true,
                "exit_code": null,
                "stdout": "secret output",
                "stderr": "secret error"
            }),
            None,
        );

        assert_eq!(
            job_run_check_outcome(JobStatus::Succeeded, &response, "run.check"),
            Some(InvocationOutcome::RunCheck(RunCheckOutcome {
                success: false,
                timed_out: true,
                exit_code: None,
            }))
        );
        assert_eq!(
            job_run_check_outcome(JobStatus::TimedOut, &response, "run.check"),
            None
        );
        assert_eq!(
            job_run_check_outcome(JobStatus::Succeeded, &response, "search.text"),
            None
        );
    }
}
