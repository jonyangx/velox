# velox-registry —— 服务注册中心

服务实例的唯一事实来源：注册、心跳续约、TTL 自动剔除与查询。架构定位见 [velox-mono/docs/subsystems.md](../velox-mono/docs/subsystems.md)。

## 快速开始

```bash
cargo run -p velox-registry                      # 使用默认配置，监听 :8300
cargo run -p velox-registry -- config/velox-registry.toml
```

## API（v0.1）

```
GET    /healthz                                                    # 健康检查
POST   /api/v1/registry/services/{name}/instances                  # 注册，返回 instance_id
PUT    /api/v1/registry/services/{name}/instances/{id}/heartbeat   # 心跳续约
DELETE /api/v1/registry/services/{name}/instances/{id}             # 注销
GET    /api/v1/registry/services                                   # 服务名列表
GET    /api/v1/registry/services/{name}/instances?healthy=true     # 实例列表
```

注册示例：

```bash
curl -X POST localhost:8300/api/v1/registry/services/demo/instances \
  -H 'content-type: application/json' \
  -d '{"address": "172.17.0.2", "port": 9000, "metadata": {"version": "0.1.0"}}'
```

## 配置

| 项 | 默认值 | 环境变量覆盖 |
|----|--------|--------------|
| `server.addr` | `0.0.0.0:8300` | `VELOX_REGISTRY__SERVER__ADDR` |
| `registry.heartbeat_ttl_secs` | `30` | `VELOX_REGISTRY__REGISTRY__HEARTBEAT_TTL_SECS` |
| `registry.sweep_interval_secs` | `5` | `VELOX_REGISTRY__REGISTRY__SWEEP_INTERVAL_SECS` |

## 测试

```bash
cargo test -p velox-registry
```
