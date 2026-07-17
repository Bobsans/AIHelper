use std::{
    io::{BufRead, BufReader, Write},
    process::{Child, ChildStdin, Command as ProcessCommand, Stdio},
    sync::mpsc::{self, Receiver},
    thread,
    time::{Duration, Instant},
};

use serde_json::{Value, json};
use tempfile::TempDir;

struct McpProcess {
    child: Child,
    stdin: Option<ChildStdin>,
    responses: Receiver<String>,
}

impl McpProcess {
    fn start(config_dir: &TempDir) -> Self {
        let mut child = ProcessCommand::new(assert_cmd::cargo::cargo_bin("ah"))
            .env("AH_CONFIG_DIR", config_dir.path())
            .args(["mcp", "serve", "--default-timeout-ms", "5000"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("MCP server should start");
        let stdin = child.stdin.take().expect("MCP stdin should be piped");
        let stdout = child.stdout.take().expect("MCP stdout should be piped");
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
        }
    }

    fn send(&mut self, message: Value) {
        let stdin = self.stdin.as_mut().expect("MCP stdin should be open");
        writeln!(stdin, "{message}").expect("MCP message should be written");
        stdin.flush().expect("MCP message should be flushed");
    }

    fn response(&self) -> Value {
        let line = self
            .responses
            .recv_timeout(Duration::from_secs(10))
            .expect("MCP server should respond");
        serde_json::from_str(&line).expect("MCP stdout must contain JSON-RPC only")
    }

    fn stop(mut self) {
        drop(self.stdin.take());
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if self
                .child
                .try_wait()
                .expect("MCP process status should be readable")
                .is_some()
            {
                return;
            }
            if Instant::now() >= deadline {
                self.child.kill().expect("MCP process should stop");
                let _ = self.child.wait();
                panic!("MCP server did not stop after stdin closed");
            }
            thread::sleep(Duration::from_millis(10));
        }
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

    server.stop();
}
