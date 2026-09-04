# MemoryPool Wiki / MemoryPool 维基

**MemoryPool** — A shared, persistent memory pool for AI agents around the [BIT](https://github.com/yxpil/bit) ecosystem: cross-process memory storage with keyword scoring search, importance weighting and time decay, usable from a CLI, an HTTP API or BIT's Remote-tool protocol.

**MemoryPool** —— 面向 [BIT](https://github.com/yxpil/bit) 生态 AI 智能体的共享、可持久化记忆池：跨进程记忆存储，支持关键词评分检索、重要度加权与时间衰减，可通过 CLI、HTTP API 或 BIT Remote 工具协议接入。

- Repo / 仓库: <https://github.com/yxpil/MemoryPool>
- Releases / 发行版: <https://github.com/yxpil/MemoryPool/releases>
- Binary name / 二进制名: `memorypool`
- Default port / 默认端口: `8751`
- Data dir / 数据目录: `~/.memorypool/memories.jsonl`（env / 环境变量 `MEMORYPOOL_DATA_DIR` 可覆盖）

---

## Install / 安装

Grab a per-platform binary from the [latest release](https://github.com/yxpil/MemoryPool/releases/latest) (`tar.gz` for macOS/Linux, `zip` for Windows), or:

从 [最新 Release](https://github.com/yxpil/MemoryPool/releases/latest) 下载对应平台二进制（macOS/Linux 为 `tar.gz`，Windows 为 `zip`），或：

```bash
cargo install --git https://github.com/yxpil/MemoryPool
```

## Usage / 使用

```bash
memorypool add "Deploy redis cluster on Friday" --tags ops,infra --importance 0.9
memorypool search "redis deploy" --limit 5     # ranked by score / 按得分排序
memorypool list --tag ops --limit 10           # newest first / 按时间倒序
memorypool get <ID>
memorypool tag <ID> --add urgent --remove infra
memorypool delete <ID>
memorypool stats

# piped stdin (BIT exec-mode contract): JSON overrides CLI args
# 管道 stdin（BIT exec 模式契约）：JSON 字段优先于命令行参数
echo '{"text":"note from another agent","tags":["agent"],"source":"bit"}' | memorypool add
echo '{"query":"redis","limit":3}' | memorypool search

# start the HTTP server (127.0.0.1:8751) / 启动 HTTP 服务
memorypool serve
memorypool serve --host 127.0.0.1 --port 8751 --token s3cret   # with auth / 带鉴权
```

## Memory shape / 记忆结构

| Field / 字段 | Type / 类型 | Notes / 说明 |
|---|---|---|
| `id` | uuid | stable identifier / 稳定标识 |
| `text` | string | the memory content / 记忆内容 |
| `tags` | string[] | free-form labels / 自由标签 |
| `importance` | float 0.0–1.0 | default `0.5` / 重要程度，默认 0.5 |
| `source` | string | default `"cli"` (use `"bit"` from agents) / 来源，智能体建议用 `"bit"` |
| `created_at` / `updated_at` | RFC 3339 | UTC timestamps / UTC 时间戳 |

Writes are durable: `add` appends one JSON line; mutations rewrite the file atomically (temp file + rename). Malformed lines are skipped with a warning instead of breaking the pool.

写入持久可靠：`add` 追加一行 JSON；修改/删除通过临时文件 + rename 原子落盘。损坏行跳过并警告，不影响整个记忆池。

## Search scoring / 检索评分

`search` ranks by / 检索按以下综合得分排序：

- term matches in `text` / 正文关键词命中
- exact tag hits / 标签精确命中
- importance weighting / 重要程度加权
- exponential time decay (30-day half-life) / 时间指数衰减（半衰期 30 天）

## BIT integration / BIT 集成

Three ways / 三种方式:

1. **CLI (exec runtime)** — On BIT's Tools page add an `exec` runtime pointing at `memorypool`; create an Interpreter tool whose `code` is the subcommand (e.g. `search`). BIT pipes `params` as JSON to stdin and reads JSON from stdout — the native contract. / 在 BIT 工具页添加 exec 运行时，Interpreter 的 `code` 即子命令；BIT 通过 stdin 传 JSON、stdout 读 JSON。
2. **Remote tool** — run `memorypool serve`, register URL `http://127.0.0.1:8751/invoke` with `params.action ∈ add|search|list|delete|stats`; BIT POSTs `{"tool_id":"...","tool":"memorypool","invoked_by":"...","params":{...}}`. / 启动 serve 后注册 Remote 工具。
3. **Plain REST** — `GET /health`, `POST /mem`, `GET /mem/search`, `GET /mem/list`, `GET/DELETE /mem/{id}`, `POST /invoke` (see [Protocol](Protocol)).

Typical scenario / 典型场景: several BIT agents share one pool — each writes what it learned (`add ... --source bit`), any agent later recalls it with `search`; importance and decay keep long-term knowledge ranked above noise.

多个 BIT 智能体共用一个池：各自把经验写入（`add ... --source bit`），任何智能体随后都能 `search` 找回；重要度与时间衰减让长期知识排在噪音之上。

## FAQ

**Q: Where is the data stored? / 数据存在哪里？**
`~/.memorypool/memories.jsonl` (or `$MEMORYPOOL_DATA_DIR/memories.jsonl`). One JSON object per line, human-inspectable.

`~/.memorypool/memories.jsonl`（或 `$MEMORYPOOL_DATA_DIR/memories.jsonl`）。每行一个 JSON 对象，可直接查看。

**Q: Is MemoryPool the same as BIT's built-in memory? / 和 BIT 自带记忆是一回事吗？**
No — it is an independent companion. BIT's own `memories.json` is per-agent; MemoryPool is a central, cross-process service any number of agents share.

不是 —— 它是独立的卫星工具。BIT 自带 `memories.json` 是单智能体的；MemoryPool 是多智能体共享的中央服务。

**Q: Can two agents use the pool at the same time? / 两个智能体能同时使用吗？**
Yes — `add` appends atomically and mutations rewrite via temp-file + rename; concurrent writers do not corrupt the file.

可以 —— `add` 原子追加，修改经临时文件 + rename 落盘；并发写入不会损坏文件。

**Q: Does `delete` physically remove the line? / `delete` 会物理删除那行吗？**
Mutations rewrite the file without the deleted record, so yes — the JSONL stays consistent with the API view.

修改操作会重写文件并剔除被删记录，JSONL 与 API 视图保持一致。

**Q: How do I protect the HTTP API? / 如何保护 HTTP API？**
`memorypool serve --token s3cret`; then every call except `GET /health` needs `Authorization: Bearer s3cret`.

`memorypool serve --token s3cret`；此后除 `GET /health` 外都需要 `Authorization: Bearer s3cret`。

---

Part of the [BIT](https://github.com/yxpil/bit) ecosystem.
