# velox-config —— 配置中心

服务治理配置与系统配置的唯一权威存储：写入、读取、删除，落盘持久化。架构定位见 [velox-mono/docs/subsystems.md](../velox-mono/docs/subsystems.md)。

## 快速开始

```bash
cargo run -p velox-config                       # 使用默认配置，监听 :8200，数据目录 ./data
cargo run -p velox-config -- config/velox-config.toml
```

## API（v0.1）

```
GET    /healthz                             # 健康检查
GET    /api/v1/config/{namespace}           # 列举 namespace 下全部配置
GET    /api/v1/config/{namespace}/{key}     # 读取单项配置
PUT    /api/v1/config/{namespace}/{key}     # 写入（body 为任意 JSON 值）
DELETE /api/v1/config/{namespace}/{key}     # 删除
```

使用示例：

```bash
# 写入服务治理配置
curl -X PUT localhost:8200/api/v1/config/velox-mesh/governance \
  -H 'content-type: application/json' \
  -d '{"load_balance": "round_robin", "max_retries": 2}'

# 读取
curl localhost:8200/api/v1/config/velox-mesh/governance
```

## 存储模型

- 逻辑：`namespace`（如 `velox-mesh`）→ `key`（如 `governance`）→ JSON 值；
- 物理：`data/{namespace}.json` 写穿落盘，重启自动恢复；namespace 仅允许 `[a-zA-Z0-9_-]`。

## 配置

| 项 | 默认值 | 环境变量覆盖 |
|----|--------|--------------|
| `server.addr` | `0.0.0.0:8200` | `VELOX_CONFIG__SERVER__ADDR` |
| `storage.data_dir` | `./data` | `VELOX_CONFIG__STORAGE__DATA_DIR` |

## 测试

```bash
cargo test -p velox-config
```
