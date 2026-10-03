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
