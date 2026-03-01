# Arc 控制面 API

这份文档描述 `arc-gateway` 控制面的全部接口。控制面用于配置、集群协同和 XDP 黑白名单管理。

## 鉴权与访问边界

- 如果配置了 `control_plane.auth_token`，所有控制面接口都需要 `Authorization: Bearer <token>`。
- 如果没有配置 `control_plane.auth_token`，控制面只允许本机回环地址访问（`127.0.0.1`/`::1`），非本机来源会返回 `401`。
- 建议控制面默认绑定 `127.0.0.1`，避免直接暴露公网。

## 通用返回码

- `200`：请求成功
- `400`：请求参数或请求体不合法
- `401`：鉴权失败
- `404`：接口不存在
- `409`：配置可编译但包含需要重启才生效的参数
- `500`：服务内部错误
- `503`：当前能力未启用或运行不可用（例如 gossip/xdp 未启动）

## 配置与状态

## GET `/v1/status`
**作用**：返回当前节点角色和配置代际号。  
**请求体**：无。  
**成功响应示例**：
```json
{
  "generation": 123456789,
  "node_id": "node-a",
  "role": "leader"
}
```

## GET `/v1/config`
**作用**：返回当前生效配置原文（JSON）。  
**请求体**：无。  
**请求头**：
- 可以传 `If-None-Match: "<generation>"`，未变化时返回 `304`。  
**成功响应**：
- `200`：返回完整配置 JSON，并带 `ETag`。
- `304`：配置未变化。

## GET `/v1/config/longpoll`
**作用**：长轮询配置变更。  
**查询参数**：
- `since`：上次已知代际号。
- `timeout_ms`：本次长轮询等待时间，单位毫秒。  
**行为**：
- 如果配置已变化，立即返回 `200` 和新配置。
- 如果在等待窗口内未变化，返回 `304`。

## POST `/v1/config/validate`
**作用**：只编译配置，不应用。  
**请求体**：完整配置 JSON 文本。  
**成功响应示例**：
```json
{
  "ok": true,
  "generation": 123456789,
  "compile_ms": 8
}
```

## POST `/v1/config`
**作用**：应用本节点配置。  
**请求体**：完整配置 JSON 文本。  
**成功响应示例**：
```json
{
  "generation": 123456789,
  "scope": "local"
}
```
**冲突响应示例（需要重启）**：
```json
{
  "error": "restart required for some fields",
  "changed_params": ["workers", "listen", "io_uring.entries"]
}
```

## POST `/v1/cluster/config`
**作用**：集群配置下发。  
**请求体**：完整配置 JSON 文本。  
**行为**：
- 集群模式下会先做集群验证，再在本地应用，并触发分发逻辑。
- 失败时返回明确错误信息，便于定位是编译失败还是集群校验失败。

## 集群电路与 Gossip

## GET `/v1/cluster/circuit/local`
**作用**：读取本节点的集群熔断快照。  
**请求体**：无。

## GET `/v1/cluster/members`
**作用**：读取当前 gossip 成员列表。  
**请求体**：无。  
**备注**：gossip 未启用或未运行时返回 `503`。

## GET `/v1/cluster/gossip/stats`
**作用**：读取 gossip 统计信息。  
**请求体**：无。

## POST `/v1/cluster/gossip/join`
**作用**：让当前节点主动加入指定 peer。  
**请求体示例**：
```json
{
  "peer": "10.0.0.12:22101"
}
```

## POST `/v1/cluster/gossip/leave`
**作用**：让当前节点主动离开 gossip 集群。  
**请求体**：无。

## XDP 黑白名单管理

## GET `/v1/xdp/status`
**作用**：查询 XDP 管理器状态。  
**请求体**：无。  
**成功响应示例**：
```json
{
  "mode": "xdp_skb",
  "interface": "eth0",
  "pin_base": "/sys/fs/bpf/arc",
  "kernel_release": "6.6.87.2-microsoft-standard-WSL2",
  "program_version": "v1",
  "blacklist_capacity": 65536,
  "whitelist_capacity": 65536
}
```

## GET `/v1/xdp/blacklist`
**作用**：列出黑名单。  
**查询参数**：
- `max`：最多返回条数，默认 `1024`，范围 `1..65536`。

## POST `/v1/xdp/blacklist`
**作用**：添加黑名单条目。  
**请求体示例**：
```json
{
  "ip": "203.0.113.7/32",
  "ttl_ms": 600000,
  "reason": "manual"
}
```
`ttl_ms` 不传时默认 `600000`。

## DELETE `/v1/xdp/blacklist`
**作用**：删除黑名单条目。  
**请求体示例**：
```json
{
  "ip": "203.0.113.7/32"
}
```

## GET `/v1/xdp/whitelist`
**作用**：列出白名单。  
**查询参数**：
- `max`：最多返回条数，默认 `1024`，范围 `1..65536`。

## POST `/v1/xdp/whitelist`
**作用**：添加白名单条目。  
**请求体示例**：
```json
{
  "ip": "198.51.100.10/32"
}
```

## DELETE `/v1/xdp/whitelist`
**作用**：删除白名单条目。  
**请求体示例**：
```json
{
  "ip": "198.51.100.10/32"
}
```

## 兼容别名与非 `/v1` 接口

- 以下接口有不带 `/v1` 的兼容路径：  
  - `GET /cluster/members`  
  - `GET /cluster/gossip/stats`  
  - `POST /cluster/gossip/join`  
  - `POST /cluster/gossip/leave`

## 指标与健康检查

- 指标和健康检查不在控制面 `/v1/*` 下。  
- 请使用 admin server：  
  - `GET /metrics`  
  - `GET /healthz`

说明：当前实现没有控制面 `GET /v1/metrics` 和 `GET /v1/status/health` 这两个路径。

