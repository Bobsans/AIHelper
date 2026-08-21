use std::{
    collections::HashMap,
    io::{BufRead, BufReader, Read, Write},
    net::{TcpListener, TcpStream},
    path::{Path, PathBuf},
    process::{ChildStdin, Command as ProcessCommand, ExitStatus, Stdio},
    sync::{
        Arc,
        mpsc::{self, Receiver},
    },
    thread,
    time::{Duration, Instant},
};

use aihelper::secrets::{ExplicitMasterKey, NewSecret, VaultStore};
use command_group::{CommandGroup, GroupChild};
use serde_json::{Value, json};
use tempfile::TempDir;

#[cfg(windows)]
use aihelper::mcp_service::{
    model::{
        ExitKind, RuntimePhase, SCHEMA_VERSION, ServerDefinition, ServiceDefinition,
        ServiceEndpoint, TASK_SPEC_VERSION,
    },
    paths::{ServicePaths, current_user_sid},
    store::{Document, ServiceStore},
};

#[cfg(windows)]
#[test]
fn managed_service_status_routes_before_configuration_and_plugin_startup() {
    let output = ProcessCommand::new(assert_cmd::cargo::cargo_bin("ah"))
        .env("AH_CONFIG_DIR", "")
        .args(["mcp", "service", "status", "--json"])
        .output()
        .expect("managed service status should start");
    assert!(
        output.status.success(),
        "status should bypass ordinary startup: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: Value = serde_json::from_slice(&output.stdout).expect("status should emit JSON");
    assert_eq!(value["command"], "mcp.service.status");
    assert_eq!(value["schema_version"], 1);
}

#[cfg(windows)]
#[test]
fn managed_service_mutation_help_routes_before_ambient_startup() {
    for command in ["stop", "restart", "uninstall"] {
        let output = ProcessCommand::new(assert_cmd::cargo::cargo_bin("ah"))
            .env("AH_CONFIG_DIR", "")
            .args(["mcp", "service", command, "--help"])
            .output()
            .expect("managed service help should start");
        assert!(
            output.status.success(),
            "{command} help should bypass ordinary startup: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            String::from_utf8_lossy(&output.stdout).contains("Usage:"),
            "{command} should render early help"
        );
    }
}

#[cfg(windows)]
#[test]
fn managed_serve_preflight_fails_before_ambient_configuration_load() {
    let output = ProcessCommand::new(assert_cmd::cargo::cargo_bin("ah"))
        .env("AH_CONFIG_DIR", "")
        .args([
            "mcp",
            "serve",
            "--transport",
            "http",
            "--managed-config",
            "missing-definition.json",
        ])
        .output()
        .expect("managed serve preflight should start");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("ah: managed definition path has no service base directory"),
        "{stderr}"
    );
    assert!(!stderr.contains("MCP_SERVICE_STATE_INVALID"), "{stderr}");
    assert!(
        !stderr.contains("AH_CONFIG_DIR must not be empty"),
        "{stderr}"
    );
}

#[cfg(windows)]
#[test]
fn managed_serve_persists_exact_identity_and_enforces_single_instance() {
    let temp = TempDir::new().expect("temporary service directory should exist");
    let port = TcpListener::bind(("127.0.0.1", 0))
        .expect("ephemeral port should bind")
        .local_addr()
        .expect("ephemeral address should resolve")
        .port();
    let (store, definition_path, definition) = install_managed_definition(&temp, port);

    let mut first = spawn_managed_process(&definition_path);
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_millis(500))
        .build()
        .expect("readiness client should build");
    let readiness = wait_for_json(&client, &definition.endpoint.readiness_url);
    let Document::Valid(runtime) = store.read_runtime() else {
        let _ = first.kill();
        panic!("managed runtime state should be valid")
    };
    assert_eq!(runtime.phase, RuntimePhase::Ready);
    assert_eq!(readiness["instance_id"], runtime.instance_id.to_string());
    assert_eq!(readiness["pid"], runtime.pid);
    assert_eq!(runtime.configuration_id, definition.configuration_id);

    let mut second = spawn_managed_process(&definition_path);
    let second_status = wait_for_child(&mut second, Duration::from_secs(3));
    assert!(
        second_status.success(),
        "duplicate managed process should exit successfully"
    );
    let repeated = wait_for_json(&client, &definition.endpoint.readiness_url);
    assert_eq!(repeated["instance_id"], readiness["instance_id"]);

    let response = client
        .post(format!("http://127.0.0.1:{port}/control/shutdown"))
        .json(&json!({"instance_id": runtime.instance_id}))
        .send()
        .expect("managed shutdown should respond");
    assert_eq!(response.status(), reqwest::StatusCode::ACCEPTED);
    let first_status = wait_for_child(&mut first, Duration::from_secs(8));
    assert!(
        first_status.success(),
        "managed process should stop cleanly"
    );
    let Document::Valid(stopped) = store.read_runtime() else {
        panic!("stopped runtime state should remain valid")
    };
    assert_eq!(stopped.phase, RuntimePhase::Stopped);
    assert_eq!(
        stopped
            .last_exit
            .as_ref()
            .map(|exit| (exit.exit_code, exit.diagnostic_code.as_deref())),
        Some((0, None))
    );

    let mut restarted = spawn_managed_process(&definition_path);
    let restarted_readiness = wait_for_json(&client, &definition.endpoint.readiness_url);
    assert_ne!(
        restarted_readiness["instance_id"], readiness["instance_id"],
        "a restarted managed runner must receive a new instance identity"
    );
    let Document::Valid(restarted_runtime) = store.read_runtime() else {
        let _ = restarted.kill();
        panic!("restarted runtime state should be valid")
    };
    let response = client
        .post(format!("http://127.0.0.1:{port}/control/shutdown"))
        .json(&json!({"instance_id": restarted_runtime.instance_id}))
        .send()
        .expect("restarted managed shutdown should respond");
    assert_eq!(response.status(), reqwest::StatusCode::ACCEPTED);
    assert!(wait_for_child(&mut restarted, Duration::from_secs(8)).success());
}

#[cfg(windows)]
#[test]
fn managed_serve_persists_nonzero_failure_for_port_conflict() {
    let temp = TempDir::new().expect("temporary service directory should exist");
    let occupied = TcpListener::bind(("127.0.0.1", 0)).expect("ephemeral port should bind");
    let port = occupied
        .local_addr()
        .expect("ephemeral address should resolve")
        .port();
    let (store, definition_path, _) = install_managed_definition(&temp, port);

    let mut child = spawn_managed_process(&definition_path);
    let status = wait_for_child(&mut child, Duration::from_secs(8));

    assert_eq!(status.code(), Some(1), "fatal managed exit must be nonzero");
    let Document::Valid(runtime) = store.read_runtime() else {
        panic!("failed runtime state should remain valid")
    };
    assert_eq!(runtime.phase, RuntimePhase::Failed);
    let exit = runtime
        .last_exit
        .as_ref()
        .expect("failed runtime should contain last_exit");
    assert_eq!(exit.kind, ExitKind::RuntimeFailure);
    assert_ne!(exit.exit_code, 0);
    assert_eq!(exit.diagnostic_code.as_deref(), Some("MCP_SERVER_FAILED"));
    drop(occupied);
}

#[cfg(windows)]
fn install_managed_definition(
    temp: &TempDir,
    port: u16,
) -> (ServiceStore, PathBuf, ServiceDefinition) {
    let paths = ServicePaths::from_base(temp.path().join("managed-mcp"))
        .expect("service paths should resolve");
    let store = ServiceStore::new(paths.clone());
    let configuration_id = uuid::Uuid::new_v4();
    let definition = ServiceDefinition {
        schema_version: SCHEMA_VERSION,
        task_spec_version: TASK_SPEC_VERSION,
        service_id: uuid::Uuid::new_v4(),
        configuration_id,
        user_sid: current_user_sid().expect("current SID should resolve"),
        executable_path: assert_cmd::cargo::cargo_bin("ah"),
        working_directory: temp.path().to_path_buf(),
        config_directory: temp.path().join("config"),
        runtime_state_path: paths.runtime.clone(),
        instance_lock_path: paths.instance_lock.clone(),
        expected_version: env!("CARGO_PKG_VERSION").to_owned(),
        endpoint: ServiceEndpoint::loopback(port).expect("endpoint should be valid"),
        server: ServerDefinition {
            limit: None,
            max_active: 4,
            default_timeout_ms: 5_000,
        },
    };
    let definition_path = paths.definition(configuration_id);
    store
        .write_immutable_definition(&definition_path, &definition)
        .expect("managed definition should be written");
    (store, definition_path, definition)
}

#[cfg(windows)]
fn spawn_managed_process(definition_path: &Path) -> std::process::Child {
    ProcessCommand::new(assert_cmd::cargo::cargo_bin("ah"))
        .args(["mcp", "serve", "--transport", "http", "--managed-config"])
        .arg(definition_path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("managed MCP process should start")
}

#[cfg(windows)]
fn wait_for_json(client: &reqwest::blocking::Client, url: &str) -> Value {
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        if let Ok(response) = client.get(url).send()
            && response.status().is_success()
        {
            return response.json().expect("readiness should be JSON");
        }
        assert!(
            Instant::now() < deadline,
            "managed MCP endpoint did not become ready"
        );
        thread::sleep(Duration::from_millis(25));
    }
}

#[cfg(windows)]
fn wait_for_child(child: &mut std::process::Child, timeout: Duration) -> ExitStatus {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = child.try_wait().expect("child status should be readable") {
            return status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("managed MCP child did not exit before the deadline");
        }
        thread::sleep(Duration::from_millis(25));
    }
}

struct McpProcess {
    child: GroupChild,
    stdin: Option<ChildStdin>,
    responses: Receiver<String>,
    pending: HashMap<String, Value>,
}

impl McpProcess {
    fn start(config_dir: &TempDir) -> Self {
        Self::start_with_args(config_dir, &[])
    }

    fn start_with_args(config_dir: &TempDir, extra_args: &[&str]) -> Self {
        Self::start_with_options(config_dir, extra_args, None)
    }

    fn start_with_master_key(config_dir: &TempDir, master_key: &str) -> Self {
        Self::start_with_options(config_dir, &[], Some(master_key))
    }

    fn start_with_options(
        config_dir: &TempDir,
        extra_args: &[&str],
        master_key: Option<&str>,
    ) -> Self {
        let mut command = ProcessCommand::new(assert_cmd::cargo::cargo_bin("ah"));
        command
            .env("AH_CONFIG_DIR", config_dir.path())
            .args(["mcp", "serve", "--default-timeout-ms", "5000"])
            .args(extra_args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(master_key) = master_key {
            command
                .env("APPDATA", "")
                .env("AH_VAULT_MASTER_KEY", master_key);
        }
        let mut child = command.group_spawn().expect("MCP server should start");
        let stdin = child
            .inner()
            .stdin
            .take()
            .expect("MCP stdin should be piped");
        let stdout = child
            .inner()
            .stdout
            .take()
            .expect("MCP stdout should be piped");
        let (sender, responses) = mpsc::channel();
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else {
                    break;
                };
                if sender.send(line).is_err() {
                    break;
                }
            }
        });
        Self {
            child,
            stdin: Some(stdin),
            responses,
            pending: HashMap::new(),
        }
    }

    fn send(&mut self, message: Value) {
        let stdin = self.stdin.as_mut().expect("MCP stdin should be open");
        writeln!(stdin, "{message}").expect("MCP message should be written");
        stdin.flush().expect("MCP message should be flushed");
    }

    fn response(&mut self) -> Value {
        let line = self
            .responses
            .recv_timeout(Duration::from_secs(10))
            .expect("MCP server should respond");
        serde_json::from_str(&line).expect("MCP stdout must contain JSON-RPC only")
    }

    fn response_for(&mut self, id: u64) -> Value {
        let key = id.to_string();
        if let Some(response) = self.pending.remove(&key) {
            return response;
        }
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            let line = self
                .responses
                .recv_timeout(remaining)
                .expect("MCP server should respond before the deadline");
            let response: Value =
                serde_json::from_str(&line).expect("MCP stdout must contain JSON-RPC only");
            let Some(response_id) = response.get("id") else {
                continue;
            };
            let response_key = response_id.to_string();
            if response_key == key {
                return response;
            }
            self.pending.insert(response_key, response);
        }
    }

    fn stop(mut self) {
        self.shutdown(true);
    }

    fn shutdown(&mut self, panic_on_timeout: bool) {
        drop(self.stdin.take());
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            match self.child.try_wait() {
                Ok(Some(status)) => {
                    if panic_on_timeout && !status.success() {
                        panic!("MCP server should exit cleanly, got {status}");
                    }
                    return;
                }
                Ok(None) => {}
                Err(error) if panic_on_timeout => {
                    panic!("MCP process status should be readable: {error}")
                }
                Err(_) => return,
            }
            if Instant::now() >= deadline {
                if let Err(error) = self.child.kill()
                    && error.kind() != std::io::ErrorKind::InvalidInput
                {
                    if panic_on_timeout {
                        panic!("MCP process should stop: {error}");
                    }
                    return;
                }
                let _ = self.child.wait();
                if panic_on_timeout {
                    panic!("MCP server did not stop after stdin closed");
                }
                return;
            }
            thread::sleep(Duration::from_millis(10));
        }
    }
}

impl Drop for McpProcess {
    fn drop(&mut self) {
        self.shutdown(false);
    }
}

#[test]
fn stdio_secrets_list_tools_call_redacts_values() {
    const MASTER_KEY: &str = "3333333333333333333333333333333333333333333333333333333333333333";
    let config_dir = TempDir::new().expect("temporary config dir should be created");
    let secret_value = "mcp-tools-call-secret";
    let store = VaultStore::at(
        config_dir.path(),
        Arc::new(ExplicitMasterKey::parse(MASTER_KEY.to_owned()).unwrap()),
    );
    store.initialize().unwrap();
    store
        .put(
            NewSecret::postgres("billing", "Billing", secret_value)
                .with_description(Some("Production billing database".to_owned())),
        )
        .unwrap();
    let mut server = McpProcess::start_with_master_key(&config_dir, MASTER_KEY);

    server.send(json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2025-11-25",
            "capabilities": {},
            "clientInfo": {"name": "aihelper-test", "version": "1.0.0"}
        }
    }));
    assert_eq!(server.response()["id"], 1);
    server.send(json!({
        "jsonrpc": "2.0",
        "method": "notifications/initialized"
    }));
    server.send(json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": {
            "name": "ah.secrets.list",
            "arguments": {"kind": "postgres"}
        }
    }));
    let response = server.response();

    assert_eq!(response["id"], 2);
    assert_eq!(response["result"]["isError"], false);
    assert_eq!(
        response["result"]["structuredContent"],
        json!({"secrets": [{
            "id": "billing",
            "kind": "postgres",
            "label": "Billing",
            "description": "Production billing database"
        }]})
    );
    assert!(!response.to_string().contains(secret_value));
    server.stop();
}

struct HttpMcpProcess {
    child: GroupChild,
    url: String,
    readiness_url: String,
    shutdown_url: String,
    origin: String,
}

impl HttpMcpProcess {
    fn start(config_dir: &TempDir) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("ephemeral port should bind");
        let port = listener
            .local_addr()
            .expect("ephemeral address should be available")
            .port();
        drop(listener);
        let port = port.to_string();
        let mut command = ProcessCommand::new(assert_cmd::cargo::cargo_bin("ah"));
        command
            .env("AH_CONFIG_DIR", config_dir.path())
            .args([
                "mcp",
                "serve",
                "--transport",
                "http",
                "--port",
                &port,
                "--default-timeout-ms",
                "5000",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        let child = command.group_spawn().expect("HTTP MCP server should start");
        let origin = format!("http://127.0.0.1:{port}");
        Self {
            child,
            url: format!("{origin}/mcp"),
            readiness_url: format!("{origin}/health/ready"),
            shutdown_url: format!("{origin}/control/shutdown"),
            origin,
        }
    }

    fn wait_for_exit(&mut self, timeout: Duration) -> ExitStatus {
        let deadline = Instant::now() + timeout;
        loop {
            match self.child.try_wait() {
                Ok(Some(status)) => return status,
                Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(10)),
                Ok(None) => {
                    let _ = self.child.kill();
                    let _ = self.child.wait();
                    panic!("HTTP MCP server did not stop before the deadline");
                }
                Err(error) => panic!("HTTP MCP process status should be readable: {error}"),
            }
        }
    }

    fn read_stderr(&mut self) -> String {
        let mut output = String::new();
        self.child
            .inner()
            .stderr
            .take()
            .expect("HTTP MCP stderr should be piped")
            .read_to_string(&mut output)
            .expect("HTTP MCP stderr should be readable");
        output
    }

    fn stop(mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for HttpMcpProcess {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn read_readiness(process: &HttpMcpProcess) -> (reqwest::header::HeaderMap, Value) {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .expect("HTTP client should build");
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match client.get(&process.readiness_url).send() {
            Ok(response) if response.status().is_success() => {
                let headers = response.headers().clone();
                let body = response.json().expect("readiness body should be JSON");
                return (headers, body);
            }
            _ if Instant::now() < deadline => thread::sleep(Duration::from_millis(25)),
            Ok(response) => panic!("HTTP readiness failed: {}", response.status()),
            Err(error) => panic!("HTTP MCP server did not become ready: {error}"),
        }
    }
}

struct HttpMcpClient {
    client: reqwest::blocking::Client,
    url: String,
    session_id: String,
}

impl HttpMcpClient {
    fn connect(url: &str, client_name: &str) -> Self {
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .expect("HTTP client should build");
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let response = client
                .post(url)
                .header("Accept", "application/json, text/event-stream")
                .header("Content-Type", "application/json")
                .json(&json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "method": "initialize",
                    "params": {
                        "protocolVersion": "2025-11-25",
                        "capabilities": {},
                        "clientInfo": {"name": client_name, "version": "1.0.0"}
                    }
                }))
                .send();
            match response {
                Ok(response) if response.status().is_success() => {
                    let session_id = response
                        .headers()
                        .get("mcp-session-id")
                        .expect("initialize must return an MCP session id")
                        .to_str()
                        .expect("session id must be text")
                        .to_owned();
                    let initialized = parse_http_mcp_response(response);
                    assert_eq!(initialized["id"], 1);
                    let this = Self {
                        client,
                        url: url.to_owned(),
                        session_id,
                    };
                    this.notify(json!({
                        "jsonrpc": "2.0",
                        "method": "notifications/initialized"
                    }));
                    return this;
                }
                _ if Instant::now() < deadline => thread::sleep(Duration::from_millis(25)),
                Ok(response) => panic!("HTTP MCP initialize failed: {}", response.status()),
                Err(error) => panic!("HTTP MCP server did not start: {error}"),
            }
        }
    }

    fn call(&self, message: Value) -> Value {
        let response = self
            .request(message)
            .send()
            .expect("HTTP MCP request should succeed");
        assert!(
            response.status().is_success(),
            "HTTP MCP call failed: {response:?}"
        );
        parse_http_mcp_response(response)
    }

    fn notify(&self, message: Value) {
        let response = self
            .request(message)
            .send()
            .expect("HTTP MCP notification should succeed");
        assert!(response.status().is_success());
    }

    fn request(&self, message: Value) -> reqwest::blocking::RequestBuilder {
        self.client
            .post(&self.url)
            .header("Accept", "application/json, text/event-stream")
            .header("Content-Type", "application/json")
            .header("MCP-Session-Id", &self.session_id)
            .header("MCP-Protocol-Version", "2025-11-25")
            .json(&message)
    }
}

fn parse_http_mcp_response(response: reqwest::blocking::Response) -> Value {
    let body = response.text().expect("HTTP MCP response body should read");
    if let Ok(value) = serde_json::from_str(&body) {
        return value;
    }
    body.lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .find_map(|data| serde_json::from_str(data).ok())
        .unwrap_or_else(|| panic!("HTTP MCP response did not contain JSON-RPC data: {body}"))
}

struct ReleaseMarker {
    path: PathBuf,
}

impl ReleaseMarker {
    fn new(path: PathBuf) -> Self {
        Self { path }
    }

    fn release(&self) {
        std::fs::write(&self.path, b"release").expect("release marker should be written");
    }
}

impl Drop for ReleaseMarker {
    fn drop(&mut self) {
        let _ = std::fs::write(&self.path, b"release");
    }
}

#[test]
fn stdio_server_lists_and_calls_typed_ctx_tool() {
    let config_dir = TempDir::new().expect("temporary config dir should be created");
    let workspace = TempDir::new().expect("temporary workspace should be created");
    std::fs::write(workspace.path().join("sample.rs"), "fn sample() {}\n")
        .expect("sample file should be written");
    let mut server = McpProcess::start(&config_dir);

    server.send(json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2025-11-25",
            "capabilities": {},
            "clientInfo": {"name": "aihelper-test", "version": "1.0.0"}
        }
    }));
    let initialized = server.response();
    assert_eq!(initialized["id"], 1);
    assert_eq!(
        initialized["result"]["capabilities"]["tools"]["listChanged"],
        true
    );
    assert!(
        initialized["result"]["capabilities"]
            .get("resources")
            .is_none()
    );

    server.send(json!({
        "jsonrpc": "2.0",
        "method": "notifications/initialized"
    }));
    server.send(json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/list",
        "params": {}
    }));
    let listed = server.response();
    assert_eq!(listed["id"], 2);
    let tools = listed["result"]["tools"]
        .as_array()
        .expect("tools/list should return an array");
    for name in [
        "ah.ai.info",
        "ah.plugins.list",
        "ah.plugins.enable",
        "ah.plugins.disable",
        "ah.plugins.reset",
        "ah.ctx.pack",
        "ah.ctx.symbols",
        "ah.ctx.changed",
        "ah.file.read",
        "ah.file.head",
        "ah.file.tail",
        "ah.file.stat",
        "ah.file.tree",
        "ah.git.status",
        "ah.git.tags",
        "ah.git.tag.create",
        "ah.git.remotes",
        "ah.git.changed",
        "ah.git.diff",
        "ah.git.blame",
        "ah.git.commit-info",
        "ah.http.request",
        "ah.http.get",
        "ah.http.post",
        "ah.http.put",
        "ah.http.patch",
        "ah.http.delete",
        "ah.http.replay",
        "ah.http.assert",
        "ah.http.run",
        "ah.project.detect",
        "ah.project.commands",
        "ah.project.version",
        "ah.run.check",
        "ah.search.text",
        "ah.search.files",
        "ah.task.save",
        "ah.task.run",
        "ah.task.list",
        "ah.job.start",
        "ah.job.status",
        "ah.job.result",
        "ah.job.cancel",
    ] {
        assert!(
            tools.iter().any(|tool| tool["name"] == name),
            "missing MCP tool {name}"
        );
    }
    assert!(!tools.iter().any(|tool| tool["name"] == "ah.mcp.serve"));
    let ctx_symbols = tools
        .iter()
        .find(|tool| tool["name"] == "ah.ctx.symbols")
        .expect("ctx.symbols tool should exist");
    assert!(ctx_symbols["inputSchema"]["properties"]["context"].is_object());
    assert_eq!(ctx_symbols["_meta"]["dev.aihelper/risk"]["level"], "low");
    assert_eq!(ctx_symbols["execution"]["taskSupport"], "forbidden");
    let run_check = tools
        .iter()
        .find(|tool| tool["name"] == "ah.run.check")
        .expect("run.check tool should exist");
    assert_eq!(run_check["_meta"]["dev.aihelper/risk"]["level"], "critical");
    assert_eq!(run_check["annotations"]["destructiveHint"], true);

    server.send(json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "tools/call",
        "params": {
            "name": "ah.ctx.symbols",
            "arguments": {
                "path": "sample.rs",
                "preset": "summary",
                "context": {
                    "cwd": workspace.path().to_string_lossy(),
                    "timeout_ms": 2000
                }
            }
        }
    }));
    let called = server.response();
    assert_eq!(called["id"], 3);
    assert_eq!(called["result"]["isError"], false);
    assert_eq!(
        called["result"]["structuredContent"]["command"],
        "ctx.symbols"
    );
    assert_eq!(called["result"]["structuredContent"]["symbol_count"], 1);
    assert!(
        called["result"]["content"][0]["text"]
            .as_str()
            .expect("tool content should be text")
            .starts_with('{')
    );

    server.send(json!({
        "jsonrpc": "2.0",
        "id": 4,
        "method": "tools/call",
        "params": {
            "name": "ah.ctx.symbols",
            "arguments": {
                "path": "sample.rs",
                "context": {
                    "cwd": workspace.path().to_string_lossy(),
                    "timeout_ms": 0
                }
            }
        }
    }));
    let invalid_context = server.response();
    assert_eq!(invalid_context["id"], 4);
    assert_eq!(invalid_context["result"]["isError"], true);

    server.send(json!({
        "jsonrpc": "2.0",
        "id": 5,
        "method": "tools/call",
        "params": {"name": "ah.missing.tool", "arguments": {}}
    }));
    let unknown = server.response();
    assert_eq!(unknown["id"], 5);
    assert!(unknown["error"].is_object());

    server.stop();

    let records = log_records(&config_dir);
    let tool_event = records
        .iter()
        .find(|record| {
            record["event"] == "command.completed"
                && record["transport"] == "mcp"
                && record["command"] == "ctx.symbols"
        })
        .expect("MCP tool call should be logged");
    assert_eq!(tool_event["tool"], "ah.ctx.symbols");
    assert_eq!(tool_event["status"], "success");
    assert_eq!(tool_event["parameters"]["path"], "sample.rs");
    assert!(tool_event["request_id"].as_str().is_some());
    assert_eq!(
        records
            .iter()
            .filter(|record| {
                record["event"] == "command.completed"
                    && record["transport"] == "mcp"
                    && record["command"] == "ctx.symbols"
                    && record["status"] == "success"
            })
            .count(),
        1
    );
    let invalid_context_event = records
        .iter()
        .find(|record| {
            record["event"] == "command.completed"
                && record["transport"] == "mcp"
                && record["command"] == "ctx.symbols"
                && record["status"] == "error"
        })
        .expect("invalid MCP context should be logged once");
    assert_eq!(
        invalid_context_event["diagnostic"]["code"],
        "INVALID_CONTEXT"
    );
    assert_eq!(
        records
            .iter()
            .filter(|record| {
                record["transport"] == "mcp"
                    && record["command"] == "ctx.symbols"
                    && record["status"] == "error"
            })
            .count(),
        1
    );
    let unknown_event = records
        .iter()
        .find(|record| {
            record["event"] == "command.completed"
                && record["transport"] == "mcp"
                && record["tool"] == "ah.missing.tool"
        })
        .expect("unknown MCP tool should be logged");
    assert_eq!(unknown_event["status"], "error");
    let serve_event = records
        .iter()
        .find(|record| {
            record["event"] == "command.completed"
                && record["transport"] == "cli"
                && record["command"] == "mcp.serve"
        })
        .expect("mcp.serve completion should be logged");
    assert_eq!(serve_event["status"], "success");
}

#[test]
fn stdio_job_tools_return_repeatable_terminal_result_without_waiting_on_start() {
    let config_dir = TempDir::new().expect("temporary config dir should be created");
    let workspace = TempDir::new().expect("temporary workspace should be created");
    std::fs::write(workspace.path().join("sample.txt"), "sample")
        .expect("sample file should be written");
    let mut server = McpProcess::start(&config_dir);

    server.send(json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2025-11-25",
            "capabilities": {},
            "clientInfo": {"name": "aihelper-job-test", "version": "1.0.0"}
        }
    }));
    assert_eq!(server.response_for(1)["id"], 1);
    server.send(json!({
        "jsonrpc": "2.0",
        "method": "notifications/initialized"
    }));
    server.send(json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": {
            "name": "ah.job.start",
            "arguments": {
                "tool": "ah.file.stat",
                "arguments": {
                    "path": "sample.txt",
                    "context": {
                        "cwd": workspace.path().to_string_lossy(),
                        "timeout_ms": 2000
                    }
                }
            }
        }
    }));
    let started = server.response_for(2);
    assert_eq!(started["result"]["isError"], false);
    assert_eq!(started["result"]["structuredContent"]["status"], "running");
    let job_id = started["result"]["structuredContent"]["job_id"]
        .as_str()
        .expect("job id must be returned")
        .to_owned();

    let terminal = (3..103)
        .find_map(|id| {
            server.send(json!({
                "jsonrpc": "2.0",
                "id": id,
                "method": "tools/call",
                "params": {
                    "name": "ah.job.result",
                    "arguments": {"job_id": job_id.clone()}
                }
            }));
            let response = server.response_for(id);
            if response["result"]["structuredContent"]["ready"] == true {
                Some(response)
            } else {
                thread::sleep(Duration::from_millis(10));
                None
            }
        })
        .expect("job should complete");
    assert_eq!(
        terminal["result"]["structuredContent"]["status"],
        "succeeded"
    );
    assert_eq!(
        terminal["result"]["structuredContent"]["response"]["success"],
        true
    );
    assert_eq!(
        terminal["result"]["structuredContent"]["response"]["data"]["command"],
        "file.stat"
    );

    server.send(json!({
        "jsonrpc": "2.0",
        "id": 103,
        "method": "tools/call",
        "params": {
            "name": "ah.job.result",
            "arguments": {"job_id": job_id}
        }
    }));
    let repeated = server.response_for(103);
    assert_eq!(
        repeated["result"]["structuredContent"],
        terminal["result"]["structuredContent"]
    );
    server.stop();

    let records = log_records(&config_dir);
    let target_events = records
        .iter()
        .filter(|record| {
            record["event"] == "command.completed"
                && record["transport"] == "mcp"
                && record["command"] == "file.stat"
        })
        .collect::<Vec<_>>();
    assert_eq!(target_events.len(), 1);
    assert_eq!(target_events[0]["tool"], "ah.file.stat");
    assert_eq!(target_events[0]["status"], "success");
    assert_eq!(target_events[0]["parameters"]["path"], "sample.txt");
    assert_eq!(target_events[0]["queue_wait_ms"], 0);
    assert_eq!(target_events[0]["job_id"], job_id);
}

#[test]
fn http_sessions_share_jobs_but_keep_working_directories_explicit() {
    let config_dir = TempDir::new().expect("temporary config dir should be created");
    let first_workspace = TempDir::new().expect("first workspace should be created");
    let second_workspace = TempDir::new().expect("second workspace should be created");
    std::fs::write(first_workspace.path().join("first.txt"), "first")
        .expect("first sample should be written");
    std::fs::write(second_workspace.path().join("second.txt"), "second")
        .expect("second sample should be written");
    let process = HttpMcpProcess::start(&config_dir);
    let first = HttpMcpClient::connect(&process.url, "first-client");
    let second = HttpMcpClient::connect(&process.url, "second-client");

    let first_direct = first.call(json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": {
            "name": "ah.file.stat",
            "arguments": {
                "path": "first.txt",
                "context": {"cwd": first_workspace.path().to_string_lossy()}
            }
        }
    }));
    let second_direct = second.call(json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": {
            "name": "ah.file.stat",
            "arguments": {
                "path": "second.txt",
                "context": {"cwd": second_workspace.path().to_string_lossy()}
            }
        }
    }));
    assert_eq!(first_direct["result"]["isError"], false);
    assert_eq!(second_direct["result"]["isError"], false);

    let started = first.call(json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "tools/call",
        "params": {
            "name": "ah.job.start",
            "arguments": {
                "tool": "ah.file.stat",
                "arguments": {
                    "path": "first.txt",
                    "context": {"cwd": first_workspace.path().to_string_lossy()}
                }
            }
        }
    }));
    let job_id = started["result"]["structuredContent"]["job_id"]
        .as_str()
        .expect("HTTP job id should be returned")
        .to_owned();
    let terminal = (3..103)
        .find_map(|id| {
            let response = second.call(json!({
                "jsonrpc": "2.0",
                "id": id,
                "method": "tools/call",
                "params": {
                    "name": "ah.job.result",
                    "arguments": {"job_id": job_id.clone()}
                }
            }));
            if response["result"]["structuredContent"]["ready"] == true {
                Some(response)
            } else {
                thread::sleep(Duration::from_millis(10));
                None
            }
        })
        .expect("second HTTP session should observe first session's job");
    assert_eq!(
        terminal["result"]["structuredContent"]["response"]["data"]["command"],
        "file.stat"
    );

    let missing_cwd = second.call(json!({
        "jsonrpc": "2.0",
        "id": 200,
        "method": "tools/call",
        "params": {
            "name": "ah.file.stat",
            "arguments": {"path": "second.txt"}
        }
    }));
    assert_eq!(missing_cwd["result"]["isError"], true);
    assert_eq!(
        missing_cwd["result"]["_meta"]["dev.aihelper/diagnostic"]["code"],
        "INVALID_CONTEXT"
    );
    process.stop();
}

#[test]
fn http_readiness_identifies_the_running_process_without_a_session() {
    let first_config = TempDir::new().expect("temporary config dir should be created");
    let first = HttpMcpProcess::start(&first_config);
    let (headers, body) = read_readiness(&first);

    assert_eq!(
        headers
            .get(reqwest::header::CONTENT_TYPE)
            .expect("readiness content type should exist"),
        "application/json"
    );
    assert!(headers.get("access-control-allow-origin").is_none());
    let mut fields = body
        .as_object()
        .expect("readiness body should be an object")
        .keys()
        .map(String::as_str)
        .collect::<Vec<_>>();
    fields.sort_unstable();
    assert_eq!(fields, ["instance_id", "pid", "status", "version"]);
    assert_eq!(body["status"], "ready");
    assert_eq!(body["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(body["pid"], u64::from(first.child.id()));
    let first_instance = body["instance_id"]
        .as_str()
        .expect("instance identity should be text")
        .to_owned();
    assert_eq!(first_instance.len(), 36);
    assert_eq!(first_instance.as_bytes()[14], b'4');

    let (_, repeated) = read_readiness(&first);
    assert_eq!(repeated, body);

    let second_config = TempDir::new().expect("temporary config dir should be created");
    let second = HttpMcpProcess::start(&second_config);
    let (_, second_body) = read_readiness(&second);
    assert_ne!(second_body["instance_id"], first_instance);

    second.stop();
    first.stop();
}

#[test]
fn http_control_shutdown_validates_identity_and_exits_cleanly() {
    let config_dir = TempDir::new().expect("temporary config dir should be created");
    let mut process = HttpMcpProcess::start(&config_dir);
    let (_, readiness) = read_readiness(&process);
    let instance_id = readiness["instance_id"]
        .as_str()
        .expect("instance identity should be text");
    let stale_instance_id = if instance_id == "00000000-0000-4000-8000-000000000000" {
        "10000000-0000-4000-8000-000000000000"
    } else {
        "00000000-0000-4000-8000-000000000000"
    };
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .expect("HTTP client should build");

    let unsupported_media_type = client
        .post(&process.shutdown_url)
        .body(format!(r#"{{"instance_id":"{instance_id}"}}"#))
        .send()
        .expect("shutdown request without JSON content type should receive a response");
    assert_eq!(
        unsupported_media_type.status(),
        reqwest::StatusCode::UNSUPPORTED_MEDIA_TYPE
    );
    let unsupported_body: Value = unsupported_media_type
        .json()
        .expect("unsupported media type response should be JSON");
    assert_eq!(unsupported_body["error"]["code"], "UNSUPPORTED_MEDIA_TYPE");

    let malformed = client
        .post(&process.shutdown_url)
        .json(&json!({"instance_id": "not-a-uuid"}))
        .send()
        .expect("malformed shutdown request should receive a response");
    assert_eq!(malformed.status(), reqwest::StatusCode::BAD_REQUEST);
    let malformed_body: Value = malformed
        .json()
        .expect("malformed shutdown response should be JSON");
    assert_eq!(malformed_body["error"]["code"], "INVALID_SHUTDOWN_REQUEST");

    let unknown_field = client
        .post(&process.shutdown_url)
        .json(&json!({"instance_id": instance_id, "force": true}))
        .send()
        .expect("shutdown request with an unknown field should receive a response");
    assert_eq!(unknown_field.status(), reqwest::StatusCode::BAD_REQUEST);
    let unknown_field_body: Value = unknown_field
        .json()
        .expect("unknown field response should be JSON");
    assert_eq!(
        unknown_field_body["error"]["code"],
        "INVALID_SHUTDOWN_REQUEST"
    );

    let mismatch = client
        .post(&process.shutdown_url)
        .json(&json!({"instance_id": stale_instance_id}))
        .send()
        .expect("stale shutdown request should receive a response");
    assert_eq!(mismatch.status(), reqwest::StatusCode::CONFLICT);
    let mismatch_body: Value = mismatch
        .json()
        .expect("identity mismatch response should be JSON");
    assert_eq!(mismatch_body["error"]["code"], "INSTANCE_ID_MISMATCH");
    assert!(
        !mismatch_body.to_string().contains(instance_id),
        "identity mismatch response must not reveal the current instance identity"
    );

    let hostile_host = client
        .post(&process.shutdown_url)
        .header("Host", "evil.example")
        .json(&json!({"instance_id": instance_id}))
        .send()
        .expect("hostile shutdown Host request should receive a response");
    assert_eq!(hostile_host.status(), reqwest::StatusCode::FORBIDDEN);
    let hostile_host_body: Value = hostile_host
        .json()
        .expect("hostile Host response should be JSON");
    assert_eq!(hostile_host_body["error"]["code"], "LOCAL_REQUEST_REJECTED");

    let hostile_origin = client
        .post(&process.shutdown_url)
        .header("Origin", "http://evil.example")
        .json(&json!({"instance_id": instance_id}))
        .send()
        .expect("hostile shutdown Origin request should receive a response");
    assert_eq!(hostile_origin.status(), reqwest::StatusCode::FORBIDDEN);
    let hostile_origin_body: Value = hostile_origin
        .json()
        .expect("hostile Origin response should be JSON");
    assert_eq!(
        hostile_origin_body["error"]["code"],
        "LOCAL_REQUEST_REJECTED"
    );

    let (_, still_ready) = read_readiness(&process);
    assert_eq!(still_ready, readiness);

    let accepted = client
        .post(&process.shutdown_url)
        .header("Origin", &process.origin)
        .json(&json!({"instance_id": instance_id}))
        .send()
        .expect("matching shutdown request should receive a response");
    assert_eq!(accepted.status(), reqwest::StatusCode::ACCEPTED);
    assert!(
        accepted
            .headers()
            .get("access-control-allow-origin")
            .is_none()
    );
    let accepted_body: Value = accepted
        .json()
        .expect("accepted shutdown response should be JSON");
    assert_eq!(
        accepted_body,
        json!({"status": "shutting_down", "instance_id": instance_id})
    );

    let exit = process.wait_for_exit(Duration::from_secs(7));
    assert!(exit.success(), "HTTP MCP server should exit cleanly");
}

#[test]
fn http_control_shutdown_cancels_active_job_and_stops_new_work() {
    let config_dir = TempDir::new().expect("temporary config dir should be created");
    let workspace = TempDir::new().expect("temporary workspace should be created");
    let ready = workspace.path().join("shutdown-job.ready");
    let release = workspace.path().join("shutdown-job.release");
    let completed = workspace.path().join("shutdown-job.completed");
    let must_not_start = workspace.path().join("shutdown-new-work.txt");
    let release_marker = ReleaseMarker::new(release.clone());
    let mut process = HttpMcpProcess::start(&config_dir);
    let (_, readiness) = read_readiness(&process);
    let instance_id = readiness["instance_id"]
        .as_str()
        .expect("instance identity should be text")
        .to_owned();
    let client = HttpMcpClient::connect(&process.url, "shutdown-job-client");

    let started = client.call(json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": {
            "name": "ah.job.start",
            "arguments": {
                "tool": "ah.run.check",
                "arguments": {
                    "command": shutdown_blocker_command(&ready, &release, &completed),
                    "timeout_secs": 60,
                    "context": {
                        "cwd": workspace.path().to_string_lossy(),
                        "timeout_ms": 60_000
                    }
                }
            }
        }
    }));
    assert_eq!(started["result"]["isError"], false);
    let job_id = started["result"]["structuredContent"]["job_id"]
        .as_str()
        .expect("active job id should be returned")
        .to_owned();
    wait_for_path(&ready);

    let active = client.call(json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "tools/call",
        "params": {
            "name": "ah.job.status",
            "arguments": {"job_id": job_id.clone()}
        }
    }));
    assert_eq!(active["result"]["isError"], false);
    assert_eq!(active["result"]["structuredContent"]["status"], "running");
    assert_eq!(active["result"]["structuredContent"]["draining"], false);

    let accepted = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .expect("control client should build")
        .post(&process.shutdown_url)
        .header("Origin", &process.origin)
        .json(&json!({"instance_id": instance_id.clone()}))
        .send()
        .expect("matching shutdown request should receive a response");
    assert_eq!(accepted.status(), reqwest::StatusCode::ACCEPTED);
    let accepted_body: Value = accepted
        .json()
        .expect("accepted shutdown response should be JSON");
    assert_eq!(
        accepted_body,
        json!({"status": "shutting_down", "instance_id": instance_id})
    );

    let post_shutdown = client
        .request(json!({
            "jsonrpc": "2.0",
            "id": 4,
            "method": "tools/call",
            "params": {
                "name": "ah.run.check",
                "arguments": {
                    "command": write_marker_command(&must_not_start),
                    "context": {
                        "cwd": workspace.path().to_string_lossy(),
                        "timeout_ms": 1000
                    }
                }
            }
        }))
        .timeout(Duration::from_secs(1))
        .send();
    if let Ok(response) = post_shutdown
        && response.status().is_success()
    {
        let rejected = parse_http_mcp_response(response);
        assert_ne!(
            rejected["result"]["isError"], false,
            "new work must not succeed after shutdown acceptance"
        );
    }
    drop(client);

    let exit = process.wait_for_exit(Duration::from_secs(7));
    assert!(
        exit.success(),
        "HTTP MCP server should exit cleanly after cancelling the active job"
    );
    assert!(
        !must_not_start.exists(),
        "work submitted after shutdown acceptance must never start"
    );

    release_marker.release();
    thread::sleep(Duration::from_secs(1));
    assert!(
        !completed.exists(),
        "the ah.run.check child process survived MCP shutdown"
    );

    let records = log_records(&config_dir);
    let job_events = records
        .iter()
        .filter(|record| {
            record["event"] == "command.completed"
                && record["transport"] == "mcp"
                && record["command"] == "run.check"
                && record["job_id"] == job_id
        })
        .collect::<Vec<_>>();
    assert_eq!(job_events.len(), 1);
    assert_eq!(job_events[0]["status"], "error");
    assert_eq!(job_events[0]["diagnostic"]["code"], "CANCELLED");
    let serve_event = records
        .iter()
        .find(|record| {
            record["event"] == "command.completed"
                && record["transport"] == "cli"
                && record["command"] == "mcp.serve"
        })
        .expect("mcp.serve completion should be logged");
    assert_eq!(serve_event["status"], "success");
}

#[test]
fn mcp_invalid_startup_configuration_exits_nonzero() {
    let config_dir = TempDir::new().expect("temporary config dir should be created");
    let output = ProcessCommand::new(assert_cmd::cargo::cargo_bin("ah"))
        .env("AH_CONFIG_DIR", config_dir.path())
        .args(["mcp", "serve", "--max-active", "0"])
        .stdin(Stdio::null())
        .output()
        .expect("invalid MCP configuration should exit");

    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("ah: --max-active must be >= 1"),
        "stderr should explain the invalid configuration: {stderr}"
    );
    assert!(!stderr.contains("INVALID_RANGE"), "{stderr}");
}

#[test]
fn http_port_conflict_exits_nonzero() {
    let config_dir = TempDir::new().expect("temporary config dir should be created");
    let listener = TcpListener::bind("127.0.0.1:0").expect("ephemeral port should bind");
    let port = listener
        .local_addr()
        .expect("ephemeral address should be available")
        .port()
        .to_string();
    let output = ProcessCommand::new(assert_cmd::cargo::cargo_bin("ah"))
        .env("AH_CONFIG_DIR", config_dir.path())
        .args(["mcp", "serve", "--transport", "http", "--port", &port])
        .stdin(Stdio::null())
        .output()
        .expect("conflicting HTTP MCP server should exit");

    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.starts_with("ah: "),
        "stderr should contain a human-readable bind diagnostic: {stderr}"
    );
    assert!(!stderr.contains("MCP_SERVER_FAILED"), "{stderr}");
    let event = log_records(&config_dir)
        .into_iter()
        .find(|record| record["event"] == "system" && record["component"] == "mcp_transport")
        .expect("bind failure should be logged as an MCP transport event");
    assert_eq!(event["diagnostic"]["code"], "MCP_SERVER_FAILED");
    assert_eq!(event["diagnostic"]["exit_code_hint"], 1);
}

#[test]
fn http_shutdown_timeout_exits_nonzero() {
    let config_dir = TempDir::new().expect("temporary config dir should be created");
    let mut process = HttpMcpProcess::start(&config_dir);
    let (_, readiness) = read_readiness(&process);
    let instance_id = readiness["instance_id"]
        .as_str()
        .expect("instance identity should be text");
    let authority = process
        .origin
        .strip_prefix("http://")
        .expect("test origin should use HTTP");
    let mut stalled_request =
        TcpStream::connect(authority).expect("stalled control request should connect");
    write!(
        stalled_request,
        "POST /control/shutdown HTTP/1.1\r\nHost: {authority}\r\nContent-Type: application/json\r\nContent-Length: 128\r\nConnection: keep-alive\r\n\r\n{{"
    )
    .expect("partial control request should be written");
    stalled_request
        .flush()
        .expect("partial control request should be flushed");
    thread::sleep(Duration::from_millis(100));

    let accepted = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .expect("HTTP client should build")
        .post(&process.shutdown_url)
        .json(&json!({"instance_id": instance_id}))
        .send()
        .expect("matching shutdown request should receive a response");
    assert_eq!(accepted.status(), reqwest::StatusCode::ACCEPTED);

    let exit = process.wait_for_exit(Duration::from_secs(7));
    assert_eq!(exit.code(), Some(1));
    let stderr = process.read_stderr();
    assert!(
        stderr.contains("ah: MCP shutdown exceeded the configured 5000 ms grace period"),
        "stderr should explain the shutdown timeout: {stderr}"
    );
    assert!(!stderr.contains("MCP_SHUTDOWN_TIMEOUT"), "{stderr}");
    assert!(
        stderr.contains("5000 ms grace period"),
        "stderr should contain the deterministic grace period: {stderr}"
    );
    let event = log_records(&config_dir)
        .into_iter()
        .find(|record| record["event"] == "system" && record["component"] == "mcp_transport")
        .expect("shutdown timeout should be logged as an MCP transport event");
    assert_eq!(event["diagnostic"]["code"], "MCP_SHUTDOWN_TIMEOUT");
    assert_eq!(event["diagnostic"]["exit_code_hint"], 1);
}

#[test]
fn http_transport_rejects_hostile_host_origin_and_oversized_body() {
    let config_dir = TempDir::new().expect("temporary config dir should be created");
    let process = HttpMcpProcess::start(&config_dir);
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .expect("HTTP client should build");
    let initialize = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2025-11-25",
            "capabilities": {},
            "clientInfo": {"name": "security-test", "version": "1.0.0"}
        }
    });
    let deadline = Instant::now() + Duration::from_secs(10);
    while client.get(&process.url).send().is_err() {
        assert!(Instant::now() < deadline, "HTTP MCP server should start");
        thread::sleep(Duration::from_millis(25));
    }
    let hostile_host = client
        .post(&process.url)
        .header("Host", "evil.example")
        .header("Accept", "application/json, text/event-stream")
        .header("Content-Type", "application/json")
        .json(&initialize)
        .send()
        .expect("hostile Host request should receive a response");
    assert_eq!(hostile_host.status(), reqwest::StatusCode::FORBIDDEN);

    let hostile_origin = client
        .post(&process.url)
        .header("Origin", "http://evil.example")
        .header("Accept", "application/json, text/event-stream")
        .header("Content-Type", "application/json")
        .json(&initialize)
        .send()
        .expect("hostile Origin request should receive a response");
    assert_eq!(hostile_origin.status(), reqwest::StatusCode::FORBIDDEN);

    let hostile_readiness_host = client
        .get(&process.readiness_url)
        .header("Host", "evil.example")
        .send()
        .expect("hostile readiness Host request should receive a response");
    assert_eq!(
        hostile_readiness_host.status(),
        reqwest::StatusCode::FORBIDDEN
    );

    let hostile_readiness_origin = client
        .get(&process.readiness_url)
        .header("Origin", "http://evil.example")
        .send()
        .expect("hostile readiness Origin request should receive a response");
    assert_eq!(
        hostile_readiness_origin.status(),
        reqwest::StatusCode::FORBIDDEN
    );

    let local_readiness_origin = client
        .get(&process.readiness_url)
        .header("Origin", &process.origin)
        .send()
        .expect("local readiness Origin request should receive a response");
    assert_eq!(local_readiness_origin.status(), reqwest::StatusCode::OK);
    assert!(
        local_readiness_origin
            .headers()
            .get("access-control-allow-origin")
            .is_none()
    );

    let oversized = client
        .post(&process.url)
        .header("Accept", "application/json, text/event-stream")
        .header("Content-Type", "application/json")
        .body("x".repeat(1024 * 1024 + 1))
        .send()
        .expect("oversized request should receive a response");
    assert_eq!(oversized.status(), reqwest::StatusCode::PAYLOAD_TOO_LARGE);
    process.stop();
}

#[cfg(unix)]
#[test]
fn http_server_stops_gracefully_on_sigterm() {
    let config_dir = TempDir::new().expect("temporary config dir should be created");
    let mut process = HttpMcpProcess::start(&config_dir);
    let _client = HttpMcpClient::connect(&process.url, "sigterm-client");
    let pid = process.child.inner().id().to_string();

    let signal = ProcessCommand::new("kill")
        .args(["-TERM", &pid])
        .status()
        .expect("SIGTERM should be sent");
    assert!(signal.success());

    let deadline = Instant::now() + Duration::from_secs(7);
    let exit = loop {
        match process.child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(10)),
            Ok(None) => {
                let _ = process.child.kill();
                let _ = process.child.wait();
                panic!("HTTP MCP server did not stop after SIGTERM");
            }
            Err(error) => panic!("HTTP MCP process status should be readable: {error}"),
        }
    };
    assert!(exit.success(), "HTTP MCP server should exit cleanly");
}

#[test]
fn direct_calls_and_jobs_share_fail_fast_capacity() {
    let config_dir = TempDir::new().expect("temporary config dir should be created");
    let workspace = TempDir::new().expect("temporary workspace should be created");
    let ready = workspace.path().join("capacity.ready");
    let release = workspace.path().join("capacity.release");
    let must_not_start = workspace.path().join("must-not-start.txt");
    let release_marker = ReleaseMarker::new(release.clone());
    let mut server = McpProcess::start_with_args(&config_dir, &["--max-active", "1"]);
    server.send(json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2025-11-25",
            "capabilities": {},
            "clientInfo": {"name": "capacity-test", "version": "1.0.0"}
        }
    }));
    assert_eq!(server.response_for(1)["id"], 1);
    server.send(json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));
    server.send(json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": {
            "name": "ah.run.check",
            "arguments": {
                "command": blocker_command(&ready, &release),
                "context": {"cwd": workspace.path().to_string_lossy(), "timeout_ms": 10_000}
            }
        }
    }));
    wait_for_path(&ready);

    server.send(json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "tools/call",
        "params": {
            "name": "ah.job.start",
            "arguments": {
                "tool": "ah.run.check",
                "arguments": {
                    "command": write_marker_command(&must_not_start),
                    "context": {"cwd": workspace.path().to_string_lossy(), "timeout_ms": 5000}
                }
            }
        }
    }));
    let rejected = server.response_for(3);
    assert_eq!(rejected["result"]["isError"], true);
    assert_eq!(
        rejected["result"]["_meta"]["dev.aihelper/diagnostic"]["code"],
        "EXECUTION_CAPACITY_FULL"
    );
    release_marker.release();
    assert_eq!(server.response_for(2)["result"]["isError"], false);
    thread::sleep(Duration::from_millis(100));
    assert!(
        !must_not_start.exists(),
        "rejected job must never start later"
    );
    server.stop();
}

#[test]
fn stdio_server_executes_calls_in_parallel_without_queue_wait() {
    let config_dir = TempDir::new().expect("temporary config dir should be created");
    let workspace = TempDir::new().expect("temporary workspace should be created");
    let ready = workspace.path().join("blocker.ready");
    let release = workspace.path().join("blocker.release");
    let sample = workspace.path().join("sample.txt");
    std::fs::write(&sample, b"sample").expect("sample file should be written");
    let mut server = McpProcess::start_with_args(&config_dir, &["--max-active", "2"]);
    let release_marker = ReleaseMarker::new(release.clone());

    server.send(json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2025-11-25",
            "capabilities": {},
            "clientInfo": {"name": "aihelper-timeout-test", "version": "1.0.0"}
        }
    }));
    assert_eq!(server.response_for(1)["id"], 1);
    server.send(json!({
        "jsonrpc": "2.0",
        "method": "notifications/initialized"
    }));

    server.send(json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": {
            "name": "ah.run.check",
            "arguments": {
                "command": blocker_command(&ready, &release),
                "timeout_secs": 10,
                "context": {
                    "cwd": workspace.path().to_string_lossy(),
                    "timeout_ms": 10_000
                }
            }
        }
    }));
    wait_for_path(&ready);

    server.send(json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "tools/call",
        "params": {
            "name": "ah.file.stat",
            "arguments": {
                "path": "sample.txt",
                "context": {
                    "cwd": workspace.path().to_string_lossy(),
                    "timeout_ms": 1000
                }
            }
        }
    }));
    let parallel = server.response();
    assert_eq!(parallel["id"], 3, "parallel call should respond first");
    assert_eq!(parallel["result"]["isError"], false);

    release_marker.release();
    let blocker = server.response_for(2);
    assert_eq!(blocker["result"]["isError"], false);
    assert_eq!(blocker["result"]["structuredContent"]["success"], true);
    assert_eq!(blocker["result"]["structuredContent"]["timed_out"], false);
    server.stop();

    let records = log_records(&config_dir);
    let events = records
        .iter()
        .filter(|record| {
            record["event"] == "command.completed"
                && record["transport"] == "mcp"
                && record["command"] == "file.stat"
                && record["status"] == "success"
        })
        .collect::<Vec<_>>();
    assert_eq!(events.len(), 1);
    let event = events[0];
    assert!(event["timeout_phase"].is_null());
    assert_eq!(event["queue_wait_ms"], 0);
}

fn wait_for_path(path: &Path) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !path.exists() {
        assert!(Instant::now() < deadline, "blocker did not become ready");
        thread::sleep(Duration::from_millis(10));
    }
}

#[cfg(windows)]
fn blocker_command(ready: &Path, release: &Path) -> Vec<String> {
    let ready = ready.to_string_lossy().replace('\'', "''");
    let release = release.to_string_lossy().replace('\'', "''");
    vec![
        "powershell.exe".to_owned(),
        "-NoProfile".to_owned(),
        "-Command".to_owned(),
        format!(
            "$ready='{ready}'; $release='{release}'; [IO.File]::WriteAllText($ready, 'ready'); \
             $deadline=[DateTime]::UtcNow.AddSeconds(10); while (!(Test-Path -LiteralPath $release)) {{ \
             if ([DateTime]::UtcNow -ge $deadline) {{ exit 2 }}; Start-Sleep -Milliseconds 10 }}"
        ),
    ]
}

#[cfg(windows)]
fn shutdown_blocker_command(ready: &Path, release: &Path, completed: &Path) -> Vec<String> {
    let ready = ready.to_string_lossy().replace('\'', "''");
    let release = release.to_string_lossy().replace('\'', "''");
    let completed = completed.to_string_lossy().replace('\'', "''");
    vec![
        "powershell.exe".to_owned(),
        "-NoProfile".to_owned(),
        "-Command".to_owned(),
        format!(
            "$ready='{ready}'; $release='{release}'; $completed='{completed}'; \
             [IO.File]::WriteAllText($ready, 'ready'); \
             $deadline=[DateTime]::UtcNow.AddSeconds(30); while (!(Test-Path -LiteralPath $release)) {{ \
             if ([DateTime]::UtcNow -ge $deadline) {{ exit 2 }}; Start-Sleep -Milliseconds 10 }}; \
             [IO.File]::WriteAllText($completed, 'completed')"
        ),
    ]
}

#[cfg(windows)]
fn write_marker_command(path: &Path) -> Vec<String> {
    let path = path.to_string_lossy().replace('\'', "''");
    vec![
        "powershell.exe".to_owned(),
        "-NoProfile".to_owned(),
        "-Command".to_owned(),
        format!("[IO.File]::WriteAllText('{path}', 'started')"),
    ]
}

#[cfg(not(windows))]
fn blocker_command(ready: &Path, release: &Path) -> Vec<String> {
    vec![
        "sh".to_owned(),
        "-c".to_owned(),
        "touch \"$1\"; attempts=0; while [ ! -f \"$2\" ]; do attempts=$((attempts + 1)); \
         [ \"$attempts\" -ge 1000 ] && exit 2; sleep 0.01; done"
            .to_owned(),
        "sh".to_owned(),
        ready.to_string_lossy().into_owned(),
        release.to_string_lossy().into_owned(),
    ]
}

#[cfg(not(windows))]
fn shutdown_blocker_command(ready: &Path, release: &Path, completed: &Path) -> Vec<String> {
    vec![
        "sh".to_owned(),
        "-c".to_owned(),
        "touch \"$1\"; attempts=0; while [ ! -f \"$2\" ]; do attempts=$((attempts + 1)); \
         [ \"$attempts\" -ge 3000 ] && exit 2; sleep 0.01; done; printf completed > \"$3\""
            .to_owned(),
        "sh".to_owned(),
        ready.to_string_lossy().into_owned(),
        release.to_string_lossy().into_owned(),
        completed.to_string_lossy().into_owned(),
    ]
}

#[cfg(not(windows))]
fn write_marker_command(path: &Path) -> Vec<String> {
    vec![
        "sh".to_owned(),
        "-c".to_owned(),
        "printf started > \"$1\"".to_owned(),
        "sh".to_owned(),
        path.to_string_lossy().into_owned(),
    ]
}

fn log_records(config_dir: &TempDir) -> Vec<Value> {
    let mut paths = std::fs::read_dir(config_dir.path().join("logs"))
        .expect("log directory should exist")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "jsonl")
        })
        .collect::<Vec<_>>();
    paths.sort();
    paths
        .into_iter()
        .flat_map(|path| {
            std::fs::read_to_string(path)
                .expect("log should be readable")
                .lines()
                .map(|line| serde_json::from_str(line).expect("log line should be JSON"))
                .collect::<Vec<Value>>()
        })
        .collect()
}
