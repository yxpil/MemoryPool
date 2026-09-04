//! Integration tests: they run the real `memorypool` binary against an
//! isolated data directory (`MEMORYPOOL_DATA_DIR`), covering the full
//! add -> search -> delete chain, stdin piping (BIT exec-mode contract) and
//! the serve mode HTTP API (including /invoke and token auth).

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use serde_json::{json, Value};

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_memorypool")
}

fn data_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "memorypool-it-{}-{name}",
        uuid::Uuid::new_v4().simple()
    ));
    std::fs::create_dir_all(&dir).expect("failed to create temp data dir");
    dir
}

fn run(args: &[&str], dir: &Path) -> Output {
    Command::new(bin())
        .args(args)
        .env("MEMORYPOOL_DATA_DIR", dir)
        .stdin(Stdio::null())
        .output()
        .expect("failed to spawn memorypool")
}

fn run_with_stdin(args: &[&str], dir: &Path, stdin: &str) -> Output {
    let mut child = Command::new(bin())
        .args(args)
        .env("MEMORYPOOL_DATA_DIR", dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn memorypool");
    child
        .stdin
        .as_mut()
        .expect("stdin")
        .write_all(stdin.as_bytes())
        .expect("failed to write stdin");
    child.wait_with_output().expect("failed to collect output")
}

fn success(out: &Output, context: &str) {
    assert!(
        out.status.success(),
        "{context} failed (exit {:?}): stdout={}, stderr={}",
        out.status.code(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

fn failure(out: &Output, context: &str) {
    assert!(
        !out.status.success(),
        "{context} unexpectedly succeeded: stdout={}",
        String::from_utf8_lossy(&out.stdout)
    );
}

fn stdout_json(out: &Output, context: &str) -> Value {
    let text = String::from_utf8_lossy(&out.stdout);
    serde_json::from_str(text.trim())
        .unwrap_or_else(|e| panic!("{context}: stdout is not valid JSON ({e}): {text}"))
}

#[test]
fn full_chain_add_search_get_tag_delete() {
    let dir = data_dir("full-chain");

    // add (first memory with tags + importance + custom source)
    let out = run(
        &[
            "add",
            "Deploy redis cluster on Friday",
            "--tags",
            "ops,infra",
            "--importance",
            "0.9",
            "--source",
            "bit",
            "--json",
        ],
        &dir,
    );
    success(&out, "add");
    let mem: Value = stdout_json(&out, "add output");
    let id1 = mem["id"].as_str().expect("id").to_string();
    assert_eq!(mem["text"], "Deploy redis cluster on Friday");
    assert_eq!(mem["tags"], json!(["ops", "infra"]));
    assert!((mem["importance"].as_f64().unwrap() - 0.9).abs() < 1e-6);
    assert_eq!(mem["source"], "bit");
    assert!(!mem["created_at"].as_str().unwrap().is_empty());
    assert!(!mem["updated_at"].as_str().unwrap().is_empty());

    // add a second, unrelated memory
    let out = run(&["add", "Buy groceries tomorrow", "--json"], &dir);
    success(&out, "add 2");

    // search finds only the matching one, ranked, with a score field
    let out = run(&["search", "redis deploy", "--json"], &dir);
    success(&out, "search");
    let results = stdout_json(&out, "search output")
        .as_array()
        .cloned()
        .unwrap();
    assert_eq!(results.len(), 1, "expected exactly one match");
    assert_eq!(results[0]["id"], id1.as_str());
    assert!(results[0]["score"].as_f64().unwrap() > 0.0);

    // search with no match returns an empty array
    let out = run(&["search", "quantum", "--json"], &dir);
    success(&out, "search no match");
    assert_eq!(
        stdout_json(&out, "empty search").as_array().unwrap().len(),
        0
    );

    // search respects --tag filter
    let out = run(&["search", "groceries", "--tag", "ops", "--json"], &dir);
    success(&out, "search tag filter");
    assert_eq!(
        stdout_json(&out, "tag-filtered search")
            .as_array()
            .unwrap()
            .len(),
        0
    );

    // get by id
    let out = run(&["get", &id1, "--json"], &dir);
    success(&out, "get");
    assert_eq!(
        stdout_json(&out, "get output")["text"],
        "Deploy redis cluster on Friday"
    );

    // tag add/remove
    let out = run(
        &[
            "tag", &id1, "--add", "urgent", "--remove", "infra", "--json",
        ],
        &dir,
    );
    success(&out, "tag");
    let updated = stdout_json(&out, "tag output");
    assert_eq!(updated["tags"], json!(["ops", "urgent"]));

    // delete
    let out = run(&["delete", &id1, "--json"], &dir);
    success(&out, "delete");
    assert_eq!(stdout_json(&out, "delete output")["deleted"], id1.as_str());

    // it is gone
    let out = run(&["search", "redis", "--json"], &dir);
    success(&out, "search after delete");
    assert_eq!(
        stdout_json(&out, "post-delete search")
            .as_array()
            .unwrap()
            .len(),
        0
    );

    // error path: getting a deleted id fails with "not found" on stderr
    let out = run(&["get", &id1], &dir);
    failure(&out, "get deleted id");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("not found"), "unexpected stderr: {stderr}");

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn stdin_pipe_merges_over_cli_args() {
    let dir = data_dir("stdin-pipe");

    // BIT exec-mode contract: params JSON arrives via stdin, stdout is JSON
    // even without --json.
    let out = run_with_stdin(
        &["add"],
        &dir,
        r#"{"text":"memory delivered over stdin","tags":["piped"],"importance":0.7,"source":"bit"}"#,
    );
    success(&out, "add via stdin");
    let mem = stdout_json(&out, "add via stdin output");
    assert_eq!(mem["text"], "memory delivered over stdin");
    assert_eq!(mem["tags"], json!(["piped"]));
    assert!((mem["importance"].as_f64().unwrap() - 0.7).abs() < 1e-6);
    assert_eq!(mem["source"], "bit");
    let id = mem["id"].as_str().unwrap().to_string();

    // search driven entirely by stdin JSON
    let out = run_with_stdin(&["search"], &dir, r#"{"query":"stdin","limit":5}"#);
    success(&out, "search via stdin");
    let results = stdout_json(&out, "search via stdin output");
    assert_eq!(results.as_array().unwrap().len(), 1);
    assert_eq!(results[0]["id"], id.as_str());

    // stdin wins over CLI args
    let out = run_with_stdin(
        &["search", "nomatch", "--limit", "3"],
        &dir,
        r#"{"query":"stdin"}"#,
    );
    success(&out, "search stdin wins");
    assert_eq!(
        stdout_json(&out, "stdin-wins search")
            .as_array()
            .unwrap()
            .len(),
        1
    );

    // invalid stdin JSON is ignored with a warning; the command still works
    let out = run_with_stdin(&["list", "--json"], &dir, "not json at all");
    success(&out, "list with bad stdin");
    assert_eq!(
        stdout_json(&out, "list after bad stdin")
            .as_array()
            .unwrap()
            .len(),
        1
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn list_and_stats_reflect_the_pool() {
    let dir = data_dir("list-stats");
    run(&["add", "alpha note", "--tags", "work", "--json"], &dir);
    run(
        &[
            "add",
            "beta note",
            "--tags",
            "work,home",
            "--importance",
            "0.2",
            "--json",
        ],
        &dir,
    );
    run(&["add", "gamma note", "--tags", "home", "--json"], &dir);

    // list newest first, filterable by tag and limited
    let out = run(&["list", "--json"], &dir);
    success(&out, "list");
    let all = stdout_json(&out, "list output")
        .as_array()
        .cloned()
        .unwrap();
    assert_eq!(all.len(), 3);
    assert_eq!(all[0]["text"], "gamma note");
    let out = run(&["list", "--tag", "work", "--json"], &dir);
    success(&out, "list tag");
    let tagged = stdout_json(&out, "tagged list")
        .as_array()
        .cloned()
        .unwrap();
    assert_eq!(tagged.len(), 2);
    let out = run(&["list", "--limit", "1", "--json"], &dir);
    success(&out, "list limit");
    assert_eq!(
        stdout_json(&out, "limited list").as_array().unwrap().len(),
        1
    );

    // stats
    let out = run(&["stats", "--json"], &dir);
    success(&out, "stats");
    let stats = stdout_json(&out, "stats output");
    assert_eq!(stats["count"], 3);
    assert_eq!(stats["tags"]["work"], 2);
    assert_eq!(stats["tags"]["home"], 2);
    assert!(stats["oldest"].is_string());
    assert!(stats["newest"].is_string());
    assert!(stats["oldest"].as_str().unwrap() <= stats["newest"].as_str().unwrap());

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn human_mode_and_error_paths() {
    let dir = data_dir("human-errors");

    // default (non --json) output is human-readable
    let out = run(&["add", "human readable note"], &dir);
    success(&out, "human add");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("human readable note"), "stdout: {stdout}");
    let out = run(&["search", "human"], &dir);
    success(&out, "human search");
    assert!(
        String::from_utf8_lossy(&out.stdout).contains('1'),
        "ranked list expected"
    );

    // empty pool / no match messaging
    let out = run(&["search", "zzz-nothing"], &dir);
    success(&out, "empty search human");
    assert!(String::from_utf8_lossy(&out.stdout).contains("No memories found."));

    // missing required text
    let out = run(&["add"], &dir);
    failure(&out, "add without text");
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("text is required")
            || String::from_utf8_lossy(&out.stderr).contains("missing required field: text")
    );

    // missing id
    let out = run(&["delete", "00000000-0000-0000-0000-000000000000"], &dir);
    failure(&out, "delete unknown id");
    assert!(String::from_utf8_lossy(&out.stderr).contains("not found"));

    // data landed in MEMORYPOOL_DATA_DIR, not in ~/.memorypool
    assert!(
        dir.join("memories.jsonl").exists(),
        "data file missing in override dir"
    );

    std::fs::remove_dir_all(&dir).ok();
}

// ==================== serve mode ====================

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

fn spawn_serve(dir: &Path, extra_args: &[&str]) -> ServeHandle {
    let mut child = Command::new(bin())
        .args(["serve", "--port", "0"])
        .args(extra_args)
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

fn http(req: ureq::Request, body: Option<Value>) -> (u16, Value) {
    let result = match body {
        Some(v) => req.send_json(v),
        None => req.call(),
    };
    match result {
        Ok(resp) => (resp.status(), resp.into_json().unwrap_or(Value::Null)),
        Err(ureq::Error::Status(code, resp)) => (code, resp.into_json().unwrap_or(Value::Null)),
        Err(e) => panic!("http request failed: {e}"),
    }
}

#[test]
fn serve_health_invoke_and_rest_api() {
    let dir = data_dir("serve");
    let server = spawn_serve(&dir, &[]);
    let base = format!("http://{}", server.addr);

    // /health: ok + service name + zero count on a fresh pool
    let (status, health) = http(ureq::get(&format!("{base}/health")), None);
    assert_eq!(status, 200);
    assert_eq!(health["ok"], true);
    assert_eq!(health["service"], "memorypool");
    assert_eq!(health["count"], 0);

    // POST /invoke add
    let (status, mem) = http(
        ureq::post(&format!("{base}/invoke")),
        Some(json!({
            "tool_id": "tool-1",
            "tool": "memorypool",
            "invoked_by": "agent:test",
            "params": {"action": "add", "text": "serve mode memory", "tags": ["http"], "importance": 0.6}
        })),
    );
    assert_eq!(status, 200, "invoke add: {mem}");
    let id = mem["id"]
        .as_str()
        .expect("invoke add returns id")
        .to_string();
    assert_eq!(mem["text"], "serve mode memory");

    // POST /invoke search
    let (status, results) = http(
        ureq::post(&format!("{base}/invoke")),
        Some(json!({"params": {"action": "search", "query": "serve"}})),
    );
    assert_eq!(status, 200, "invoke search: {results}");
    assert_eq!(results.as_array().unwrap().len(), 1);
    assert_eq!(results[0]["id"], id.as_str());

    // POST /invoke list + stats
    let (status, items) = http(
        ureq::post(&format!("{base}/invoke")),
        Some(json!({"params": {"action": "list", "tag": "http"}})),
    );
    assert_eq!(status, 200);
    assert_eq!(items.as_array().unwrap().len(), 1);
    let (status, stats) = http(
        ureq::post(&format!("{base}/invoke")),
        Some(json!({"params": {"action": "stats"}})),
    );
    assert_eq!(status, 200);
    assert_eq!(stats["count"], 1);

    // POST /invoke delete (then a missing id -> 404)
    let (status, deleted) = http(
        ureq::post(&format!("{base}/invoke")),
        Some(json!({"params": {"action": "delete", "id": id}})),
    );
    assert_eq!(status, 200, "invoke delete: {deleted}");
    assert_eq!(deleted["deleted"], id.as_str());
    let (status, _) = http(
        ureq::post(&format!("{base}/invoke")),
        Some(json!({"params": {"action": "delete", "id": id}})),
    );
    assert_eq!(status, 404);

    // unknown action -> 400
    let (status, err) = http(
        ureq::post(&format!("{base}/invoke")),
        Some(json!({"params": {"action": "explode"}})),
    );
    assert_eq!(status, 400);
    assert!(err["error"].as_str().unwrap().contains("unknown action"));

    // plain REST endpoints
    let (status, mem) = http(
        ureq::post(&format!("{base}/mem")),
        Some(json!({"text": "rest memory", "tags": "rest"})),
    );
    assert_eq!(status, 201, "POST /mem: {mem}");
    let rest_id = mem["id"].as_str().unwrap().to_string();

    let (status, found) = http(
        ureq::get(&format!("{base}/mem/search?q=rest&limit=5")),
        None,
    );
    assert_eq!(status, 200);
    assert_eq!(found.as_array().unwrap().len(), 1);

    let (status, listed) = http(ureq::get(&format!("{base}/mem/list?tag=rest")), None);
    assert_eq!(status, 200);
    assert_eq!(listed.as_array().unwrap().len(), 1);

    let (status, one) = http(ureq::get(&format!("{base}/mem/{rest_id}")), None);
    assert_eq!(status, 200);
    assert_eq!(one["text"], "rest memory");

    let (status, _) = http(ureq::get(&format!("{base}/mem/does-not-exist")), None);
    assert_eq!(status, 404);

    let (status, removed) = http(ureq::delete(&format!("{base}/mem/{rest_id}")), None);
    assert_eq!(status, 200);
    assert_eq!(removed["deleted"], rest_id.as_str());

    // health count reflects activity
    let (_, health) = http(ureq::get(&format!("{base}/health")), None);
    assert_eq!(health["count"], 0);

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn serve_token_auth_gates_everything_but_health() {
    let dir = data_dir("serve-token");
    let server = spawn_serve(&dir, &["--token", "s3cret"]);
    let base = format!("http://{}", server.addr);

    // /health stays open
    let (status, health) = http(ureq::get(&format!("{base}/health")), None);
    assert_eq!(status, 200);
    assert_eq!(health["ok"], true);

    // every other endpoint rejects missing/wrong credentials
    let (status, _) = http(
        ureq::post(&format!("{base}/mem")),
        Some(json!({"text": "nope"})),
    );
    assert_eq!(status, 401);
    let (status, _) = http(ureq::get(&format!("{base}/mem/list")), None);
    assert_eq!(status, 401);
    let (status, _) = http(
        ureq::get(&format!("{base}/mem/list")).set("Authorization", "Bearer wrong"),
        None,
    );
    assert_eq!(status, 401);
    let (status, _) = http(
        ureq::post(&format!("{base}/invoke")),
        Some(json!({"params": {"action": "stats"}})),
    );
    assert_eq!(status, 401);

    // the correct Bearer token works
    let auth = ureq::post(&format!("{base}/mem")).set("Authorization", "Bearer s3cret");
    let (status, mem) = http(auth, Some(json!({"text": "authenticated memory"})));
    assert_eq!(status, 201, "POST /mem with token: {mem}");
    let id = mem["id"].as_str().unwrap().to_string();

    let (status, results) = http(
        ureq::post(&format!("{base}/invoke")).set("Authorization", "Bearer s3cret"),
        Some(json!({"params": {"action": "search", "query": "authenticated"}})),
    );
    assert_eq!(status, 200);
    assert_eq!(results.as_array().unwrap().len(), 1);

    let (status, _) = http(
        ureq::delete(&format!("{base}/mem/{id}")).set("Authorization", "Bearer s3cret"),
        None,
    );
    assert_eq!(status, 200);

    std::fs::remove_dir_all(&dir).ok();
}
