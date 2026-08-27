//! Per-request cancellation, shared by every command handler.
//!
//! The host cancels by request id — `cancel_command_v1` across the C ABI, and
//! `BuiltinPlugin::cancel_typed` in process — so a handler has to answer "was
//! the request I am serving cancelled?" without being handed a token. There is
//! no way to pass one: the ABI carries JSON, not pointers.
//!
//! That answer needs two pieces of state, and every domain that supported
//! cancellation had grown its own copy of both: a process-wide set of cancelled
//! ids, and a thread-local naming the request the current thread is serving.
//! Five copies meant five chances for one of them to leak an entry or check the
//! wrong thread. This is the single copy.
//!
//! A handler brackets its work with [`RequestScope::enter`] and then polls
//! [`is_cancelled`] wherever it can stop cheaply — between files, between output
//! chunks, around a child process wait:
//!
//! ```ignore
//! let _scope = RequestScope::enter(&request.context.request_id);
//! if is_cancelled() {
//!     return cancelled_response(request);
//! }
//! ```
//!
//! Dropping the scope removes the id from the cancelled set, so a cancellation
//! that arrives for an id that is never served again cannot accumulate.

use std::{
    cell::RefCell,
    collections::HashSet,
    sync::{Condvar, Mutex, MutexGuard, OnceLock},
    time::Duration,
};

/// The cancelled set plus a signal, so a handler that is waiting on a poll
/// interval can wake the moment it is cancelled instead of sleeping it out.
struct Registry {
    request_ids: Mutex<HashSet<String>>,
    changed: Condvar,
}

fn registry() -> &'static Registry {
    static REGISTRY: OnceLock<Registry> = OnceLock::new();
    REGISTRY.get_or_init(|| Registry {
        request_ids: Mutex::new(HashSet::new()),
        changed: Condvar::new(),
    })
}

thread_local! {
    static CURRENT_REQUEST_ID: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// Marks the calling thread as serving one request until dropped.
///
/// Restores whatever request the thread was serving before, so a handler that
/// invokes another one nests correctly rather than clearing the outer request.
#[must_use = "cancellation is only observable while the scope is alive"]
pub struct RequestScope {
    request_id: String,
    previous_request_id: Option<String>,
}

impl RequestScope {
    pub fn enter(request_id: impl Into<String>) -> Self {
        let request_id = request_id.into();
        let previous_request_id =
            CURRENT_REQUEST_ID.with(|current| current.replace(Some(request_id.clone())));
        Self {
            request_id,
            previous_request_id,
        }
    }
}

impl Drop for RequestScope {
    fn drop(&mut self) {
        CURRENT_REQUEST_ID.with(|current| current.replace(self.previous_request_id.take()));
        lock_unpoisoned().remove(&self.request_id);
    }
}

/// Record that a request should stop.
///
/// Always reports success: a cancellation may arrive before the handler starts,
/// and it is honoured when the handler enters its scope and polls.
pub fn cancel(request_id: &str) -> bool {
    lock_unpoisoned().insert(request_id.to_owned());
    registry().changed.notify_all();
    true
}

/// Sleep up to `duration`, returning as soon as the current request is
/// cancelled. Reports whether it was.
///
/// This is what a handler polling a remote job uses instead of `thread::sleep`:
/// a 15-second poll interval would otherwise make cancellation take up to 15
/// seconds to be noticed.
pub fn wait_or_cancel(duration: Duration) -> bool {
    let Some(request_id) = CURRENT_REQUEST_ID.with(|current| current.borrow().clone()) else {
        std::thread::sleep(duration);
        return false;
    };
    let guard = lock_unpoisoned();
    let (guard, _timeout) = registry()
        .changed
        .wait_timeout_while(guard, duration, |cancelled| {
            !cancelled.contains(&request_id)
        })
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    guard.contains(&request_id)
}

/// Whether the request the calling thread is serving has been cancelled.
///
/// False when the thread is not inside a [`RequestScope`], which is the case for
/// ordinary CLI invocations — there is nothing to cancel them with.
pub fn is_cancelled() -> bool {
    let Some(request_id) = CURRENT_REQUEST_ID.with(|current| current.borrow().clone()) else {
        return false;
    };
    lock_unpoisoned().contains(&request_id)
}

/// A poisoned mutex here means a handler panicked mid-check; the set is a plain
/// collection of ids and cannot be left inconsistent, so recovering is correct.
fn lock_unpoisoned() -> MutexGuard<'static, HashSet<String>> {
    registry()
        .request_ids
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The refusal a handler answers with when its request was cancelled before it
/// ran.
///
/// Five domains wrote this out, differing only in the two strings below; the
/// diagnostic code, the detail wording and the exit code are the contract and
/// were identical in all five.
///
/// `domain` names the domain the command belongs to and `summary` is the
/// one-line message the caller sees, which stays each domain's own wording.
#[must_use]
pub fn cancelled_response(
    domain: &str,
    summary: &str,
    request: &crate::TypedInvocationRequest,
) -> crate::TypedInvocationResponse {
    crate::TypedInvocationResponse::error(crate::CommandError::new(
        Some(domain.to_owned()),
        Some(request.command.clone()),
        "EXECUTION_CANCELLED",
        summary,
        format!(
            "request '{}' was cancelled before handler execution",
            request.context.request_id
        ),
        1,
        false,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_thread_outside_any_scope_is_never_cancelled() {
        assert!(!is_cancelled());
    }

    #[test]
    fn cancellation_delivered_before_the_scope_is_still_observed() {
        let request_id = "cancellation-before-scope";
        assert!(cancel(request_id));
        let _scope = RequestScope::enter(request_id);
        assert!(is_cancelled());
    }

    #[test]
    fn leaving_the_scope_forgets_the_request() {
        let request_id = "cancellation-forgotten-on-exit";
        {
            let _scope = RequestScope::enter(request_id);
            assert!(cancel(request_id));
            assert!(is_cancelled());
        }
        assert!(!lock_unpoisoned().contains(request_id));
        // Re-entering the same id must not inherit the previous cancellation.
        let _scope = RequestScope::enter(request_id);
        assert!(!is_cancelled());
    }

    #[test]
    fn a_nested_scope_restores_the_outer_request() {
        let outer = "cancellation-outer";
        let inner = "cancellation-inner";
        let _outer_scope = RequestScope::enter(outer);
        {
            let _inner_scope = RequestScope::enter(inner);
            assert!(cancel(inner));
            assert!(is_cancelled());
        }
        assert!(!is_cancelled(), "the outer request was not cancelled");
        assert!(cancel(outer));
        assert!(is_cancelled());
    }

    #[test]
    fn waiting_returns_early_when_the_request_is_cancelled() {
        let request_id = "cancellation-wakes-the-waiter";
        let _scope = RequestScope::enter(request_id);
        let waker = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(20));
            cancel(request_id);
        });

        let started = std::time::Instant::now();
        let cancelled = wait_or_cancel(Duration::from_secs(30));
        let waited = started.elapsed();

        waker.join().expect("waker thread should finish");
        assert!(cancelled, "the wait did not observe the cancellation");
        assert!(
            waited < Duration::from_secs(5),
            "the wait slept through the cancellation: {waited:?}"
        );
    }

    #[test]
    fn cancelling_one_request_does_not_cancel_another() {
        let served = "cancellation-served";
        let other = "cancellation-other";
        let _scope = RequestScope::enter(served);
        assert!(cancel(other));
        assert!(!is_cancelled());
    }
}
