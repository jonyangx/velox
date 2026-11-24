# velox-mesh —— 服务网关

宿主机级数据面网关（**per-host sidecar daemon**）：一台宿主机一个 mesh 进程，为该机上全部业务容器提供统一的出口代理与服务发现接入。**禁止打进业务容器镜像。**

架构定位与部署模型详见 [velox-mono/docs/subsystems.md](../velox-mono/docs/subsystems.md)。

## 部署模型

```
┌────────────── 宿主机 ──────────────┐
│  ┌──────────┐  ┌──────────┐        │
│  │ 容器 svcA │  │ 容器 svcB │  ...  │
│  └────┬─────┘  └────┬─────┘        │
│       └──────┬──────┘              │
│        localhost:8100               │
│      ┌──────▼───────┐               │
│      │  velox-mesh  │──► registry   │
│      │ (host daemon)│──► config     │
│      └──────────────┘               │
└─────────────────────────────────────┘
```

- systemd 部署建议：`Restart=always`（mesh 崩溃由 systemd 拉起）；
- 业务容器通过 `localhost:8100`（host 网络或端口映射）接入。

## 快速开始

```bash
cargo run -p velox-mesh                          # 默认：数据面 :8100，管理面 :8101
cargo run -p velox-mesh -- config/velox-mesh.toml
```

## API（v0.1）

数据面（`:8100`）：

```
GET   /healthz                      # 健康检查
ANY   /proxy/{service}[/...]        # 反向代理到目标服务的健康实例（轮询负载均衡）
```

管理面（`:8101`，仅本机访问）：

```
GET   /healthz                      # 健康检查
GET   /api/v1/mesh/routes           # 当前服务快照
GET   /api/v1/mesh/status           # 运行状态（uptime / 快照年龄 / 实例数）
```

转发示例（目标服务 `demo` 已注册到 velox-registry）：

```bash
curl localhost:8100/proxy/demo/api/v1/ping
```

## 降级语义

- mesh 启动时不依赖 registry 可用（首拉失败仅告警，poller 持续重试）；
- registry 不可达期间使用**最后一次成功快照**继续转发（WARN 日志）；
- 目标服务无健康实例 → `404 {"code":"no_healthy_instance"}`；上游失败 → `502 {"code":"upstream_error"}`。

## 配置

| 项 | 默认值 | 环境变量覆盖 |
|----|--------|--------------|
| `server.inbound_addr` | `0.0.0.0:8100` | `VELOX_MESH__SERVER__INBOUND_ADDR` |
| `server.admin_addr` | `127.0.0.1:8101` | `VELOX_MESH__SERVER__ADMIN_ADDR` |
| `registry.endpoint` | `http://127.0.0.1:8300` | `VELOX_MESH__REGISTRY__ENDPOINT` |
| `registry.poll_secs` | `5` | `VELOX_MESH__REGISTRY__POLL_SECS` |
| `proxy.connect_timeout_secs` | `3` | `VELOX_MESH__PROXY__CONNECT_TIMEOUT_SECS` |
| `proxy.request_timeout_secs` | `30` | `VELOX_MESH__PROXY__REQUEST_TIMEOUT_SECS` |
