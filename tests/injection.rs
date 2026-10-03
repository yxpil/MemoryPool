//! Injection / robustness tests for `memorypool serve` (black-box, over loopback).
//!
//! The store is a plain JSONL file matched by exact string id — there is no SQL,
//! no shell-out and no file path ever derived from a request id. These tests
//! prove hostile input is stored as opaque data or rejected cleanly rather than
//! executed:
//!   - malformed / non-object bodies are 400;
//!   - XSS / shell payloads round-trip verbatim as JSON (never rendered, never
//!     expanded);
//!   - path-traversal ids in `/mem/{id}` resolve to 404, never a file read;
//!   - regex / glob metacharacters in a search query are literal substrings;
//!   - SQL-style guesses do not defeat the bearer-token gate.

use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use serde_json::Value;

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
        "memorypool-inj-it-{}-{name}",
        uuid::Uuid::new_v4().simple()
    ));
    std::fs::create_dir_all(&dir).expect("failed to create temp data dir");
    dir
}

fn spawn_serve(dir: &PathBuf, token: Option<&str>) -> ServeHandle {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_memorypool"));
    cmd.args(["serve", "--port", "0"]).env("MEMORYPOOL_DATA_DIR", dir);
    if let Some(t) = token {
        cmd.args(["--token", t]);
    }
    let mut child = cmd
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

#[test]
fn hostile_text_round_trips_verbatim_as_json() {
    let dir = data_dir("roundtrip");
    let server = spawn_serve(&dir, None);
    let addr = &server.addr;

    let payload = r#"{"text":"<script>alert(document.cookie)</script>; rm -rf /","tags":["x"]}"#;
    let resp = ureq::post(&format!("http://{addr}/mem"))
        .set("Content-Type", "application/json")
        .send_string(payload)
        .expect("create");
    assert_eq!(resp.status(), 201);
    let mem: Value = resp.into_json().expect("json");
    let id = mem["id"].as_str().expect("id").to_string();
    // Stored verbatim — no HTML escaping, no shell expansion.
    assert_eq!(
        mem["text"],
        "<script>alert(document.cookie)</script>; rm -rf /"
    );

    // GET returns the same string byte-for-byte.
    let got: Value = ureq::get(&format!("http://{addr}/mem/{id}"))
        .call()
        .expect("get")
        .into_json()
        .expect("json");
    assert_eq!(got["text"], mem["text"]);
}

#[test]
fn malformed_and_non_object_bodies_are_400() {
    let dir = data_dir("badbody");
    let server = spawn_serve(&dir, None);
    let addr = &server.addr;

    for body in ["not json", r#"[1,2,3]"#, r#"42"#, r#""just a string""#] {
        let err = ureq::post(&format!("http://{addr}/mem"))
            .set("Content-Type", "application/json")
            .send_string(body)
            .expect_err("must be a client error");
        match err {
            ureq::Error::Status(400, r) => {
                let v: Value = r.into_json().unwrap_or(Value::Null);
                assert!(v["error"].is_string(), "body: {body}");
            }
            other => panic!("expected 400 for {body:?}, got {other:?}"),
        }
    }
}

#[test]
fn path_traversal_id_is_404_not_a_file_read() {
    let dir = data_dir("traversal");
    let server = spawn_serve(&dir, None);
    let addr = &server.addr;

    for id in ["..%2F..%2Fetc%2Fpasswd", "%2E%2E%2Fsecret.db", "....//....//win.ini"] {
        // GET must 404 — the id is matched against in-memory memories, never used
        // to open a file path.
        let get_err = ureq::get(&format!("http://{addr}/mem/{id}"))
            .call()
            .expect_err("traversal get must 404");
        assert!(
            matches!(get_err, ureq::Error::Status(404, _)),
            "GET {id} must 404"
        );
        // DELETE likewise.
        let del_err = ureq::delete(&format!("http://{addr}/mem/{id}"))
            .call()
            .expect_err("traversal delete must 404");
        assert!(
            matches!(del_err, ureq::Error::Status(404, _)),
            "DELETE {id} must 404"
        );
    }
}

#[test]
fn search_treats_regex_glob_metachars_as_literal() {
    let dir = data_dir("search");
    let server = spawn_serve(&dir, None);
    let addr = &server.addr;

    // A normal memory.
    ureq::post(&format!("http://{addr}/mem"))
        .set("Content-Type", "application/json")
        .send_string(r#"{"text":"hello world redis","tags":[]}"#)
        .expect("add");

    // ".*" as a regex would match everything; as a literal substring it matches
    // nothing (the text contains no literal ".*").
    let r: Value = ureq::get(&format!("http://{addr}/mem/search?q=.*"))
        .call()
        .expect("search")
        .into_json()
        .expect("json");
    assert!(
        r.as_array().expect("results").is_empty(),
        "regex .* must be treated as a literal substring"
    );

    // "%" and "_" (SQL LIKE wildcards) likewise match nothing here.
    let r: Value = ureq::get(&format!("http://{addr}/mem/search?q=%25"))
        .call()
        .expect("search")
        .into_json()
        .expect("json");
    assert!(r.as_array().expect("results").is_empty());

    // The real term still works.
    let r: Value = ureq::get(&format!("http://{addr}/mem/search?q=redis"))
        .call()
        .expect("search")
        .into_json()
        .expect("json");
    assert_eq!(r.as_array().expect("results").len(), 1);
}

#[test]
fn sql_like_guesses_do_not_defeat_the_bearer_token() {
    let dir = data_dir("token");
    let server = spawn_serve(&dir, Some("secret"));
    let addr = &server.addr;

    for guess in ["' OR 1=1 --", "\" OR \"\"=\"", "secret' --", "admin"] {
        let err = ureq::post(&format!("http://{addr}/mem"))
            .set("Content-Type", "application/json")
            .set("Authorization", &format!("Bearer {guess}"))
            .send_string(r#"{"text":"x"}"#)
            .expect_err("guess must be unauthorized");
        assert!(
            matches!(err, ureq::Error::Status(401, _)),
            "guess {guess:?} must 401"
        );
    }
    // Control: the exact token works.
    let resp = ureq::post(&format!("http://{addr}/mem"))
        .set("Content-Type", "application/json")
        .set("Authorization", "Bearer secret")
        .send_string(r#"{"text":"allowed"}"#)
        .expect("authorized create");
    assert_eq!(resp.status(), 201);
}
