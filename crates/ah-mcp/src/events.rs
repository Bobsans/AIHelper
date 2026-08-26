use std::{
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{
        Arc,
        mpsc::{SyncSender, TrySendError, sync_channel},
    },
    thread,
};

use ah_runtime::executor::ExecutionTelemetry;
use tokio::sync::oneshot;

use crate::server::{EventSink, McpCommandEvent};

pub(crate) const DEFAULT_EVENT_QUEUE_CAPACITY: usize = 256;

pub(crate) struct EventDispatcher {
    sender: SyncSender<EventMessage>,
}

enum EventMessage {
    Delivery(Box<EventDelivery>),
    Flush(oneshot::Sender<()>),
}

struct EventDelivery {
    event: McpCommandEvent,
    telemetry: Option<ExecutionTelemetry>,
}

impl EventDispatcher {
    pub(crate) fn new(sink: Arc<dyn EventSink>, capacity: usize) -> Arc<Self> {
        let (sender, receiver) = sync_channel(capacity);
        let dispatcher = Arc::new(Self { sender });
        let _ = thread::Builder::new()
            .name("ah-mcp-events".to_owned())
            .spawn(move || {
                while let Ok(message) = receiver.recv() {
                    match message {
                        EventMessage::Delivery(delivery) => {
                            let _ = catch_unwind(AssertUnwindSafe(|| {
                                sink.record_command_with_telemetry(
                                    delivery.event,
                                    delivery.telemetry,
                                );
                            }));
                        }
                        EventMessage::Flush(completed) => {
                            let _ = completed.send(());
                        }
                    }
                }
            });
        dispatcher
    }

    pub(crate) fn standard(sink: Arc<dyn EventSink>) -> Arc<Self> {
        Self::new(sink, DEFAULT_EVENT_QUEUE_CAPACITY)
    }

    pub(crate) fn dispatch(
        &self,
        event: McpCommandEvent,
        telemetry: Option<ExecutionTelemetry>,
    ) -> bool {
        match self
            .sender
            .try_send(EventMessage::Delivery(Box::new(EventDelivery {
                event,
                telemetry,
            }))) {
            Ok(()) => true,
            // Overflow drops the event rather than blocking the caller. The
            // `false` is the whole report: nothing surfaces an aggregate count.
            Err(TrySendError::Full(_) | TrySendError::Disconnected(_)) => false,
        }
    }

    pub(crate) async fn flush(&self, timeout: std::time::Duration) -> bool {
        if timeout.is_zero() {
            return false;
        }
        let (completed, waiting) = oneshot::channel();
        if self
            .sender
            .try_send(EventMessage::Flush(completed))
            .is_err()
        {
            return false;
        }
        matches!(tokio::time::timeout(timeout, waiting).await, Ok(Ok(())))
    }
}

#[cfg(test)]
mod tests {
    use std::{
        sync::{Arc, Mutex, mpsc},
        time::Duration,
    };

    use crate::server::{EventSink, McpCommandEvent, McpCommandStatus};

    use super::EventDispatcher;

    struct BlockingSink {
        started: mpsc::Sender<()>,
        release: Mutex<mpsc::Receiver<()>>,
    }

    impl EventSink for BlockingSink {
        fn record_command(&self, _event: McpCommandEvent) {
            self.started.send(()).unwrap();
            self.release.lock().unwrap().recv().unwrap();
        }
    }

    struct PanickingSink;

    impl EventSink for PanickingSink {
        fn record_command(&self, _event: McpCommandEvent) {
            panic!("event sink panic");
        }
    }

    struct RecordingSink(mpsc::Sender<()>);

    impl EventSink for RecordingSink {
        fn record_command(&self, _event: McpCommandEvent) {
            self.0.send(()).unwrap();
        }
    }

    fn event(request_id: &str) -> McpCommandEvent {
        McpCommandEvent {
            command: "test.echo".to_owned(),
            tool: "ah.test.echo".to_owned(),
            request_id: request_id.to_owned(),
            job_id: None,
            parameters: serde_json::json!({}),
            status: McpCommandStatus::Success,
            duration_ms: 1,
            diagnostic: None,
            outcome: None,
        }
    }

    #[test]
    fn blocking_sink_does_not_block_dispatch_and_overflow_is_dropped() {
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let dispatcher = EventDispatcher::new(
            Arc::new(BlockingSink {
                started: started_tx,
                release: Mutex::new(release_rx),
            }),
            1,
        );

        assert!(dispatcher.dispatch(event("first"), None));
        started_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert!(dispatcher.dispatch(event("second"), None));
        assert!(!dispatcher.dispatch(event("third"), None));
        release_tx.send(()).unwrap();
    }

    #[test]
    fn panicking_sink_does_not_stop_dispatcher_thread() {
        let dispatcher = EventDispatcher::new(Arc::new(PanickingSink), 2);
        assert!(dispatcher.dispatch(event("first"), None));
        assert!(dispatcher.dispatch(event("second"), None));
    }

    #[test]
    fn flush_waits_for_preceding_healthy_events() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            let (recorded_tx, recorded_rx) = mpsc::channel();
            let dispatcher = EventDispatcher::new(Arc::new(RecordingSink(recorded_tx)), 2);
            assert!(dispatcher.dispatch(event("first"), None));
            assert!(dispatcher.flush(Duration::from_secs(1)).await);
            recorded_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        });
    }
}
