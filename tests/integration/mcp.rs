use std::{
    collections::HashMap,
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::{ChildStdin, Command as ProcessCommand, Stdio},
    sync::mpsc::{self, Receiver},
    thread,
    time::{Duration, Instant},
};

use command_group::{CommandGroup, GroupChild};
use serde_json::{Value, json};
use tempfile::TempDir;

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
        let mut command = ProcessCommand::new(assert_cmd::cargo::cargo_bin("ah"));
        command
            .env("AH_CONFIG_DIR", config_dir.path())
            .args(["mcp", "serve", "--default-timeout-ms", "5000"])
            .args(extra_args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
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
                Ok(Some(_)) => return,
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
fn stdio_server_logs_head_of_line_timeout_as_queued() {
    let config_dir = TempDir::new().expect("temporary config dir should be created");
    let workspace = TempDir::new().expect("temporary workspace should be created");
    let ready = workspace.path().join("blocker.ready");
    let release = workspace.path().join("blocker.release");
    let sample = workspace.path().join("sample.txt");
    std::fs::write(&sample, b"sample").expect("sample file should be written");
    let mut server = McpProcess::start_with_args(&config_dir, &["--max-queued", "2"]);
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
                    "timeout_ms": 5000
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
                    "timeout_ms": 200
                }
            }
        }
    }));
    let timed_out = server.response();
    assert_eq!(timed_out["id"], 3, "queued timeout should respond first");
    assert_eq!(timed_out["result"]["isError"], true);
    assert_eq!(
        timed_out["result"]["_meta"]["dev.aihelper/diagnostic"]["code"],
        "TIMEOUT"
    );

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
                && record["status"] == "error"
        })
        .collect::<Vec<_>>();
    assert_eq!(events.len(), 1);
    let event = events[0];
    assert_eq!(event["diagnostic"]["code"], "TIMEOUT");
    assert_eq!(event["timeout_phase"], "queue");
    assert_eq!(event["execution_ms"], 0);
    assert!(
        event["queue_wait_ms"]
            .as_u64()
            .is_some_and(|value| value > 0)
    );
    assert!(
        event["duration_ms"].as_u64().unwrap_or_default()
            >= event["queue_wait_ms"].as_u64().unwrap_or(u64::MAX)
    );
}

fn wait_for_path(path: &Path) {
    let deadline = Instant::now() + Duration::from_secs(5);
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
             $deadline=[DateTime]::UtcNow.AddSeconds(5); while (!(Test-Path -LiteralPath $release)) {{ \
             if ([DateTime]::UtcNow -ge $deadline) {{ exit 2 }}; Start-Sleep -Milliseconds 10 }}"
        ),
    ]
}

#[cfg(not(windows))]
fn blocker_command(ready: &Path, release: &Path) -> Vec<String> {
    vec![
        "sh".to_owned(),
        "-c".to_owned(),
        "touch \"$1\"; attempts=0; while [ ! -f \"$2\" ]; do attempts=$((attempts + 1)); \
         [ \"$attempts\" -ge 500 ] && exit 2; sleep 0.01; done"
            .to_owned(),
        "sh".to_owned(),
        ready.to_string_lossy().into_owned(),
        release.to_string_lossy().into_owned(),
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
