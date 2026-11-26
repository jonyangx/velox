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
| `server.max_concurrent_requests` | `10` | `VELOX_CONFIG__SERVER__MAX_CONCURRENT_REQUESTS` |
| `storage.data_dir` | `./data` | `VELOX_CONFIG__STORAGE__DATA_DIR` |
| `log.dir` | `logs` | `VELOX_CONFIG__LOG__DIR` |
| `log.level` | `warn` | `VELOX_CONFIG__LOG__LEVEL`（严格语法；`RUST_LOG` 完整语法且优先级最高，调级须重启） |

> 日志（addlog）：进程自身写 `logs/velox-config.log` 并按大小轮转（含活跃共 3 份、单份 200 MiB 硬上限，归档 `.log.1/.log.2`）；默认 `warn` 静默 INFO。详见 [velox-mono/docs/adr/0002-log-rotation.md](../velox-mono/docs/adr/0002-log-rotation.md)。

`server.max_concurrent_requests` 为单实例进程级最大并发处理请求数：合法技术域 `1..=U`（`U = min(i64::MAX, usize::MAX, Semaphore::MAX_PERMITS)`，随平台），非法值或超界启动即失败（超界报错含上界 `U`）；空环境变量不覆盖配置文件中的值（文件有值则保留文件值）；配置仅在启动时加载一次，修改需重启生效。
默认 10 对高并发部署是行为收紧：超限请求立即收到 429（`code=too_many_requests`，附带 `Retry-After: 1`）；按峰值在途请求数调大即可，`/healthz` 不受限流影响。
日志核验前提：默认 `warn` 级别下启动 INFO 行静默（以饱和行为与每拒 WARN 日志证明生效），需查看启动上限 INFO 行时显式设置 `RUST_LOG=info` 或 `VELOX_CONFIG__LOG__LEVEL=info`。

## 测试

```bash
cargo test -p velox-config
```
