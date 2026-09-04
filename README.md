# MemoryPool

A shared, persistent memory pool for AI agents — built around the BIT ecosystem.

[![Release](https://img.shields.io/github/v/release/yxpil/MemoryPool?style=flat-square)](https://github.com/yxpil/MemoryPool/releases/latest)
[![Downloads](https://img.shields.io/github/downloads/yxpil/MemoryPool/total?style=flat-square)](https://github.com/yxpil/MemoryPool/releases)
[![License](https://img.shields.io/badge/license-Apache--2.0-black?style=flat-square)](LICENSE)
[![CI](https://img.shields.io/github/actions/workflow/status/yxpil/MemoryPool/ci.yml?style=flat-square&label=CI)](https://github.com/yxpil/MemoryPool/actions/workflows/ci.yml)
[![Platform](https://img.shields.io/badge/platform-macOS%20%7C%20Linux%20%7C%20Windows-black?style=flat-square)](https://github.com/yxpil/MemoryPool/releases)

---

## About

MemoryPool gives AI agents a **cross-process, persistent, shared memory**. It is a
stand-alone companion to [BIT](https://github.com/yxpil/bit) — independent of BIT's
own `memories.json` — that any number of agents can use as a central memory service
through a CLI, an HTTP API, or BIT's Remote-tool protocol.

Memories are stored in a JSONL file (`~/.memorypool/memories.jsonl` by default,
override the directory with `MEMORYPOOL_DATA_DIR`). Each memory carries:

| Field | Type | Notes |
|---|---|---|
| `id` | uuid | stable identifier |
| `text` | string | the memory content |
| `tags` | string[] | free-form labels |
| `importance` | float 0.0–1.0 | default `0.5` |
| `source` | string | origin, default `"cli"` (use `"bit"` from agents) |
| `created_at` / `updated_at` | RFC 3339 | UTC timestamps |

Writes are durable: `add` appends one JSON line, mutations rewrite the file
atomically (temp file + rename).

## Features

- **CLI first**: `add / search / list / get / delete / tag / stats / serve`, every
  subcommand supports `--json`.
- **Keyword search with scoring**: term matches in text, exact tag hits,
  importance weighting and exponential time decay (30-day half-life), ranked output.
- **stdin contract**: when stdin is a pipe, a JSON object is read and its fields
  override CLI arguments (stdin wins) — exactly what BIT's exec runtime sends.
- **HTTP server** (`serve`, default `127.0.0.1:8751`): REST endpoints, a BIT
  Remote-tool `/invoke` endpoint and MCP (Streamable HTTP, `/mcp`), optional
  `--token` Bearer auth.
- **Atomic, durable storage**: append + temp-file/rename on JSONL, malformed lines
  are skipped with a warning instead of breaking the pool.
- **Single static binary**, no runtime dependencies (Rust, edition 2021).

## Install

**Prebuilt binaries** — grab the archive for your platform from the
[latest release](https://github.com/yxpil/MemoryPool/releases/latest), unpack it and
put `memorypool` on your `PATH`:

| Platform | Archive |
|---|---|
| Linux x64 | `memorypool-v0.2.0-x86_64-unknown-linux-gnu.tar.gz` |
| macOS Apple Silicon | `memorypool-v0.2.0-aarch64-apple-darwin.tar.gz` |
| macOS Intel | `memorypool-v0.2.0-x86_64-apple-darwin.tar.gz` |
| Windows x64 | `memorypool-v0.2.0-x86_64-pc-windows-msvc.zip` |

**Build from source**:

```bash
cargo install --git https://github.com/yxpil/MemoryPool
# or clone + cargo install --path .
memorypool --help
```

## Quick start

```bash
memorypool add "Deploy redis cluster on Friday" --tags ops,infra --importance 0.9
memorypool search "redis deploy" --limit 5
memorypool list --tag ops --limit 10
memorypool stats
memorypool get <ID>
memorypool tag <ID> --add urgent --remove infra
memorypool delete <ID>

# piped stdin (BIT exec-mode contract): JSON overrides CLI args
echo '{"text":"note from another agent","tags":["agent"],"source":"bit"}' | memorypool add
echo '{"query":"redis","limit":3}' | memorypool search

# start the HTTP server (127.0.0.1:8751)
memorypool serve
memorypool serve --host 127.0.0.1 --port 8751 --token s3cret   # with auth
```

## BIT Integration

MemoryPool plugs into [BIT](https://github.com/yxpil/bit) in all three supported
ways. Pick any subset.

### 1) CLI via a BIT "exec" runtime

On BIT's **Tools page → Runtimes**, add an `exec` runtime that points at the
`memorypool` binary (`mode: exec`, e.g. `/usr/local/bin/memorypool`). Then create an
Interpreter tool whose `code` is the subcommand — BIT pipes `params` as JSON to
stdin and expects JSON on stdout, which `memorypool` implements natively.

Example: a search tool with `code = "search"`, invoked by the AI with
`params = {"query": "...", "tag": "...", "limit": 5}`:

```json
{
  "id": "mem.search",
  "name": "memory_search",
  "description": "Search the shared agent memory pool (keyword + tag + importance + recency ranking)",
  "parameters": {
    "type": "object",
    "properties": {
      "query": { "type": "string", "description": "Search keywords" },
      "tag":   { "type": "string",  "description": "Only memories with this tag (optional)" },
      "limit": { "type": "integer", "description": "Max results, default 10 (optional)" }
    },
    "required": ["query"]
  },
  "kind": { "kind": "interpreter", "runtime": "memorypool", "code": "search" },
  "created_by": "user",
  "created_at": "2026-01-01 00:00:00",
  "enabled": true
}
```

An add tool works the same way — `code = "add"`, stdin
`{"text": "...", "tags": ["a"], "importance": 0.8, "source": "bit"}`.

### 2) Remote tool via `POST /invoke`

Start the server once (e.g. in a terminal or a launch agent):

```bash
memorypool serve --host 127.0.0.1 --port 8751
# optional auth: memorypool serve --token s3cret
```

Register it as a BIT Remote tool (BIT's HTTP API listens on `127.0.0.1:8600` by
default; set `Authorization: Bearer <client_key>` with BIT's configured key):

```bash
curl -X POST http://127.0.0.1:8600/api/tools \
  -H "Authorization: Bearer <client_key>" \
  -H "Content-Type: application/json" \
  -d '{
    "kind": "remote",
    "name": "memorypool",
    "description": "Shared agent memory pool. params: {action: add|search|list|delete|stats, text, query, id, tag, limit, tags, importance}",
    "parameters": {
      "type": "object",
      "properties": {
        "action":     { "type": "string", "enum": ["add", "search", "list", "delete", "stats"] },
        "text":       { "type": "string",  "description": "Memory text (action=add)" },
        "query":      { "type": "string",  "description": "Search keywords (action=search)" },
        "id":         { "type": "string",  "description": "Memory id (action=delete)" },
        "tag":        { "type": "string",  "description": "Tag filter (search/list)" },
        "limit":      { "type": "integer", "description": "Max results (search/list)" },
        "tags":       { "type": "array",   "description": "Tags (action=add)" },
        "importance": { "type": "number",  "description": "0.0-1.0 (action=add)" }
      },
      "required": ["action"]
    },
    "url": "http://127.0.0.1:8751/invoke"
  }'
```

BIT then POSTs `{"tool_id": "...", "tool": "memorypool", "invoked_by": "agent:...",
"params": {...}}` to `/invoke`; MemoryPool routes on `params.action`
(`add` / `search` / `list` / `get` / `delete` / `stats`) and answers with the
corresponding JSON. Unknown actions get HTTP 400.

### 3) MCP client (since 0.2.0)

`memorypool serve` is also a full MCP server (Streamable HTTP JSON-RPC,
compatible with BIT's built-in MCP client, same wire contract as
SECFORGE/PANOPTES): `initialize` handshake + `Mcp-Session-Id`, `tools/list`,
`tools/call` (failures return as `isError:true` results, not transport errors),
`ping`, protocol versions `2024-11-05` / `2025-03-26` / `2025-06-18`. Six tools
are exposed: `add`, `search`, `list`, `get`, `delete`, `stats`. A configured
`--token` protects the MCP endpoints too.

```bash
# add the endpoint on BIT's MCP servers page
http://127.0.0.1:8751/mcp
```

### 4) Direct HTTP API

```bash
curl http://127.0.0.1:8751/health
curl -X POST http://127.0.0.1:8751/mem \
  -H "Content-Type: application/json" \
  -d '{"text":"remember this","tags":["x"],"importance":0.8,"source":"bit"}'
curl "http://127.0.0.1:8751/mem/search?q=remember&tag=x&limit=5"
curl "http://127.0.0.1:8751/mem/list?limit=10"
curl http://127.0.0.1:8751/mem/<id>
curl -X DELETE http://127.0.0.1:8751/mem/<id>
curl -X POST http://127.0.0.1:8751/invoke -H "Content-Type: application/json" \
  -d '{"params":{"action":"stats"}}'
```

With `--token <t>`, add `-H "Authorization: Bearer <t>"` to every call except
`/health`.

## API

| Method | Path | Description |
|---|---|---|
| GET | `/health` | `{"ok":true,"service":"memorypool","count":N}` |
| POST | `/mem` | create a memory (body: `text` required, `tags`, `importance`, `source`) → 201 |
| GET | `/mem/search?q=&tag=&limit=` | ranked search results (each with `score`) |
| GET | `/mem/list?limit=&tag=` | memories, newest first |
| GET | `/mem/{id}` | one memory (404 if missing) |
| DELETE | `/mem/{id}` | `{"deleted":"<id>"}` (404 if missing) |
| POST | `/invoke` | BIT Remote protocol: routes on `params.action` = `add`/`search`/`list`/`get`/`delete`/`stats`; unknown → 400 |
| POST | `/` and `/mcp` | MCP JSON-RPC 2.0 (Streamable HTTP): `initialize`, `tools/list`, `tools/call`, `ping` — six tools (`add`/`search`/`list`/`get`/`delete`/`stats`), `isError` results for failures; covered by `--token` auth |

CLI equivalents exist for everything (`memorypool --help`), and all CLI output is
pure JSON whenever `--json` is passed or stdin carries the BIT exec-mode JSON.

---

## 中文

## 简介

MemoryPool 为 AI 智能体提供**跨进程、可持久化的共享记忆**。它是
[BIT](https://github.com/yxpil/bit) 的独立卫星工具，不依赖 BIT 自带的
`memories.json`，可作为多个智能体共享的中央记忆服务，通过 CLI、HTTP API 或
BIT Remote 工具协议、MCP 客户端四种方式接入。

记忆默认存储在 `~/.memorypool/memories.jsonl`（可用环境变量
`MEMORYPOOL_DATA_DIR` 覆盖目录）。每条记忆包含：

| 字段 | 类型 | 说明 |
|---|---|---|
| `id` | uuid | 稳定标识 |
| `text` | 字符串 | 记忆内容 |
| `tags` | 字符串数组 | 自由标签 |
| `importance` | 浮点 0.0–1.0 | 重要程度，默认 `0.5` |
| `source` | 字符串 | 来源，默认 `"cli"`（智能体建议用 `"bit"`） |
| `created_at` / `updated_at` | RFC 3339 | UTC 时间戳 |

写入持久可靠：`add` 追加一行 JSON，修改/删除通过临时文件 + rename 原子落盘。

## 功能

- **CLI 优先**：`add / search / list / get / delete / tag / stats / serve`，全部支持 `--json`。
- **关键词评分检索**：正文命中、标签精确命中、重要程度加权、时间指数衰减
  （半衰期 30 天），按综合得分排序输出。
- **stdin 契约**：stdin 为管道时读取 JSON 对象，字段优先于命令行参数
  （stdin 优先）——正是 BIT exec 运行时的调用方式。
- **HTTP 服务**（`serve`，默认 `127.0.0.1:8751`）：REST 端点 + BIT Remote 工具
  `/invoke` 端点 + MCP（Streamable HTTP，`/mcp`），可选 `--token` Bearer 鉴权。
- **原子可靠存储**：JSONL 追加写 + 临时文件/rename 原子落盘；损坏行跳过并警告，
  不影响整个记忆池。
- **单一静态二进制**，无运行时依赖（Rust, edition 2021）。

## 安装

**预编译二进制** —— 从[最新 Release](https://github.com/yxpil/MemoryPool/releases/latest)
下载对应平台压缩包，解压后将 `memorypool` 放入 `PATH`：

| 平台 | 文件 |
|---|---|
| Linux x64 | `memorypool-v0.2.0-x86_64-unknown-linux-gnu.tar.gz` |
| macOS Apple Silicon | `memorypool-v0.2.0-aarch64-apple-darwin.tar.gz` |
| macOS Intel | `memorypool-v0.2.0-x86_64-apple-darwin.tar.gz` |
| Windows x64 | `memorypool-v0.2.0-x86_64-pc-windows-msvc.zip` |

**源码构建**：

```bash
cargo install --git https://github.com/yxpil/MemoryPool
# 或克隆后 cargo install --path .
memorypool --help
```

## 快速上手

```bash
memorypool add "周五部署 redis 集群" --tags ops,infra --importance 0.9
memorypool search "redis 部署" --limit 5
memorypool list --tag ops --limit 10
memorypool stats
memorypool get <ID>
memorypool tag <ID> --add urgent --remove infra
memorypool delete <ID>

# 管道 stdin（BIT exec 模式契约）：JSON 字段优先于命令行参数
echo '{"text":"来自另一个智能体的备忘","tags":["agent"],"source":"bit"}' | memorypool add
echo '{"query":"redis","limit":3}' | memorypool search

# 启动 HTTP 服务（127.0.0.1:8751）
memorypool serve
memorypool serve --host 127.0.0.1 --port 8751 --token s3cret   # 带鉴权
```

## BIT 集成

MemoryPool 以 [BIT](https://github.com/yxpil/bit) 支持的全部四种方式接入，
可任选组合使用。

### 1) CLI：通过 BIT「exec 运行时」

在 BIT 的**工具页 → 运行时**中添加一个 exec 运行时，可执行路径指向
`memorypool` 二进制（`mode: exec`，如 `/usr/local/bin/memorypool`）。然后创建
Interpreter 工具，`code` 填子命令——BIT 会把 `params` 以 JSON 写入 stdin、
期望 stdout 返回 JSON，`memorypool` 原生满足该契约。

示例：检索工具 `code = "search"`，AI 以
`params = {"query": "...", "tag": "...", "limit": 5}` 调用：

```json
{
  "id": "mem.search",
  "name": "memory_search",
  "description": "搜索共享记忆池（关键词 + 标签 + 重要度 + 时间衰减排序）",
  "parameters": {
    "type": "object",
    "properties": {
      "query": { "type": "string", "description": "搜索关键词" },
      "tag":   { "type": "string",  "description": "仅检索含该标签的记忆（可选）" },
      "limit": { "type": "integer", "description": "最大返回条数，默认 10（可选）" }
    },
    "required": ["query"]
  },
  "kind": { "kind": "interpreter", "runtime": "memorypool", "code": "search" },
  "created_by": "user",
  "created_at": "2026-01-01 00:00:00",
  "enabled": true
}
```

写入工具同理 —— `code = "add"`，stdin 传
`{"text": "...", "tags": ["a"], "importance": 0.8, "source": "bit"}`。

### 2) Remote 工具：`POST /invoke`

先启动服务（终端或开机自启均可）：

```bash
memorypool serve --host 127.0.0.1 --port 8751
# 可选鉴权：memorypool serve --token s3cret
```

把它注册为 BIT 的 Remote 工具（BIT HTTP API 默认监听 `127.0.0.1:8600`，
`Authorization: Bearer <client_key>` 使用 BIT 配置的 Client Key）：

```bash
curl -X POST http://127.0.0.1:8600/api/tools \
  -H "Authorization: Bearer <client_key>" \
  -H "Content-Type: application/json" \
  -d '{
    "kind": "remote",
    "name": "memorypool",
    "description": "共享智能体记忆池。params: {action: add|search|list|delete|stats, text, query, id, tag, limit, tags, importance}",
    "parameters": {
      "type": "object",
      "properties": {
        "action":     { "type": "string", "enum": ["add", "search", "list", "delete", "stats"] },
        "text":       { "type": "string",  "description": "记忆内容（action=add）" },
        "query":      { "type": "string",  "description": "搜索关键词（action=search）" },
        "id":         { "type": "string",  "description": "记忆 id（action=delete）" },
        "tag":        { "type": "string",  "description": "标签过滤（search/list）" },
        "limit":      { "type": "integer", "description": "最大返回条数（search/list）" },
        "tags":       { "type": "array",   "description": "标签（action=add）" },
        "importance": { "type": "number",  "description": "0.0-1.0（action=add）" }
      },
      "required": ["action"]
    },
    "url": "http://127.0.0.1:8751/invoke"
  }'
```

之后 BIT 会向 `/invoke` POST
`{"tool_id": "...", "tool": "memorypool", "invoked_by": "agent:...", "params": {...}}`；
MemoryPool 按 `params.action` 路由（`add` / `search` / `list` / `get` /
`delete` / `stats`），响应对应 JSON，未知 action 返回 HTTP 400。

### 3) MCP 客户端（0.2.0 新增）

`memorypool serve` 同时是一个完整的 MCP 服务器（Streamable HTTP JSON-RPC，
与 BIT 内置 MCP 客户端兼容，协议契约与 SECFORGE/PANOPTES 一致）：
`initialize` 握手 + `Mcp-Session-Id`、`tools/list`、`tools/call`（失败以
`isError:true` 结果返回，而非传输层错误）、`ping`，支持 `2024-11-05` /
`2025-03-26` / `2025-06-18` 三个协议版本。暴露 6 个工具：`add`、`search`、
`list`、`get`、`delete`、`stats`。配置了 `--token` 时 MCP 端点同样受保护。

```bash
# 在 BIT → MCP 服务器 页面添加该端点
http://127.0.0.1:8751/mcp
```

### 4) 直接调用 HTTP API

```bash
curl http://127.0.0.1:8751/health
curl -X POST http://127.0.0.1:8751/mem \
  -H "Content-Type: application/json" \
  -d '{"text":"记住这件事","tags":["x"],"importance":0.8,"source":"bit"}'
curl "http://127.0.0.1:8751/mem/search?q=记住&tag=x&limit=5"
curl "http://127.0.0.1:8751/mem/list?limit=10"
curl http://127.0.0.1:8751/mem/<id>
curl -X DELETE http://127.0.0.1:8751/mem/<id>
curl -X POST http://127.0.0.1:8751/invoke -H "Content-Type: application/json" \
  -d '{"params":{"action":"stats"}}'
```

若以 `--token <t>` 启动，除 `/health` 外的所有请求都需加
`-H "Authorization: Bearer <t>"`。

## API

| 方法 | 路径 | 说明 |
|---|---|---|
| GET | `/health` | `{"ok":true,"service":"memorypool","count":N}` |
| POST | `/mem` | 新增记忆（body：`text` 必填，`tags`、`importance`、`source` 可选）→ 201 |
| GET | `/mem/search?q=&tag=&limit=` | 评分检索结果（含 `score` 字段） |
| GET | `/mem/list?limit=&tag=` | 按时间倒序列出记忆 |
| GET | `/mem/{id}` | 查询单条（不存在返回 404） |
| DELETE | `/mem/{id}` | `{"deleted":"<id>"}`（不存在返回 404） |
| POST | `/invoke` | BIT Remote 协议：按 `params.action` 路由 `add`/`search`/`list`/`get`/`delete`/`stats`；未知 action 返回 400 |
| POST | `/` 与 `/mcp` | MCP JSON-RPC 2.0（Streamable HTTP）：`initialize`、`tools/list`、`tools/call`、`ping`，6 个工具；失败以 `isError` 返回；`--token` 鉴权同样覆盖 |

所有 HTTP 端点都有等价的 CLI 子命令（`memorypool --help`），且只要传入
`--json`（或 stdin 携带 BIT exec 模式 JSON）即输出纯 JSON。

## 安全与合规

- 默认只绑定 `127.0.0.1`，不暴露公网；跨机共享请自行加反向代理与 TLS。
- 公网或多人环境务必启用 `--token`，所有端点（除 `/health`）校验
  `Authorization: Bearer <token>`。
- 数据全部落在本机 `~/.memorypool/memories.jsonl`，不上传任何数据。
- 请勿在共享记忆池中存入密钥、密码等敏感信息。

---

Part of the [BIT](https://github.com/yxpil/bit) ecosystem · License: [Apache-2.0](LICENSE)
