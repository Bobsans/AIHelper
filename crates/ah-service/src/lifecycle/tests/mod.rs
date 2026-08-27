//! The lifecycle's own tests. They drive a scripted scheduler rather than a
//! real one, so they run on every platform - which is the check that the
//! lifecycle above the scheduler port really is platform-neutral.

mod concurrency;
mod harness;
mod install_start_status;
mod reducer;
mod stop_restart;
mod uninstall_recovery;
