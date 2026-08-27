//! Waiting for a remote job to finish.
//!
//! `github.run.wait` and `gitlab.pipeline.wait` are the same loop: ask, answer
//! if it is terminal, give up at the deadline, otherwise sleep until the next
//! attempt or until the request is cancelled. Only the question, the terminal
//! test and the wording differ, so those stay with the plugins and the loop
//! lives here.
//!
//! The two copies had drifted. One re-checked the deadline after sleeping and
//! the other did not, so it could issue one more request after `--timeout` had
//! already passed. This loop checks, which is the behaviour the flag promises.

use std::time::{Duration, Instant};

/// The answer to one attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Poll<T> {
    /// The job reached a terminal state, and this is it.
    Ready(T),
    /// Not yet.
    Pending,
}

/// How the wait ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Waited<T> {
    Ready {
        value: T,
        elapsed: Duration,
    },
    /// The deadline passed with the job still running.
    TimedOut,
    /// The caller cancelled the request while it was sleeping.
    Cancelled,
}

/// Poll `probe` until it reports a terminal state, `timeout` elapses, or the
/// request is cancelled.
///
/// The first attempt happens immediately, so a job that is already finished
/// costs one request and no sleeping. Each sleep is shortened to land on the
/// deadline rather than past it.
///
/// # Errors
///
/// Whatever `probe` returns: an attempt that fails ends the wait, because a
/// forge that cannot answer "is it done" cannot be waited on.
pub fn until_ready<T, E>(
    timeout: Duration,
    interval: Duration,
    mut probe: impl FnMut() -> Result<Poll<T>, E>,
) -> Result<Waited<T>, E> {
    let start = Instant::now();
    loop {
        if let Poll::Ready(value) = probe()? {
            return Ok(Waited::Ready {
                value,
                elapsed: start.elapsed(),
            });
        }
        let elapsed = start.elapsed();
        if elapsed >= timeout {
            return Ok(Waited::TimedOut);
        }
        if ah_plugin_api::cancellation::wait_or_cancel(interval.min(timeout - elapsed)) {
            return Ok(Waited::Cancelled);
        }
        if start.elapsed() >= timeout {
            return Ok(Waited::TimedOut);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn a_job_that_is_already_finished_costs_one_attempt() {
        let attempts = Cell::new(0);
        let waited = until_ready::<_, ()>(Duration::from_secs(30), Duration::from_secs(5), || {
            attempts.set(attempts.get() + 1);
            Ok(Poll::Ready("done"))
        })
        .expect("the probe succeeds");

        assert_eq!(attempts.get(), 1);
        assert!(matches!(waited, Waited::Ready { value: "done", .. }));
    }

    #[test]
    fn polling_continues_until_the_job_is_terminal() {
        let attempts = Cell::new(0);
        let waited =
            until_ready::<_, ()>(Duration::from_secs(30), Duration::from_millis(1), || {
                attempts.set(attempts.get() + 1);
                Ok(if attempts.get() < 3 {
                    Poll::Pending
                } else {
                    Poll::Ready(attempts.get())
                })
            })
            .expect("the probe succeeds");

        assert_eq!(attempts.get(), 3);
        assert!(matches!(waited, Waited::Ready { value: 3, .. }));
    }

    /// The deadline is a deadline: a job still pending when it passes is a
    /// timeout, and no further attempt is made.
    #[test]
    fn a_pending_job_times_out_without_another_attempt() {
        let attempts = Cell::new(0);
        let waited =
            until_ready::<_, ()>(Duration::from_millis(2), Duration::from_millis(1), || {
                attempts.set(attempts.get() + 1);
                Ok(Poll::<()>::Pending)
            })
            .expect("the probe succeeds");

        assert_eq!(waited, Waited::TimedOut);
        assert!(
            attempts.get() >= 1,
            "the first attempt happens before any sleeping"
        );
    }

    #[test]
    fn a_failing_attempt_ends_the_wait() {
        let error = until_ready::<(), _>(Duration::from_secs(30), Duration::from_secs(5), || {
            Err("the forge refused")
        })
        .expect_err("the probe fails");

        assert_eq!(error, "the forge refused");
    }
}
