//! The shutdown protocol: the control request, the tracker that decides
//! whether a shutdown was accepted, and the stdin reader that watches for the
//! peer going away.

use std::{
    pin::Pin,
    sync::{Arc, OnceLock},
    task::{Context, Poll},
    time::{Duration, Instant},
};

use ah_runtime::executor::Executor;
use serde::{Deserialize, Serialize};
use tokio::{
    io::{AsyncRead, ReadBuf},
    sync::Notify,
};
use uuid::Uuid;

pub(crate) struct ShutdownTracker {
    pub(crate) started_at: OnceLock<Instant>,
    pub(crate) changed: Notify,
    pub(crate) grace: Duration,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ShutdownRequest {
    pub(crate) instance_id: Uuid,
}

#[derive(Serialize)]
pub(crate) struct ShutdownAccepted {
    pub(crate) status: &'static str,
    pub(crate) instance_id: Uuid,
}

impl ShutdownTracker {
    pub(crate) fn new(grace: Duration) -> Self {
        Self {
            started_at: OnceLock::new(),
            changed: Notify::new(),
            grace,
        }
    }

    pub(crate) fn begin(&self) {
        if self.started_at.set(Instant::now()).is_ok() {
            self.changed.notify_waiters();
        }
    }

    pub(crate) fn remaining(&self) -> Duration {
        self.started_at.get().map_or(self.grace, |started_at| {
            self.grace.saturating_sub(started_at.elapsed())
        })
    }

    pub(crate) async fn expired(&self) {
        loop {
            if let Some(started_at) = self.started_at.get() {
                tokio::time::sleep_until((*started_at + self.grace).into()).await;
                return;
            }
            let changed = self.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if self.started_at.get().is_some() {
                continue;
            }
            changed.await;
        }
    }
}

pub(crate) struct ShutdownReader<R> {
    inner: R,
    tracker: Arc<ShutdownTracker>,
    executor: Arc<dyn Executor>,
    shutdown_started: bool,
}

impl<R> ShutdownReader<R> {
    pub(crate) fn new(
        inner: R,
        tracker: Arc<ShutdownTracker>,
        executor: Arc<dyn Executor>,
    ) -> Self {
        Self {
            inner,
            tracker,
            executor,
            shutdown_started: false,
        }
    }

    pub(crate) fn begin_shutdown(&mut self) {
        if !self.shutdown_started {
            self.shutdown_started = true;
            self.tracker.begin();
            self.executor.close();
        }
    }
}

impl<R: AsyncRead + Unpin> AsyncRead for ShutdownReader<R> {
    fn poll_read(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let this = self.get_mut();
        let had_capacity = buffer.remaining() > 0;
        let filled_before = buffer.filled().len();
        let result = Pin::new(&mut this.inner).poll_read(context, buffer);
        if matches!(&result, Poll::Ready(Err(_)))
            || matches!(&result, Poll::Ready(Ok(())) if had_capacity && buffer.filled().len() == filled_before)
        {
            this.begin_shutdown();
        }
        result
    }
}

#[cfg(unix)]
pub(crate) async fn shutdown_signal() {
    let ctrl_c = tokio::signal::ctrl_c();
    match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
        Ok(mut terminate) => {
            tokio::select! {
                _ = ctrl_c => {}
                _ = terminate.recv() => {}
            }
        }
        Err(_) => {
            let _ = ctrl_c.await;
        }
    }
}

#[cfg(not(unix))]
pub(crate) async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
}
