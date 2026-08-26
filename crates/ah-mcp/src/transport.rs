//! Serving the adapter over stdio and over local Streamable HTTP: readiness,
//! the control endpoints, the secret-setup routes, and the policy that decides
//! whether a request is local.

use std::{
    collections::BTreeMap,
    future::{Future, IntoFuture},
    net::Ipv4Addr,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use crate::server::{DEFAULT_SHUTDOWN_GRACE, McpAdapterError, McpServeOutcome, McpServer};
use crate::shutdown::{
    ShutdownAccepted, ShutdownReader, ShutdownRequest, ShutdownTracker, shutdown_signal,
};

use ah_runtime::executor::Executor;
use ah_setup_ui::{
    SecretSetupError, SecretSetupRequest, SecretSetupService, no_store, no_store_form, page_nonce,
    render_secret_setup_form, render_secret_setup_success, wants_html,
};
use axum::{
    Form, Json, Router,
    extract::{
        Query, State,
        rejection::{FormRejection, JsonRejection, QueryRejection},
    },
    http::{
        HeaderMap, StatusCode,
        header::{HOST, ORIGIN},
    },
    response::{Html, IntoResponse, Response},
    routing::{get, post},
};
use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
};
use rmcp::{ServiceExt, transport::stdio};
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;
use tower_http::limit::RequestBodyLimitLayer;
use uuid::Uuid;

#[derive(Clone)]
pub(crate) struct HttpLifecycleState {
    pub(crate) readiness: ReadinessResponse,
    pub(crate) authority: String,
    pub(crate) origin: String,
    pub(crate) lifecycle: Arc<HttpLifecycleController>,
    pub(crate) secret_setup: Option<Arc<dyn SecretSetupService>>,
}

impl HttpLifecycleState {
    pub(crate) fn new(
        version: String,
        instance_id: Uuid,
        pid: u32,
        authority: String,
        origin: String,
        lifecycle: Arc<HttpLifecycleController>,
        secret_setup: Option<Arc<dyn SecretSetupService>>,
    ) -> Self {
        Self {
            readiness: ReadinessResponse {
                status: "ready",
                version,
                pid,
                instance_id,
            },
            authority,
            origin,
            lifecycle,
            secret_setup,
        }
    }
}

#[derive(Clone, Serialize)]
pub(crate) struct ReadinessResponse {
    pub(crate) status: &'static str,
    pub(crate) version: String,
    pub(crate) pid: u32,
    pub(crate) instance_id: Uuid,
}

pub(crate) struct HttpLifecycleController {
    shutdown_started: AtomicBool,
    tracker: Arc<ShutdownTracker>,
    executor: Arc<dyn Executor>,
    cancellation: CancellationToken,
}

impl HttpLifecycleController {
    pub(crate) fn new(
        tracker: Arc<ShutdownTracker>,
        executor: Arc<dyn Executor>,
        cancellation: CancellationToken,
    ) -> Self {
        Self {
            shutdown_started: AtomicBool::new(false),
            tracker,
            executor,
            cancellation,
        }
    }

    pub(crate) fn begin_shutdown(&self) -> bool {
        if self
            .shutdown_started
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return false;
        }
        self.tracker.begin();
        self.executor.close();
        self.cancellation.cancel();
        true
    }

    pub(crate) async fn cancelled(&self) {
        self.cancellation.cancelled().await;
    }
}

#[derive(Serialize)]
pub(crate) struct ControlErrorResponse {
    error: ControlError,
}

#[derive(Serialize)]
pub(crate) struct ControlError {
    code: &'static str,
    message: &'static str,
}

pub async fn serve_stdio(server: McpServer) -> Result<(), McpAdapterError> {
    serve_stdio_bounded(server, DEFAULT_SHUTDOWN_GRACE)
        .await
        .into_result()
}

pub async fn serve_stdio_bounded(server: McpServer, grace: Duration) -> McpServeOutcome {
    let executor = Arc::clone(&server.shared.executor);
    let event_dispatcher = server.shared.event_dispatcher.clone();
    let tracker = Arc::new(ShutdownTracker::new(grace));
    let (stdin, stdout) = stdio();
    let reader = ShutdownReader::new(stdin, Arc::clone(&tracker), Arc::clone(&executor));
    let result = match server.serve((reader, stdout)).await {
        Ok(service) => {
            wait_for_transport(
                async {
                    let result = service.waiting().await;
                    tracker.begin();
                    executor.close();
                    result
                        .map(|_| ())
                        .map_err(|error| McpAdapterError::Service(error.to_string()))
                },
                tracker.as_ref(),
            )
            .await
        }
        Err(error) => {
            tracker.begin();
            executor.close();
            Err(McpAdapterError::Service(error.to_string()))
        }
    };
    tracker.begin();
    executor.close();
    if let Some(dispatcher) = event_dispatcher {
        dispatcher.flush(tracker.remaining()).await;
    }
    McpServeOutcome {
        result,
        remaining_shutdown_grace: tracker.remaining(),
    }
}

pub async fn serve_http(server: McpServer, port: u16) -> Result<(), McpAdapterError> {
    serve_http_bounded(server, port, DEFAULT_SHUTDOWN_GRACE)
        .await
        .into_result()
}

pub async fn serve_http_bounded(server: McpServer, port: u16, grace: Duration) -> McpServeOutcome {
    serve_http_bounded_with_version(server, port, env!("CARGO_PKG_VERSION"), grace).await
}

pub async fn serve_http_bounded_with_version(
    server: McpServer,
    port: u16,
    version: impl Into<String>,
    grace: Duration,
) -> McpServeOutcome {
    serve_http_bounded_with_identity_and_listener(
        server,
        port,
        version,
        Uuid::new_v4(),
        std::process::id(),
        grace,
        || Ok(()),
    )
    .await
}

pub async fn serve_http_bounded_with_identity_and_listener<F>(
    server: McpServer,
    port: u16,
    version: impl Into<String>,
    instance_id: Uuid,
    pid: u32,
    grace: Duration,
    on_listener_bound: F,
) -> McpServeOutcome
where
    F: FnOnce() -> Result<(), McpAdapterError>,
{
    let tracker = Arc::new(ShutdownTracker::new(grace));
    let executor = Arc::clone(&server.shared.executor);
    let event_dispatcher = server.shared.event_dispatcher.clone();
    let authority = format!("127.0.0.1:{port}");
    let origin = format!("http://{authority}");
    let cancellation = CancellationToken::new();
    let lifecycle_controller = Arc::new(HttpLifecycleController::new(
        Arc::clone(&tracker),
        Arc::clone(&executor),
        cancellation.clone(),
    ));
    let lifecycle = HttpLifecycleState::new(
        version.into(),
        instance_id,
        pid,
        authority.clone(),
        origin.clone(),
        Arc::clone(&lifecycle_controller),
        server.shared.secret_setup.clone(),
    );
    let config = StreamableHttpServerConfig::default()
        .with_stateful_mode(true)
        .with_allowed_hosts([authority])
        .with_allowed_origins([origin])
        .with_cancellation_token(cancellation);
    let session_template = Arc::new(server);
    let factory_template = Arc::clone(&session_template);
    let service: StreamableHttpService<McpServer, LocalSessionManager> = StreamableHttpService::new(
        move || Ok(factory_template.http_session()),
        Arc::new(LocalSessionManager::default()),
        config,
    );
    let router = Router::new()
        .route("/health/ready", get(readiness))
        .route("/control/shutdown", post(control_shutdown))
        .route("/secrets/setup/capability", post(secret_setup_capability))
        .route(
            "/secrets/setup",
            get(secret_setup_form).post(secret_setup_submit),
        )
        .nest_service("/mcp", service)
        .with_state(lifecycle)
        .layer(RequestBodyLimitLayer::new(1024 * 1024));
    let listener = match tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, port)).await {
        Ok(listener) => listener,
        Err(error) => {
            lifecycle_controller.begin_shutdown();
            return McpServeOutcome {
                result: Err(McpAdapterError::Service(error.to_string())),
                remaining_shutdown_grace: tracker.remaining(),
            };
        }
    };
    if let Err(error) = on_listener_bound() {
        lifecycle_controller.begin_shutdown();
        return McpServeOutcome {
            result: Err(error),
            remaining_shutdown_grace: tracker.remaining(),
        };
    }
    let shutdown_controller = Arc::clone(&lifecycle_controller);
    let serving = axum::serve(listener, router)
        .with_graceful_shutdown(async move {
            tokio::select! {
                _ = shutdown_signal() => {
                    shutdown_controller.begin_shutdown();
                }
                _ = shutdown_controller.cancelled() => {}
            }
        })
        .into_future();
    let result = wait_for_transport(
        async move {
            serving
                .await
                .map_err(|error| McpAdapterError::Service(error.to_string()))
        },
        tracker.as_ref(),
    )
    .await;
    lifecycle_controller.begin_shutdown();
    if let Some(dispatcher) = event_dispatcher {
        dispatcher.flush(tracker.remaining()).await;
    }
    McpServeOutcome {
        result,
        remaining_shutdown_grace: tracker.remaining(),
    }
}

pub(crate) async fn wait_for_transport<F>(
    transport: F,
    tracker: &ShutdownTracker,
) -> Result<(), McpAdapterError>
where
    F: Future<Output = Result<(), McpAdapterError>>,
{
    tokio::pin!(transport);
    tokio::select! {
        biased;
        result = &mut transport => result,
        _ = tracker.expired() => Err(McpAdapterError::ShutdownTimeout {
            grace_ms: tracker.grace.as_millis(),
        }),
    }
}

pub(crate) async fn readiness(
    State(state): State<HttpLifecycleState>,
    headers: HeaderMap,
) -> Result<Json<ReadinessResponse>, StatusCode> {
    validate_local_headers(&headers, &state)?;
    Ok(Json(state.readiness))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SetupCapabilityQuery {
    capability: String,
}

#[derive(Serialize)]
pub(crate) struct SetupCapabilityResponse {
    setup_url: String,
}

pub(crate) async fn secret_setup_capability(
    State(state): State<HttpLifecycleState>,
    headers: HeaderMap,
    request: Result<Json<SecretSetupRequest>, JsonRejection>,
) -> Response {
    if validate_local_headers(&headers, &state).is_err() {
        return control_error(
            StatusCode::FORBIDDEN,
            "LOCAL_REQUEST_REJECTED",
            "request does not match the local HTTP policy",
        );
    }
    let Some(service) = state.secret_setup else {
        return control_error(
            StatusCode::NOT_FOUND,
            "VAULT_SETUP_UNAVAILABLE",
            "secret setup is unavailable",
        );
    };
    let Ok(Json(request)) = request else {
        return control_error(
            StatusCode::BAD_REQUEST,
            "VAULT_SETUP_REQUEST_INVALID",
            "secret setup request is invalid",
        );
    };
    match service.issue(request) {
        Ok(capability) => no_store(
            Json(SetupCapabilityResponse {
                setup_url: format!(
                    "http://{}/secrets/setup?capability={capability}",
                    state.authority
                ),
            })
            .into_response(),
        ),
        Err(error) => setup_error(error),
    }
}

pub(crate) async fn secret_setup_form(
    State(state): State<HttpLifecycleState>,
    headers: HeaderMap,
    query: Result<Query<SetupCapabilityQuery>, QueryRejection>,
) -> Response {
    if validate_local_headers(&headers, &state).is_err() {
        return control_error(
            StatusCode::FORBIDDEN,
            "LOCAL_REQUEST_REJECTED",
            "request does not match the local HTTP policy",
        );
    }
    let Some(service) = state.secret_setup else {
        return control_error(
            StatusCode::NOT_FOUND,
            "VAULT_SETUP_UNAVAILABLE",
            "secret setup is unavailable",
        );
    };
    let Ok(Query(query)) = query else {
        return setup_error(SecretSetupError::new(
            "VAULT_SETUP_CAPABILITY_INVALID",
            "secret setup capability is invalid, expired, or already used",
        ));
    };
    match service.form(&query.capability) {
        Ok(form) => {
            let nonce = page_nonce();
            no_store_form(
                Html(render_secret_setup_form(&form, &nonce)).into_response(),
                &nonce,
            )
        }
        Err(error) => setup_error(error),
    }
}

pub(crate) async fn secret_setup_submit(
    State(state): State<HttpLifecycleState>,
    headers: HeaderMap,
    query: Result<Query<SetupCapabilityQuery>, QueryRejection>,
    form: Result<Form<BTreeMap<String, String>>, FormRejection>,
) -> Response {
    if validate_local_headers(&headers, &state).is_err() {
        return control_error(
            StatusCode::FORBIDDEN,
            "LOCAL_REQUEST_REJECTED",
            "request does not match the local HTTP policy",
        );
    }
    let Some(service) = state.secret_setup else {
        return control_error(
            StatusCode::NOT_FOUND,
            "VAULT_SETUP_UNAVAILABLE",
            "secret setup is unavailable",
        );
    };
    let Ok(Query(query)) = query else {
        return setup_error(SecretSetupError::new(
            "VAULT_SETUP_CAPABILITY_INVALID",
            "secret setup capability is invalid, expired, or already used",
        ));
    };
    let Ok(Form(values)) = form else {
        return control_error(
            StatusCode::BAD_REQUEST,
            "VAULT_SETUP_SUBMISSION_INVALID",
            "secret setup form is invalid",
        );
    };
    match service.submit(&query.capability, values) {
        Ok(metadata) if wants_html(&headers) => {
            let nonce = page_nonce();
            no_store_form(
                Html(render_secret_setup_success(&metadata, &nonce)).into_response(),
                &nonce,
            )
        }
        Ok(metadata) => no_store(Json(metadata).into_response()),
        Err(error) => setup_error(error),
    }
}

pub(crate) fn setup_error(error: SecretSetupError) -> Response {
    let status = if error.code == "VAULT_SETUP_CAPABILITY_INVALID" {
        StatusCode::FORBIDDEN
    } else {
        StatusCode::BAD_REQUEST
    };
    control_error(status, error.code, error.message)
}

pub(crate) async fn control_shutdown(
    State(state): State<HttpLifecycleState>,
    headers: HeaderMap,
    request: Result<Json<ShutdownRequest>, JsonRejection>,
) -> Response {
    if validate_local_headers(&headers, &state).is_err() {
        return control_error(
            StatusCode::FORBIDDEN,
            "LOCAL_REQUEST_REJECTED",
            "request does not match the local HTTP policy",
        );
    }

    let request = match request {
        Ok(Json(request)) => request,
        Err(rejection) if rejection.status() == StatusCode::UNSUPPORTED_MEDIA_TYPE => {
            return control_error(
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
                "UNSUPPORTED_MEDIA_TYPE",
                "request content type must be application/json",
            );
        }
        Err(_) => {
            return control_error(
                StatusCode::BAD_REQUEST,
                "INVALID_SHUTDOWN_REQUEST",
                "request body must contain exactly one valid instance_id",
            );
        }
    };

    if request.instance_id != state.readiness.instance_id {
        return control_error(
            StatusCode::CONFLICT,
            "INSTANCE_ID_MISMATCH",
            "instance_id does not match the running AIHelper instance",
        );
    }

    state.lifecycle.begin_shutdown();
    (
        StatusCode::ACCEPTED,
        Json(ShutdownAccepted {
            status: "shutting_down",
            instance_id: request.instance_id,
        }),
    )
        .into_response()
}

pub(crate) fn control_error(
    status: StatusCode,
    code: &'static str,
    message: &'static str,
) -> Response {
    (
        status,
        Json(ControlErrorResponse {
            error: ControlError { code, message },
        }),
    )
        .into_response()
}

pub(crate) fn validate_local_headers(
    headers: &HeaderMap,
    state: &HttpLifecycleState,
) -> Result<(), StatusCode> {
    let host_matches = headers
        .get(HOST)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|host| host == state.authority);
    if !host_matches {
        return Err(StatusCode::FORBIDDEN);
    }

    let origin_matches = headers.get(ORIGIN).is_none_or(|value| {
        value
            .to_str()
            .ok()
            .is_some_and(|origin| origin == state.origin)
    });
    if !origin_matches {
        return Err(StatusCode::FORBIDDEN);
    }

    Ok(())
}
