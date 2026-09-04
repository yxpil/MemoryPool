//! Integration tests for the MCP surface of `memorypool serve` (Streamable
//! HTTP JSON-RPC on `/` and `/mcp`). Runs the real binary against an isolated
//! data directory, so every tool call exercises the actual store.

use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use serde_json::{json, Value};

struct ServeHandle {
    child: Child,
    addr: String,
}

impl Drop for ServeHandle {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn data_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "memorypool-mcp-it-{}-{name}",
        uuid::Uuid::new_v4().simple()
    ));
    std::fs::create_dir_all(&dir).expect("failed to create temp data dir");
    dir
}

fn spawn_serve(dir: &PathBuf) -> ServeHandle {
    let mut child = Command::new(env!("CARGO_BIN_EXE_memorypool"))
        .args(["serve", "--port", "0"])
        .env("MEMORYPOOL_DATA_DIR", dir)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn memorypool serve");
    let stderr = child.stderr.take().expect("serve stderr");
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stderr).lines() {
            let Ok(line) = line else { break };
            if let Some(addr) = line
                .trim()
                .strip_prefix("memorypool listening on http://")
                .and_then(|rest| rest.split_whitespace().next())
            {
                let _ = tx.send(addr.to_string());
                break;
            }
        }
    });
    let addr = rx
        .recv_timeout(Duration::from_secs(20))
        .expect("serve did not print its listening address in time");
    ServeHandle { child, addr }
}

/// POST a JSON-RPC message; returns (status, Mcp-Session-Id?, body).
fn rpc(addr: &str, session: Option<&str>, message: Value) -> (u16, Option<String>, Value) {
    let mut request =
        ureq::post(&format!("http://{addr}/mcp")).set("Content-Type", "application/json");
    if let Some(sid) = session {
        request = request.set("Mcp-Session-Id", sid);
    }
    match request.send_string(&message.to_string()) {
        Ok(resp) => (
            resp.status(),
            resp.header("Mcp-Session-Id").map(str::to_string),
            resp.into_json().unwrap_or(Value::Null),
        ),
        Err(ureq::Error::Status(status, resp)) => (
            status,
            resp.header("Mcp-Session-Id").map(str::to_string),
            resp.into_json().unwrap_or(Value::Null),
        ),
        Err(e) => panic!("http request failed: {e}"),
    }
}

fn initialize(addr: &str) -> String {
    let (status, sid, body) = rpc(
        addr,
        None,
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-03-26",
                "capabilities": {},
                "clientInfo": { "name": "test", "version": "0" }
            }
        }),
    );
    assert_eq!(status, 200);
    assert_eq!(body["result"]["serverInfo"]["name"], "memorypool");
    assert_eq!(body["result"]["protocolVersion"], "2025-03-26");
    sid.expect("initialize must issue an Mcp-Session-Id")
}

fn call(addr: &str, sid: &str, id: i64, name: &str, args: Value) -> Value {
    let (_, _, body) = rpc(
        addr,
        Some(sid),
        json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "tools/call",
            "params": { "name": name, "arguments": args }
        }),
    );
    body["result"].clone()
}

fn payload(result: &Value) -> Value {
    serde_json::from_str(result["content"][0]["text"].as_str().expect("text content"))
        .expect("tool payload is JSON")
}

#[test]
fn handshake_tools_list_and_full_add_search_get_delete_chain() {
    let dir = data_dir("chain");
    let server = spawn_serve(&dir);
    let addr = server.addr.clone();

    let sid = initialize(&addr);

    // Notifications are accepted silently.
    let (status, _, body) = rpc(
        &addr,
        Some(&sid),
        json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
    );
    assert_eq!(status, 202);
    assert_eq!(body, Value::Null);

    // tools/list: six tools with schemas.
    let (_, _, body) = rpc(
        &addr,
        Some(&sid),
        json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {} }),
    );
    let tools = body["result"]["tools"].as_array().expect("tools");
    let names: Vec<&str> = tools
        .iter()
        .map(|t| t["name"].as_str().expect("name"))
        .collect();
    assert_eq!(
        names,
        vec!["add", "search", "list", "get", "delete", "stats"]
    );
    for tool in tools {
        assert_eq!(tool["inputSchema"]["type"], "object");
    }

    // add
    let result = call(
        &addr,
        &sid,
        3,
        "add",
        json!({"text":"mcp integration memory","tags":["test"],"importance":0.8}),
    );
    assert_eq!(result["isError"], false);
    let memory = payload(&result);
    let id = memory["id"].as_str().expect("id").to_string();
    assert_eq!(memory["text"], "mcp integration memory");

    // get
    let result = call(&addr, &sid, 4, "get", json!({"id": id}));
    assert_eq!(result["isError"], false);
    assert_eq!(payload(&result)["id"], id.as_str());

    // search
    let result = call(&addr, &sid, 5, "search", json!({"query":"integration"}));
    assert_eq!(result["isError"], false);
    let results = payload(&result);
    assert_eq!(results.as_array().expect("results").len(), 1);
    assert_eq!(results[0]["id"], id.as_str());
    assert!(results[0]["score"].is_number());

    // list with tag filter
    let result = call(&addr, &sid, 6, "list", json!({"tag":"test"}));
    assert_eq!(result["isError"], false);
    assert_eq!(payload(&result).as_array().expect("items").len(), 1);

    // stats
    let result = call(&addr, &sid, 7, "stats", json!({}));
    assert_eq!(result["isError"], false);
    assert_eq!(payload(&result)["count"], 1);

    // delete, then get fails
    let result = call(&addr, &sid, 8, "delete", json!({"id": id}));
    assert_eq!(result["isError"], false);
    let result = call(&addr, &sid, 9, "get", json!({"id": id}));
    assert_eq!(result["isError"], true);
    assert!(result["content"][0]["text"]
        .as_str()
        .expect("text")
        .contains("not found"));
}

#[test]
fn unknown_tool_is_jsonrpc_error_and_bad_args_are_error_results() {
    let dir = data_dir("errors");
    let server = spawn_serve(&dir);
    let addr = server.addr.clone();
    let sid = initialize(&addr);

    let (_, _, body) = rpc(
        &addr,
        Some(&sid),
        json!({
            "jsonrpc": "2.0",
            "id": 10,
            "method": "tools/call",
            "params": { "name": "nope", "arguments": {} }
        }),
    );
    assert_eq!(body["error"]["code"], -32602);

    // add without text → isError result naming the missing field.
    let result = call(&addr, &sid, 11, "add", json!({}));
    assert_eq!(result["isError"], true);
    assert!(result["content"][0]["text"]
        .as_str()
        .expect("text")
        .contains("text"));

    // get without id → isError result.
    let result = call(&addr, &sid, 12, "get", json!({}));
    assert_eq!(result["isError"], true);

    // unknown method → -32601; ping → {}.
    let (_, _, body) = rpc(
        &addr,
        Some(&sid),
        json!({ "jsonrpc": "2.0", "id": 13, "method": "resources/list" }),
    );
    assert_eq!(body["error"]["code"], -32601);
    let (_, _, body) = rpc(
        &addr,
        Some(&sid),
        json!({ "jsonrpc": "2.0", "id": 14, "method": "ping" }),
    );
    assert_eq!(body["result"], json!({}));
}

#[test]
fn token_protects_mcp_endpoint_too() {
    let dir = data_dir("token");
    let server = {
        let mut child = Command::new(env!("CARGO_BIN_EXE_memorypool"))
            .args(["serve", "--port", "0", "--token", "secret"])
            .env("MEMORYPOOL_DATA_DIR", &dir)
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .expect("failed to spawn memorypool serve");
        let stderr = child.stderr.take().expect("serve stderr");
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stderr).lines() {
                let Ok(line) = line else { break };
                if let Some(addr) = line
                    .trim()
                    .strip_prefix("memorypool listening on http://")
                    .and_then(|rest| rest.split_whitespace().next())
                {
                    let _ = tx.send(addr.to_string());
                    break;
                }
            }
        });
        let addr = rx
            .recv_timeout(Duration::from_secs(20))
            .expect("serve did not print its listening address in time");
        ServeHandle { child, addr }
    };
    // Without the bearer token the MCP endpoint answers 401 like /invoke.
    let response = ureq::post(&format!("http://{}/mcp", server.addr))
        .set("Content-Type", "application/json")
        .send_string(r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#);
    match response {
        Err(ureq::Error::Status(401, _)) => {}
        other => panic!("expected 401 without token, got {other:?}"),
    }
    // With the token the handshake succeeds.
    let response = ureq::post(&format!("http://{}/mcp", server.addr))
        .set("Content-Type", "application/json")
        .set("Authorization", "Bearer secret")
        .send_string(
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{}}}"#,
        )
        .expect("token request succeeds");
    assert_eq!(response.status(), 200);
    let body: Value = response.into_json().expect("json");
    assert_eq!(body["result"]["serverInfo"]["name"], "memorypool");
}
