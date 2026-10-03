# MemoryPool 测试说明
- 测试完成：是（2026-10-04）
- 测试日期：2026-10-04
- 测试内容：单元覆盖 model(serde 默认值/importance 钳制)、store(JSONL 追加·原子重写·坏行跳过)、search(时间衰减·打分)、params(宽松强转)、mcp(注册表)；集成以黑盒方式起真实二进制覆盖 cli/REST/MCP；注入测试覆盖 HTTP 层（敌对文本经 POST/GET 原样 JSON 往返、畸形/非对象 body 400、URL 编码路径穿越 id 404 且不读文件、正则/通配符查询按字面子串处理、SQL 式 token 猜测 401）。无运行时钩子/插件/事件机制。
- 运行命令：`cargo test`（仓库根）
- 测试框架：Rust `#[cfg(test)]` + `cargo test`（无 lib target，集成测试位于 `tests/`，黑盒驱动二进制）
- 模型：豆包（Doubao）生成

# Testing MemoryPool

MemoryPool is a persistent memory pool (JSONL on disk + relevance-scored search)
exposed as a CLI, a REST API and an MCP JSON-RPC endpoint. It ships **no lib
target** — every test drives the compiled binary as a black box against an
isolated `MEMORYPOOL_DATA_DIR`, so the real store and HTTP layer are exercised.
All tests run under `cargo test`.

| Layer | Location | What it covers |
|-------|----------|----------------|
| Unit | `src/model.rs`, `src/store.rs`, `src/search.rs`, `src/params.rs`, `src/mcp.rs` (`#[cfg(test)]`) | Memory serde defaults / clamping, JSONL append + atomic rewrite, malformed-line skipping, recency decay & scoring, lenient param coercion. |
| CLI / REST | `tests/cli.rs` | Binary end-to-end: `add`/`search`/`list`/`get`/`delete`/`stats`, piped-stdin merge, token gate, REST routes. |
| MCP | `tests/mcp.rs` | Streamable-HTTP MCP handshake, `tools/list`, `tools/call` round-trip, JSON-RPC error codes, token gate. |
| Injection / robustness | `tests/injection.rs` | Untrusted-input security — see below. |

## Run

```sh
cargo test
```

Each integration test spawns the real binary on an ephemeral loopback port with a
fresh temp data directory (`MEMORYPOOL_DATA_DIR`). No real network or home
directory is touched (the default `~/.memorypool` store is never used).

## What the injection tests assert

The store is a single `memories.jsonl` file; memories are matched by exact
string id (never used as a file path), search is literal substring matching
(no regex, no SQL). `tests/injection.rs` proves hostile input is stored as
opaque data or rejected cleanly rather than executed:

- XSS / shell payloads in a memory `text` round-trip **byte-for-byte** through
  `POST /mem` and `GET /mem/{id}` as JSON — never rendered, never expanded;
- malformed JSON, a JSON array / number / string as the body → HTTP 400;
- path-traversal ids (`../../etc/passwd` URL-encoded) in `GET`/`DELETE
  /mem/{id}` → 404, never a file read;
- regex / glob metacharacters (`.*`, `%`) in a search query are treated as literal
  substrings — `q=.*` does **not** match everything;
- SQL-style bearer-token guesses (`' OR 1=1 --`, …) → 401, while the exact token
  still succeeds (control).

## Hook / plugin tests

MemoryPool exposes a **static, compile-time** tool registry (the six MCP tools
are fixed); there is no user-registerable hook, plugin, event bus or callback.
There are therefore **0 hook tests**.

## Expected result

`cargo test` should finish green. As of this document the suites are:
unit 27, `cli.rs` 6, `mcp.rs` 3, `injection.rs` 5 (passing:failing = 41:0).
