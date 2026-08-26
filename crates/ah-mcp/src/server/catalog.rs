//! Telling connected peers that the tool list changed.
//!
//! Free functions rather than methods, because a job completing has to notify
//! through a `Weak<McpShared>` with no server to call it on.

use super::*;

pub(crate) fn refresh_catalog_generation_shared(
    shared: &McpShared,
) -> Result<bool, McpAdapterError> {
    let runtime_revision = shared.manager.catalog_revision();
    if shared
        .catalog_snapshot
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .runtime_revision
        == runtime_revision
    {
        return Ok(false);
    }
    let next = Arc::new(build_catalog_snapshot(&shared.manager)?);
    let mut current = shared
        .catalog_snapshot
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if current.runtime_revision >= next.runtime_revision {
        return Ok(false);
    }
    *current = next;
    shared.catalog_generation.fetch_add(1, Ordering::AcqRel);
    Ok(true)
}

pub(crate) fn notify_tool_list_changed_shared(shared: &Arc<McpShared>) {
    let peers = shared
        .peers
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .iter()
        .map(|(session_id, registered)| {
            (*session_id, registered.generation, registered.peer.clone())
        })
        .collect::<Vec<_>>();
    for (session_id, generation, peer) in peers {
        let shared = Arc::clone(shared);
        spawn_best_effort_notification(
            async move { peer.notify_tool_list_changed().await.is_ok() },
            PEER_NOTIFICATION_TIMEOUT,
            move || {
                remove_peer_generation(&shared, session_id, generation);
            },
        );
    }
}

pub(crate) fn refresh_catalog_after_job(shared: Weak<McpShared>) {
    let Some(shared) = shared.upgrade() else {
        return;
    };
    if refresh_catalog_generation_shared(&shared).unwrap_or(false) {
        notify_tool_list_changed_shared(&shared);
    }
}

pub(super) fn remove_peer_generation(shared: &McpShared, session_id: u64, generation: u64) -> bool {
    let mut peers = shared
        .peers
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if peer_generation_matches(
        peers.get(&session_id).map(|peer| peer.generation),
        generation,
    ) {
        peers.remove(&session_id);
        true
    } else {
        false
    }
}

pub(super) fn peer_generation_matches(current: Option<u64>, expected: u64) -> bool {
    current == Some(expected)
}

pub(super) fn spawn_best_effort_notification<F, C>(notification: F, timeout: Duration, on_stale: C)
where
    F: Future<Output = bool> + Send + 'static,
    C: FnOnce() + Send + 'static,
{
    tokio::spawn(async move {
        let delivered = tokio::time::timeout(timeout, notification)
            .await
            .unwrap_or(false);
        if !delivered {
            on_stale();
        }
    });
}

/// Clone tool arguments for event logging with the `context` wrapper removed.
///
/// The `context` object carries the caller's `cwd`, `limit`, and `timeout_ms`,
/// which are execution plumbing rather than tool inputs and must not leak into
/// the recorded parameter payload.
pub(crate) fn event_parameters(arguments: &JsonObject) -> Value {
    let mut parameters = arguments.clone();
    parameters.remove("context");
    redact_mcp_plaintext_auth(&mut parameters);
    Value::Object(parameters)
}
