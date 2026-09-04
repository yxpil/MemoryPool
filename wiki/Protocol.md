# MemoryPool HTTP Protocol / MemoryPool HTTP 协议

MemoryPool `serve` mode exposes a small HTTP API on `127.0.0.1:8751` by default (`--host/--port` overridable). All responses are JSON.

MemoryPool `serve` 模式默认在 `127.0.0.1:8751` 提供一个小型 HTTP API（可用 `--host/--port` 覆盖）。所有响应均为 JSON。

## Endpoints / 端点

| Endpoint / 端点 | Method / 方法 | Auth / 认证 | Description / 说明 |
| --- | --- | --- | --- |
| `/health` | GET | none / 无 | `{"ok":true,"service":"memorypool","count":N}` — never token-protected / 永不做 token 保护 |
| `/mem` | POST | Bearer (optional / 可选) | Create a memory → 201 / 创建记忆，返回 201 |
| `/mem/search` | GET | Bearer (optional / 可选) | `?q=&tag=&limit=` — ranked results, each with `score` / 评分排序结果 |
| `/mem/list` | GET | Bearer (optional / 可选) | `?limit=&tag=` — memories, newest first / 按时间倒序 |
| `/mem/{id}` | GET | Bearer (optional / 可选) | One memory (404 if missing) / 单条记忆，缺失返回 404 |
| `/mem/{id}` | DELETE | Bearer (optional / 可选) | `{"deleted":"<id>"}` (404 if missing) / 缺失返回 404 |
| `/invoke` | POST | Bearer (optional / 可选) | BIT Remote protocol entry / BIT Remote 协议入口 |

With `serve --token <TOKEN>`, every call except `/health` requires `Authorization: Bearer <TOKEN>` (401 otherwise). / 使用 `--token <TOKEN>` 后，除 `/health` 外都需要 `Authorization: Bearer <TOKEN>`（否则 401）。

## POST /mem

```json
{ "text": "remember this", "tags": ["x"], "importance": 0.8, "source": "bit" }
```

- `text` required / 必填；`tags` array / 数组；`importance` 0.0–1.0 (default / 默认 `0.5`)；`source` string (default / 默认 `"cli"`, agents should use / 智能体建议用 `"bit"`).
- Returns `201` with the full stored memory (including `id`, `created_at`). / 返回 201 与完整存储的记忆（含 `id`、`created_at`）。

## POST /invoke

Request body (BIT Remote payload) / 请求体（BIT Remote 载荷）:

```json
{
  "tool_id": "tool-uuid",
  "tool": "memorypool",
  "invoked_by": "bit-agent",
  "params": { "action": "search", "query": "redis deploy", "limit": 5 }
}
```

Routing reads `params.action`, falling back to `params.tool`. Valid actions / 路由读取 `params.action`（回退 `params.tool`）。合法 action：

| action | Params / 参数 | Response payload / 响应载荷 |
| --- | --- | --- |
| `add` | `text`（必填）、`tags?`、`importance?` (0.5)、`source?` | the stored memory object / 存储后的完整记忆对象 |
| `search` | `query`（必填）、`tag?`、`limit?` | `[{id, text, tags, importance, source, score, ...}]` — ranked / 评分排序 |
| `list` | `tag?`、`limit?` | `[memory...]` — newest first / 按时间倒序 |
| `delete` | `id`（必填） | `{"deleted":"<id>"}` |
| `stats` | — | `{count, ...pool statistics / 记忆池统计}` |

Errors / 错误:

| HTTP | Meaning / 含义 |
| --- | --- |
| 400 | Unknown `action`, missing/invalid param (e.g. `add` without `text`) / 未知动作、参数缺失或非法 |
| 401 | Missing or invalid Bearer token / 缺失或错误的 Bearer token |
| 404 | Memory `id` not found / 记忆不存在 |

## Search scoring / 评分规则

`search` combines / 综合以下因素排序:

- term matches in `text` / 正文关键词命中
- exact tag hits / 标签精确命中
- importance weighting / 重要程度加权
- exponential time decay with a 30-day half-life / 半衰期 30 天的时间指数衰减

## Storage / 存储

`~/.memorypool/memories.jsonl` (override dir with `MEMORYPOOL_DATA_DIR`): one JSON object per line. `add` appends; mutations rewrite atomically via temp file + rename; malformed lines are skipped with a warning, never crash the pool.

`~/.memorypool/memories.jsonl`（可用 `MEMORYPOOL_DATA_DIR` 覆盖目录）：每行一个 JSON 对象。`add` 追加写；修改经临时文件 + rename 原子落盘；损坏行跳过并警告，绝不拖垮整个池。

## Example session / 示例会话

```bash
# health
curl http://127.0.0.1:8751/health
# -> {"ok":true,"service":"memorypool","count":0}

# create
curl -X POST http://127.0.0.1:8751/mem -H 'Content-Type: application/json' \
     -d '{"text":"remember this","tags":["x"],"importance":0.8,"source":"bit"}'
# -> 201 {...}

# ranked search
curl "http://127.0.0.1:8751/mem/search?q=remember&tag=x&limit=5"

# newest first / 按时间倒序
curl "http://127.0.0.1:8751/mem/list?limit=10"

# get / delete one
curl http://127.0.0.1:8751/mem/<id>
curl -X DELETE http://127.0.0.1:8751/mem/<id>

# BIT Remote protocol
curl -X POST http://127.0.0.1:8751/invoke -H 'Content-Type: application/json' \
     -d '{"tool_id":"t1","tool":"memorypool","invoked_by":"agent","params":{"action":"stats"}}'

# token-protected server / token 保护的服务
memorypool serve --port 8751 --token sekrit
curl -H 'Authorization: Bearer sekrit' "http://127.0.0.1:8751/mem/list?limit=3"
```

## CLI parity / CLI 等价

Everything above has a CLI equivalent (`memorypool --help`); all CLI output is pure JSON with `--json` or when stdin carries the BIT exec-mode JSON.

以上全部操作都有 CLI 等价命令（`memorypool --help`）；传 `--json` 或 stdin 携带 BIT exec 模式 JSON 时，CLI 输出均为纯 JSON。
