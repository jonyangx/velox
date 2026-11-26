# velox-registry —— 服务注册中心

服务实例的唯一事实来源：注册、心跳续约、TTL 自动剔除与查询。架构定位见 [velox-mono/docs/subsystems.md](../velox-mono/docs/subsystems.md)。

v0.2 新增：**状态持久化**（重启恢复，provider 心跳无缝续上）与 **watch 长轮询**（registry 侧变更订阅；push 模式留待 v0.3+，见 architecture.md §3.3）。

## 快速开始

```bash
cargo run -p velox-registry                      # 使用默认配置，监听 :8300，持久化默认开启
cargo run -p velox-registry -- config/velox-registry.toml
```

## API（v0.2）

```
GET    /healthz                                                    # 健康检查
POST   /api/v1/registry/services/{name}/instances                  # 注册，返回 instance_id
PUT    /api/v1/registry/services/{name}/instances/{id}/heartbeat   # 心跳续约
DELETE /api/v1/registry/services/{name}/instances/{id}             # 注销
GET    /api/v1/registry/services                                   # 服务名列表
GET    /api/v1/registry/services/{name}/instances?healthy=true     # 实例列表
GET    /api/v1/registry/watch?index={n}&timeout_secs={t}           # v0.2 watch 长轮询
```

注册示例：

```bash
curl -X POST localhost:8300/api/v1/registry/services/demo/instances \
  -H 'content-type: application/json' \
  -d '{"address": "172.17.0.2", "port": 9000, "metadata": {"version": "0.1.0"}}'
```

### watch 长轮询（v0.2）

- 全局版本号单调递增（注册/注销/一批剔除/复活心跳各 +1；查询不触发），重启归零；
- `index` 缺省 = 首次快照请求，立即返回当前 `(index, services)`；
- `index` 显式携带且等于当前版本 → 挂起等待，变更即时唤醒或超时返回（HTTP 200）；
  不等（过旧/过大）→ 立即返回当前快照，响应内 `index` 纠正客户端；
- `timeout_secs` 缺省 30、上限 300，`0`/`301` → 400 `invalid_parameter`；
  参数解析失败（非数字/负数/溢出/空值）同样 400；
- 优雅关停（ctrl_c）唤醒全部挂起请求；响应仅含健康实例，结构与列表端点同构；
- 客户端断开即弃（无服务端会话），重携原 `index` 重入即可；registry 重启后
  `index` 回落，客户端按「服务端已重启」直接采纳返回快照。

```bash
curl "localhost:8300/api/v1/registry/watch?index=3&timeout_secs=30"
# → {"index":4,"services":{"svc-a":[{"id":"…","address":"…","port":…, ...}]}}
```

## 持久化行为（v0.2）

- **写穿**：注册/注销在响应前完成落盘（失败 500 `persist_failed`，内存已生效）；
  剔除任务移除实例后即时落盘；
- **合并刷盘**：心跳仅置脏标记，后台任务每 `flush_interval_secs` 合并写一次
  （崩溃最多丢最近一个周期内未落盘的心跳新鲜度；存在 IO 故障/调度严重延迟时不承诺
  一个周期的硬上界）；
- **恢复**：启动时（接客之前）读取 `{data_dir}/registry/registry-state.json`，
  按 `now − last_heartbeat > heartbeat_ttl_secs`（本次启动配置值）丢弃超时实例
  （严格大于，无宽限）；能否恢复以「已持久化心跳的年龄 ≤ 本次 TTL」为准——
  `flush_interval_secs < heartbeat_ttl_secs` 仅缩小新鲜度丢失窗口，不构成恢复硬保证；
- **损坏跳过**：快照文件损坏（非法 JSON/缺字段/未知 `format_version`）整文件跳过
  + WARN，按空注册表启动；原子写（temp + fsync + rename）防半写；
- **落盘失败**：内存优先继续服务；ERROR 日志告警；失败置回脏标记由周期任务
  自动重试直至成功或进程退出。

## 配置

| 项 | 默认值 | 环境变量覆盖 |
|----|--------|--------------|
| `server.addr` | `0.0.0.0:8300` | `VELOX_REGISTRY__SERVER__ADDR` |
| `registry.heartbeat_ttl_secs` | `30` | `VELOX_REGISTRY__REGISTRY__HEARTBEAT_TTL_SECS` |
| `registry.sweep_interval_secs` | `5` | `VELOX_REGISTRY__REGISTRY__SWEEP_INTERVAL_SECS` |
| `persistence.enabled` | `true` | `VELOX_REGISTRY__PERSISTENCE__ENABLED`（仅 `true`/`false`） |
| `persistence.data_dir` | `./data` | `VELOX_REGISTRY__PERSISTENCE__DATA_DIR` |
| `persistence.flush_interval_secs` | `5` | `VELOX_REGISTRY__PERSISTENCE__FLUSH_INTERVAL_SECS` |
| `log.dir` | `logs` | `VELOX_REGISTRY__LOG__DIR` |
| `log.level` | `warn` | `VELOX_REGISTRY__LOG__LEVEL`（严格语法；`RUST_LOG` 完整语法且优先级最高，调级须重启） |

> 日志（addlog）：进程自身写 `logs/velox-registry.log` 并按大小轮转（含活跃共 3 份、单份 200 MiB 硬上限，归档 `.log.1/.log.2`）；默认 `warn` 静默 INFO（启动/恢复横幅静默，排障用 `RUST_LOG=info`）。详见 [velox-mono/docs/adr/0002-log-rotation.md](../velox-mono/docs/adr/0002-log-rotation.md)。

校验规则（fail fast，错误含配置项名与实际值；**v0.2 起环境变量空字符串不再被忽略，
而是按非法值拒绝**——GATE2 仲裁定案）：

- `flush_interval_secs = 0`、`data_dir` 为空：无论持久化是否启用都拒绝；
- `flush_interval_secs >= heartbeat_ttl_secs`：仅 `enabled = true` 时拒绝；
- `enabled = false`：行为与 v0.1 完全一致（不落盘、不恢复、无刷盘任务），
  除上述字段本身非法外无其他拒绝项。

## 部署约束与已知限制

- **data_dir 单实例独占**：多 registry 进程共享同一 data_dir 会互踩快照文件
  （无文件锁保护）；多实例协调属 v0.3+ Raft 范围。
- **与 velox-config 同 cwd 部署安全**：registry 快照与临时文件都在
  `{data_dir}/registry/` 专属子目录内；velox-config 仅非递归扫描 data_dir **顶层**
  的 `{namespace}.json`，互不装载、互不覆盖（TC-37b 验证）。
- **时钟回拨**：恢复判定依赖系统时钟（epoch 秒），回拨属低概率运维事件，
  不做专门防御，发生时恢复判定可能偏差（已知限制）。
- **provider 无重注册逻辑**（velox-demo 范围）：registry 重启窗口靠「instance_id
  不变恢复」续命；若持久化被关闭或实例被 TTL 过滤丢弃，provider 将持续心跳失败
  直至被外部重启/重注册。
- **healthy→unhealthy 翻转不唤醒 watch**（TTL 到点、剔除前）：收敛延迟上限为
  `timeout_secs`（超时响应携带当前快照）；复活心跳（迟到心跳翻回健康）会唤醒。
- 快照 `sync_all` 保证 temp 文件内容在 rename 前落盘；未对父目录 fsync，
  极端掉电下目录项持久化不作承诺。

## 测试

```bash
cargo test -p velox-registry                     # 单测 + 集成（tests/{persistence,watch,compat}.rs）
cargo test --workspace                           # 全仓回归
bash velox-registry/tests/registry-e2e.sh        # 真进程 e2e（SIGKILL 恢复/写中崩溃/SIGINT 关停/config 同 cwd 隔离）
bash velox-registry/tests/registry-perf.sh       # AC-13 性能专项（真 v0.1 基线对比，验收期执行）
```
