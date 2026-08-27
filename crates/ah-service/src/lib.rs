//! The managed MCP service: a definition on disk, a scheduled task that runs
//! it, and a server process that answers.
//!
//! Three layers that can each be wrong independently, which is why `status` is
//! a reduction rather than a read: the scheduler can report a task that is
//! running while no server has come up, and a server can be answering from a
//! definition the scheduler no longer knows about.
//!
//! Everything here is mechanism. It renders nothing and reads no command line;
//! `ah`'s `service` module does both.
//!
//! Nothing above the scheduler port names a platform. `spec` says what a
//! service is, `scheduler` is the port that makes it true, `windows_task`
//! projects one onto a Task Scheduler definition and compares the readback,
//! and `windows_scheduler` - the only module here with a `cfg` on it - makes
//! the COM calls. `lifecycle` and its tests build against any adapter, which
//! is why the tests run on machines that have no Task Scheduler.

pub mod lifecycle;
pub mod lock;
pub mod model;
pub mod operation;
pub mod output;
pub mod paths;
pub mod readiness;
pub mod runner;
pub mod scheduler;
pub mod spec;
pub mod store;
pub mod systemd_unit;
pub mod windows_task;

#[cfg(target_os = "linux")]
pub mod systemd_scheduler;

#[cfg(windows)]
pub mod windows_scheduler;
