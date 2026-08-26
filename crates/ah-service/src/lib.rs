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

pub mod lifecycle;
pub mod lock;
pub mod model;
pub mod operation;
pub mod output;
pub mod paths;
pub mod readiness;
pub mod runner;
pub mod scheduler;
pub mod store;

#[cfg(windows)]
pub mod windows_scheduler;
