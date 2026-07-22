use std::time::Duration;

use serde::Deserialize;
use uuid::Uuid;

use super::{
    model::{RuntimeState, ServiceDefinition},
    output::{ReadinessSection, ReadinessStatus},
};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReadinessResponse {
    status: String,
    version: String,
    pid: u32,
    instance_id: Uuid,
}

pub trait ReadinessProbe {
    fn inspect(
        &self,
        definition: &ServiceDefinition,
        runtime: Option<&RuntimeState>,
    ) -> ReadinessSection;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShutdownReceipt {
    Accepted,
    Failed { detail: String },
}

pub trait RuntimeControl: ReadinessProbe {
    fn shutdown(&self, definition: &ServiceDefinition, instance_id: Uuid) -> ShutdownReceipt;
}

#[derive(Debug, Clone)]
pub struct HttpReadinessProbe {
    client: reqwest::blocking::Client,
    control_client: reqwest::blocking::Client,
}

impl HttpReadinessProbe {
    pub fn new() -> Result<Self, reqwest::Error> {
        Ok(Self {
            client: reqwest::blocking::Client::builder()
                .connect_timeout(Duration::from_millis(250))
                .timeout(Duration::from_millis(500))
                .build()?,
            control_client: reqwest::blocking::Client::builder()
                .connect_timeout(Duration::from_millis(500))
                .timeout(Duration::from_secs(2))
                .build()?,
        })
    }
}

impl RuntimeControl for HttpReadinessProbe {
    fn shutdown(&self, definition: &ServiceDefinition, instance_id: Uuid) -> ShutdownReceipt {
        let url = format!(
            "http://127.0.0.1:{}/control/shutdown",
            definition.endpoint.port
        );
        match self
            .control_client
            .post(url)
            .json(&serde_json::json!({"instance_id": instance_id}))
            .send()
        {
            Ok(response) if response.status() == reqwest::StatusCode::ACCEPTED => {
                ShutdownReceipt::Accepted
            }
            Ok(response) => ShutdownReceipt::Failed {
                detail: format!(
                    "controlled MCP shutdown returned HTTP {}",
                    response.status().as_u16()
                ),
            },
            Err(error) => ShutdownReceipt::Failed {
                detail: format!("failed to request controlled MCP shutdown: {error}"),
            },
        }
    }
}

impl ReadinessProbe for HttpReadinessProbe {
    fn inspect(
        &self,
        definition: &ServiceDefinition,
        runtime: Option<&RuntimeState>,
    ) -> ReadinessSection {
        let response = match self.client.get(&definition.endpoint.readiness_url).send() {
            Ok(response) => response,
            Err(_) => return not_ready(),
        };
        let status = response.status().as_u16();
        if !response.status().is_success() {
            return ReadinessSection {
                status: ReadinessStatus::NotReady,
                http_status: Some(status),
                version: None,
                instance_id: None,
                pid: None,
                diagnostic_code: Some("MCP_SERVICE_START_TIMEOUT".to_owned()),
            };
        }
        let payload: ReadinessResponse = match response.json() {
            Ok(payload) => payload,
            Err(_) => {
                return ReadinessSection {
                    status: ReadinessStatus::Error,
                    http_status: Some(status),
                    version: None,
                    instance_id: None,
                    pid: None,
                    diagnostic_code: Some("MCP_SERVICE_STATE_INVALID".to_owned()),
                };
            }
        };
        let identity_matches = payload.status == "ready"
            && payload.version == definition.expected_version
            && runtime.is_some_and(|runtime| {
                runtime.service_id == definition.service_id
                    && runtime.configuration_id == definition.configuration_id
                    && payload.pid == runtime.pid
                    && payload.instance_id == runtime.instance_id
            });
        ReadinessSection {
            status: if identity_matches {
                ReadinessStatus::Ready
            } else {
                ReadinessStatus::IdentityMismatch
            },
            http_status: Some(status),
            version: Some(payload.version),
            instance_id: Some(payload.instance_id),
            pid: Some(payload.pid),
            diagnostic_code: (!identity_matches)
                .then(|| "MCP_SERVICE_IDENTITY_MISMATCH".to_owned()),
        }
    }
}

fn not_ready() -> ReadinessSection {
    ReadinessSection {
        status: ReadinessStatus::NotReady,
        http_status: None,
        version: None,
        instance_id: None,
        pid: None,
        diagnostic_code: Some("MCP_SERVICE_START_TIMEOUT".to_owned()),
    }
}
