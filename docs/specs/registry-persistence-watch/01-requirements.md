# 需求分析：velox-registry v0.2 —— 状态持久化 + watch 推送（registry 侧）

> feature slug: `registry-persistence-watch`
> 状态：定向修订待 GATE1 复审（第 1 轮，仅 G2-4 与 G2-5 的需求归属修订；含 9 个待确认问题，见第 7 节）
> 上游依据：`velox-mono/docs/subsystems.md`「velox-registry —— 服务注册中心」演进路线「v0.1 内存注册表 + TTL 剔除 → v0.2 状态持久化、watch 推送」；`velox-mono/docs/architecture.md` §3.3（mesh→registry 为 pull，push 模式留待 v0.3+）

## 1. 背景与目标

velox-registry v0.1（提交 `5299f66`）是**纯内存**注册表：`velox-registry/src/registry.rs` 中 `Registry` 以 `RwLock<HashMap<String, HashMap<Uuid, Instance>>>` 保存全部实例状态，实例心跳时间用 `Instant` 记录。由此产生两类问题：

1. **重启即失忆**：registry 进程重启（升级、崩溃、宿主机重启）后注册表清空。此时各业务实例仍在运行并周期心跳，但 registry 已丢失它们的 `instance_id`——证据：`velox-demo/provider/src/lifecycle.rs` 的心跳循环对失败仅 `warn!("心跳失败，下个周期重试")`，**不会重新注册**，实例将持续心跳失败（404）直至进程被外部干预。对 mesh 而言，重启窗口内快照拉取失败、只能用最后一份快照降级（`velox-mesh/src/discovery.rs`），恢复后健康实例表为空，流量转发中断。
2. **变更感知靠轮询**：mesh 以固定周期（默认 5s，`poll_secs`）全量拉取，实例上线/下线最长有一个轮询周期 + 一个 TTL 窗口的感知延迟；registry 侧没有提供任何变更订阅能力。

**目标**（对应演进路线 v0.2）：

- **G1 状态持久化**：注册/心跳/注销/超时剔除产生的实例状态落盘，registry 单机重启后注册表可恢复，provider 心跳无缝续上（`instance_id` 保持不变）；
- **G2 watch 推送（registry 侧能力）**：registry 提供 HTTP + JSON 栈内的变更订阅机制（长轮询方向），实例变更可被客户端及时感知；消费方接入（如 mesh 改 push 订阅）不在本期；
- **G3 兼容性**：v0.1 全部既有 API 行为不破坏，mesh 现有拉取逻辑零改动继续工作。

**规模假设**：单机部署、量级 ≤ 100 服务 / ≤ 1000 实例（超出该量级的性能与容量不在本期承诺内，见 NFR-3）。

## 2. 功能需求

### FR-1 注册 / 注销 / 超时剔除产生的状态变化落盘

- 作为 registry 运维者，期望实例的**结构性变更**（新注册、主动注销、心跳超时剔除）即时写穿落盘，使任意时刻进程被 kill 后丢数据窗口可控。
- 落盘内容为注册表全量快照：服务名 → 实例集合，每实例含 `instance_id`、address、port、metadata、最后一次心跳的**绝对时间戳**（epoch 秒）。
  - 证据/约束：现行 `Instance.last_heartbeat` 为 `Instant`（`registry.rs`），不可序列化且重启后无意义，落盘必须改为（或额外记录）绝对时间戳，恢复时才能按真实流逝时间做 TTL 判定。
- 心跳本身是否逐次落盘见 FR-2（策略与 Q2 待确认联动）。

### FR-2 心跳状态的落盘策略：内存即时更新，落盘合并刷写

- 作为 registry 运维者，期望心跳续约（高频操作：默认每实例每 5s 一次，证据 `velox-demo/provider/src/config.rs` `heartbeat_interval_secs = 5`）**不**产生逐次全量文件写穿——按 1000 实例估算即 ≥ 200 次全量快照写/秒，单机 IO 不可接受。
- 期望策略（建议默认，AA 可定案，见 Q2）：心跳只更新内存并置脏标记，由后台任务按可配置周期（建议默认与 `sweep_interval_secs` 同量级，如 5s）将脏快照合并落盘。
- 语义代价（需在实现与文档中明示）：在**无 IO 故障、无调度严重延迟、刷盘按期完成**的前提下，registry 崩溃可能丢失最近一个刷盘周期内尚未落盘的心跳新鲜度；存在 IO 故障或调度严重延迟时，不能承诺该窗口为一个周期的硬上界。
- 恢复窗口：快照有效且实例存在于快照时，实例能否恢复取决于**恢复时 `now − 已持久化 heartbeat ≤ 本次启动的 heartbeat_ttl_secs`**。`flush_interval_secs < heartbeat_ttl_secs` 只能减小正常刷盘时的新鲜度丢失窗口，不能保证恢复；即使真实最后心跳未超 TTL，也可能因落盘心跳偏旧而按 FR-4 被丢弃。FR-4 的严格 `age > TTL` 丢弃规则保持不变，不引入恢复宽限期。
- 判定示例（G2-4 反例）：TTL=30s、刷盘周期=5s；t20 心跳已落盘，t25 刷盘先于该次心跳，文件仍记 t20，随后 t25 心跳仅更新内存；t29 崩溃、t51 恢复时，真实心跳年龄为 26s，已持久化心跳年龄为 31s，实例必须丢弃。因此 AC-1/AC-2 的成功恢复验收须满足上述已持久化心跳窗口。

### FR-3 重启后恢复注册表状态，`instance_id` 保持不变

- 作为业务服务实例，期望 registry 重启后我可以用**原来的 `instance_id`** 继续心跳续约成功，无需重新注册（provider 无重注册逻辑，见第 5 节边界 B-5）。
- 恢复范围：服务名、全部实例的 `id`/`address`/`port`/`metadata`、心跳时间戳（按 FR-1 的绝对时间戳换算）。
- 恢复发生在 HTTP 服务开始接客之前（启动期），恢复完成前不受理任何注册/查询请求（与 velox-config `Store::open` 的启动期恢复时序一致，证据 `velox-config/src/store.rs`）。

### FR-4 恢复时的 TTL 判定：按绝对时间，超时实例不恢复

- 作为 registry 运维者，期望恢复语义与运行时 TTL 语义一致：恢复时对每条实例按 `now − last_heartbeat > heartbeat_ttl_secs` 判定，已超时实例**不恢复**（直接丢弃），并输出 WARN 日志与丢弃计数。
- 理由：registry 宕机期间实例无法续约，宕机时长超过 TTL 后该实例在旧语义下本来就会被剔除，恢复时保留它只会制造「假健康」。
- 判定所用的 TTL 取**本次启动加载的配置值**（配置修改后以新值为准，属合理语义）。

### FR-5 持久化配置项：默认开启、可关闭、可指定目录

- 新增配置节（建议命名 `[persistence]`，AA 定案，见 Q1/Q6）：
  ```toml
  [persistence]
  enabled = true            # 是否启用落盘与恢复
  data_dir  = "./data"      # 落盘目录
  flush_interval_secs = 5   # 心跳脏快照刷盘周期（FR-2）
  ```
- 遵循既有配置机制（证据 `velox-registry/src/config.rs`）：TOML + 环境变量覆盖（`VELOX_REGISTRY__PERSISTENCE__DATA_DIR` 等）、`serde(default)` 全默认值、非法值启动失败（fail fast，如 `flush_interval_secs = 0`）。
- `enabled = false` 时行为与 v0.1 完全一致（不落盘、不恢复、无刷盘任务）。

### FR-6 恢复失败的降级策略：损坏跳过、不阻断启动

- 作为 registry 运维者，期望落盘文件损坏（半写、非法 JSON、字段缺失）时：**跳过该文件**、输出 WARN 日志（含文件路径与错误）、以空注册表正常启动——与 velox-config 既有降级语义一致（证据 `velox-config/src/store.rs` `Store::open`「损坏文件跳过并告警，不阻断启动」）。
- 不做部分恢复（不尝试恢复文件中可解析的片段）：快照是单文件全量结构，整文件级取舍简单且可判定。
- `data_dir` 不存在时自动创建；目录不可创建（权限等）属启动失败，错误信息含路径与原因。

### FR-7 落盘失败的降级策略：内存优先服务，告警外显

- 作为业务实例，期望磁盘故障（写失败、磁盘满）时 registry **内存中的注册/心跳/查询继续正常工作**，不因落盘失败拒绝服务。
- 落盘失败必须输出 ERROR 日志（开发规范 §3.4：存储写失败 = 需要人介入）；结构性变更（注册/注销/剔除）落盘失败时的 HTTP 响应语义见 Q3（倾向：注册/注销返回 5xx 以与 velox-config 写穿语义一致，证据 `velox-config/src/store.rs` put「内存已更新但落盘失败时返回 Err」；剔除为后台任务，仅日志）。

### FR-8 watch 端点：基于版本号的长轮询订阅

- 作为 registry 的消费方（未来可以是 mesh 或运维工具），期望订阅实例变更：发起长轮询请求并携带上次见过的**版本号（index）**：
  - registry 维护全局单调递增版本号，任何注册/注销/剔除导致的注册表内容变化都使版本号 +1；
  - 客户端 `index < 当前版本` → **立即返回**最新快照与新版本号；
  - 客户端 `index == 当前版本` → 挂起等待，直到发生变更（立即唤醒返回）或等待超时（返回当前快照与原版本号，HTTP 200）；
  - 机制约束：HTTP + JSON 长轮询（与 architecture.md §3.3「v0.x 全部采用 HTTP + JSON」一致）；不做 SSE / WebSocket / gRPC stream（push 模式留待 v0.3+）。
- 建议端点形态（AA 定案，见 Q4）：`GET /api/v1/registry/watch?index={n}&timeout_secs={t}`，响应建议含 `index` 与各服务最新健康实例列表。
- 版本号语义边界：registry 重启后版本号从 0 重新计数（不持久化，避免「客户端 index 大于服务端」的语义混乱）；客户端收到 `index` 回落即按「服务端已重启」处理、直接采纳返回的快照。该语义写入 FR 即视为可判定。
- watch 挂起等待不得占用阻塞线程（见 NFR-1），registry 优雅关停时必须唤醒/断开所有挂起中的 watch 请求（复用现有 shutdown watch 通道模式，证据 `velox-registry/src/main.rs`）。

### FR-9 watch 的变更触发范围

- 以下事件均视为「注册表内容变化」，触发版本号递增并唤醒等待中的 watch 请求：
  1. 实例注册成功；
  2. 实例主动注销；
  3. 后台剔除任务移除超时实例；
  4. （若采用 FR-2 刷盘策略）心跳刷盘导致的健康状态翻转（实例由 healthy → unhealthy 的判定变化）是否触发，见 Q4 附注——倾向：TTL 超时剔除（事件 3）已覆盖「下线」，健康翻转随周期 sweep 一并体现，不单独触发。
- 纯查询请求（列表、healthz）不触发版本号变化。

### FR-10 watch 超时可配置且有校验

- `timeout_secs` 参数（请求参数与/或配置项，Q5）：建议默认 30s；服务端校验 0 < timeout ≤ 300，非法值返回 400 + 统一错误结构（`{"code": ..., "message": ...}`，证据 `velox-registry/src/error.rs`）。

### FR-11 v0.1 API 兼容性：既有行为零破坏

- 以下 v0.1 端点的路径、方法、请求/响应 JSON 结构、状态码与错误 `code` **全部保持不变**（证据 `velox-registry/src/api.rs`、`velox-registry/src/error.rs`）：
  - `GET /healthz`；`POST /api/v1/registry/services/{name}/instances`；
  - `PUT .../instances/{id}/heartbeat`；`DELETE .../instances/{id}`；
  - `GET /api/v1/registry/services`；`GET /api/v1/registry/services/{name}/instances?healthy=true`。
- 既有错误语义不变：未知服务/实例 → 404 + `service_not_found` / `instance_not_found`。
- 未提供配置文件时（`Config::load(None)` 路径），除新增持久化/watch 默认值生效外，其余行为与 v0.1 一致。
- mesh 既有拉取逻辑（`velox-mesh/src/discovery.rs`，周期 GET services + instances?healthy=true）**不需要任何修改**即可继续工作——本条同时是验收手段。

## 3. 非功能需求

### NFR-1 不阻塞：锁内纯内存操作，IO 交阻塞线程池

- 领域锁临界区内不得出现文件 IO（遵循 velox-config 既有模式：`std::sync::RwLock` 保护纯内存读写 + `tokio::sync::Mutex` 串行化落盘 + `spawn_blocking` 写文件，证据 `velox-config/src/store.rs` 模块注释）；
- watch 长轮询挂起必须基于异步等待（如 `tokio::sync::watch`/`Notify`），不得占用 tokio worker 线程以外的阻塞线程。

### NFR-2 可用性：恢复与落盘失败均不得阻断/拖垮服务

- 启动恢复为一次性同步 IO，发生在接客之前（FR-3），恢复耗时随数据量线性但量级受规模假设约束（1000 实例量级应在亚秒级完成）；
- 运行期任何落盘失败不导致进程退出、不阻塞请求处理（FR-7）。

### NFR-3 性能：热路径不因落盘显著劣化

- 在规模假设内（≤ 100 服务 / ≤ 1000 实例、每实例 5s 心跳）：
  - 注册/心跳/查询请求的延迟不劣于 v0.1 基线的 2 倍（同一压测条件下对比，验收时可测）；
  - 刷盘任务周期触发，单次落盘为一次全量快照写（量级：千实例 JSON 约数百 KB），不产生写放大风暴。
- 超出规模假设的容量不承诺（见第 6 节）。

### NFR-4 可观测性

- 启动日志必须包含：持久化是否启用、data_dir、恢复结果（恢复的服务数/实例数、丢弃的超时实例数、跳过的损坏文件）；
- 落盘失败 → ERROR；恢复时丢弃超时实例、跳过损坏文件 → WARN；注册/注销/剔除 → INFO（沿用 v0.1 既有日志）；watch 唤醒/超时 → DEBUG；
- 日志规范遵循开发规范 §3.4（结构化字段优先）。

### NFR-5 数据完整性：原子写防半写

- 快照落盘必须原子替换（写临时文件 + rename），避免进程在写入中途被 kill 留下半写文件；即使半写发生，FR-6 的损坏跳过语义兜底。

### NFR-6 工程约束

- 全部依赖取自 `[workspace.dependencies]`（现状 tokio/axum/serde/serde_json/uuid 等已覆盖所需能力，预期**无需新增依赖**，证据：根 `Cargo.toml`）；
- 新 API 有集成测试（`velox-registry/tests/`，当前该目录不存在，需新建；开发规范 §6「README 中的 API 示例必须有对应集成测试佐证」）；
- 新配置项有默认值并更新 `velox-registry/config/velox-registry.toml` 与 README 配置表。

## 4. 验收标准

| # | 对应 FR | 验收标准（Given / When / Then） |
|---|---------|--------------------------------|
| AC-1 | FR-1 | Given registry 已注册实例 A 于服务 svc，持久化启用且 A 已成功写入有效快照，并确保恢复时 `now − A 的已持久化 heartbeat ≤ 本次启动的 heartbeat_ttl_secs`（FR-2 恢复窗口）；When kill -9 registry 进程后原配置重启；Then 启动日志显示恢复了 svc 下实例 A，且在 A 再次超时被剔除前 `GET .../instances` 返回 A，`id`/`address`/`port`/`metadata` 与重启前一致；不以真实最后心跳未超 TTL 或刷盘周期小于 TTL 代替恢复窗口前提 |
| AC-2 | FR-3 | Given AC-1 场景恢复完成，A 在有效快照中且恢复时 `now − A 的已持久化 heartbeat ≤ 本次启动的 heartbeat_ttl_secs`；When 原实例在 A 再次超时被剔除前，以同一 `instance_id` 发 `PUT .../heartbeat`；Then 返回 200 `{"ok":true}`，无 404；若恢复时已持久化心跳超 TTL，则按 FR-4 丢弃，不适用本条成功续约断言 |
| AC-3 | FR-2 | Given 启用刷盘周期 T=5s、TTL=30s；When 实例每 5s 心跳持续运行；Then data 目录文件修改时间随刷盘周期更新，且心跳请求延迟不随落盘同步产生（心跳响应不等待本轮刷盘） |
| AC-4 | FR-4 | Given 落盘文件中实例 B 的心跳时间戳距 now 超过 TTL；When registry 重启恢复；Then B 不出现在任何实例列表中，日志输出丢弃计数（WARN） |
| AC-5 | FR-5 | Given 不提供配置文件启动；Then `[persistence]` 取默认值（enabled=true、`./data`）且启动成功；Given `enabled=false`；Then 不创建/不写落盘文件，行为与 v0.1 一致；Given `flush_interval_secs = 0`；Then 启动失败并含配置项名与实际值 |
| AC-6 | FR-6 | Given data 目录中存在内容损坏的快照文件；When registry 启动；Then WARN 日志含文件路径，registry 以空注册表正常接客（`GET /healthz` 200） |
| AC-7 | FR-7 | Given 落盘目标不可写（如目录只读）；When 注册新实例；Then 注册按 Q3 定案语义响应，且内存注册成功（实例列表可查）、ERROR 日志出现，进程不退出 |
| AC-8 | FR-8 | Given 客户端已发起 `watch?index=当前版本` 挂起；When 另一客户端注册新实例；Then 挂起请求在 1s 内返回，响应 `index` = 旧值+1 且快照含新实例；Given 另一客户端携带 `index` 小于当前版本再次请求；Then 立即返回（不挂起） |
| AC-9 | FR-8 | Given registry 收到 ctrl_c 优雅关停；Then 所有挂起中的 watch 请求被唤醒/断开，服务进程正常退出 |
| AC-10 | FR-9 | Given 实例被后台剔除任务移除；Then 等待中的 watch 请求被唤醒且快照不再含该实例；Given 仅发生查询请求；Then `index` 不变 |
| AC-11 | FR-10 | Given `watch?timeout_secs=0` 或 `timeout_secs=301`；Then 返回 400 + 统一错误结构；Given 未传 `timeout_secs`；Then 按 30s 处理 |
| AC-12 | FR-11 | v0.1 既有集成断言全部保持通过（未知服务 404 + `service_not_found`、`?healthy=true` 过滤等）；mesh 不改一行代码，其 `discovery.rs` 周期拉取在 registry 开启持久化/watch 后继续正常返回 |
| AC-13 | NFR-3 | Given 1000 实例、5s 心跳的压测；Then 注册/心跳/查询 P99 ≤ v0.1 同条件基线 × 2 |
| AC-14 | NFR-4/5 | 启动日志含持久化与恢复结果字段；落盘文件经「写临时文件后 rename」产生（代码审查 + 崩溃注入测试：写盘中途 kill 不产生半写文件被当作有效快照） |

## 5. 边界与异常场景

- **B-1 落盘失败**（磁盘满、权限、只读文件系统）：内存优先继续服务（FR-7），ERROR 告警；注册/注销响应语义按 Q3 定案执行。
- **B-2 恢复文件损坏/半写**：整文件跳过 + WARN，空注册表启动（FR-6）；不尝试部分恢复。
- **B-3 data 目录不可创建**：启动失败（fail fast），错误信息含路径。
- **B-4 宕机时长 > TTL**：全部实例恢复时按 FR-4 判定超时、全部丢弃，恢复后注册表为空——这是**正确行为**：宕机期间实例无法续约，其注册信息已失效。
- **B-5 registry 重启窗口内 provider 的行为**（现状约束，本 feature 不修改 provider）：demo-provider 心跳失败仅 WARN 重试，**无重新注册逻辑**（证据 `velox-demo/provider/src/lifecycle.rs` 心跳循环）。因此 FR-3 的「instance_id 不变恢复」是 provider 无缝续命的**必要条件**；若持久化被关闭（`enabled=false`）或实例被 FR-4 丢弃，provider 将持续心跳失败直至被外部重启/重注册——该限制写入 registry 文档说明，修复属 velox-demo（范围外，见第 6 节）。
- **B-6 watch 客户端断开/超时**：长轮询每次请求独立，无服务端订阅会话状态；客户端断开仅使本次响应丢弃，重新携带原 `index` 请求即可，服务端无需清理动作。
- **B-7 版本号回落（registry 重启）**：`index` 从 0 重计（FR-8）；客户端携带大于当前版本的 `index` 请求时，按「立即返回当前快照」处理（等同 index 过旧），并在响应中以新 `index` 纠正客户端。
- **B-8 关停竞态**：优雅关停时刷盘任务执行一次最终快照落盘（尽力而为，失败仅日志），挂起 watch 全部唤醒（FR-8/AC-9）。
- **B-9 时钟回拨**：恢复判定依赖系统时钟（epoch 秒）。单机部署假设下时钟回拨属低概率运维事件，不做专门防御；发生时恢复判定可能偏差，属已知限制（文档说明）。
- **B-10 多 registry 进程共享同一 data_dir**：无文件锁保护，两个 registry 写同一快照文件会互踩。部署约束：data_dir 语义为**单实例独占**，文档明示；多实例协调属 v0.3+ Raft（范围外）。
- **B-11 并发注册/并发 watch 唤醒**：版本号递增与唤醒由锁内/原子操作保证一致性；并发注册同服务沿用 v0.1 锁语义，无新增风险。
- **B-12 国际化/字符集**：服务名沿用 v0.1 未限制字符集的现状（本 feature 不引入新校验，避免破坏兼容）；metadata 值任意字符串经 JSON 序列化落盘，无需额外处理。

## 6. 范围外

1. **velox-mesh / velox-config / velox-demo 的任何代码改动**。watch 消费方接入（mesh 改 push 订阅、demo-provider 增加断线重注册 B-5）均不在本期——architecture.md §3.3 明确「push 模式留待 v0.3+」；
2. **velox-mono 代码**：无（本 feature 交付时按开发规范评审检查单同步 `velox-mono/docs/subsystems.md` velox-registry 章节 API 概览，属文档更新）；
3. **Raft 多节点仲裁、etcd 后端适配、多机房 zone 感知**：v0.3+/v0.4+（subsystems.md 演进路线）；
4. **分布式一致性 / registry 多实例部署**：本期持久化为单机落盘，不做跨节点复制与仲裁；
5. **push 模式**（SSE / WebSocket / gRPC stream）：留待 v0.3+；
6. **metrics（Prometheus）、TLS/mTLS、审计/操作历史**：v0.3+ 各自规划，本期仅 tracing 日志可观测性（NFR-4）;
7. **配置热更新**：新配置项启动时加载一次；
8. **实例数据的加密存储、落盘文件的手工编辑契约**：data 目录内容仅为内部恢复用途，不对外承诺格式兼容（格式变更无需升 API 版本）。

## 7. 待确认问题（每条附建议默认方案）

- **Q1 持久化默认开启还是关闭？**
  建议默认**开启**（`enabled = true`）：目标场景（单机重启恢复）无需额外配置即获益；关闭仅作为排障逃生门。风险：v0.1 用户升级后多出 data 目录写入行为——属无害增量，README 说明即可。
- **Q2 心跳落盘策略：逐次写穿 vs 脏标记周期刷盘？**
  建议默认**脏标记 + 周期刷盘**（FR-2）：1000 实例 × 每 5s 心跳 = 200 次/秒全量快照写穿不可接受；周期刷盘（默认 5s，可配 `flush_interval_secs`）在无 IO 故障、无调度严重延迟且刷盘按期完成时，可能丢失最近一个周期内尚未落盘的心跳新鲜度（FR-2）。注册/注销仍是即时写穿，但已持久化实例恢复时仍须满足 `now − 已持久化 heartbeat ≤ 本次 TTL`，否则按 FR-4 丢弃，不能承诺真实最后心跳未超 TTL 就一定恢复。备选「逐次写穿」仅在实例规模 ≤ 100 的保守场景可辩护，不做默认。
- **Q3 注册/注销落盘失败时的 HTTP 语义？**
  建议默认**返回 5xx**（与 velox-config 写穿语义一致，证据 `velox-config/src/store.rs` put 失败返回 Err）：保证「注册成功 ⇒ 已落盘」的强语义，调用方（provider）已有注册退避重试逻辑可自愈。代价：磁盘故障期间无法注册新实例（可接受，磁盘故障本需人介入）。剔除为后台任务，失败仅 ERROR 日志。心跳不直接落盘（FR-2），永不因落盘失败报错。
- **Q4 watch 端点形态与粒度？**
  建议默认**全局单端点** `GET /api/v1/registry/watch?index=&timeout_secs=`，响应携带各服务最新健康实例列表 + 新 `index`：mesh 类消费方本来就全量订阅，无需 per-service 端点；实现最简、无会话状态。备选 per-service 端点（`.../services/{name}/watch`）留给真需求出现再加。附注：健康翻转是否独立触发版本号，倾向**不独立触发**（FR-9.4，周期 sweep 已覆盖下线感知）。
- **Q5 watch 超时默认值与上限？**
  建议默认 **30s**、上限 300s、非法值 400（FR-10）：与心跳 TTL 量级匹配，客户端断线重连成本低。
- **Q6 data 目录默认路径与文件布局？**
  建议保留默认 `data_dir = "./data"`（FR-5/AC-5 不变），由 registry 在该根目录下使用专属子目录，例如 `./data/registry/registry-state.json`；实际文件布局由 AA 定案，快照及临时文件均应置于隔离子目录内。纠正原依据「共目录文件名不同、互不冲突」：`velox-config/src/store.rs` 的 `Store::open` 非递归扫描 data_dir 顶层所有 `.json`，`valid_namespace` 接受 `registry-state`，且 registry 快照的顶层 JSON 对象可被 `NamespaceData = BTreeMap<String, Value>` 解析，因此顶层 `./data/registry-state.json` 会被 config 装载为 namespace；后续对该 namespace 的 put/delete 会经 `flush` 覆盖同一路径，破坏快照。专属子目录可避开现有非递归扫描；不修改 config 扫描实现。B-10 的双 registry 独占约束仍成立，但不能用它推导 registry 与 config 共用顶层文件目录无冲突。
- **Q7 healthz 是否暴露持久化/watch 状态？**
  建议默认**不变**（FR-11 兼容优先），观测靠 NFR-4 日志；若评审认为需要，可另加 `GET /api/v1/registry/status`（非本期验收项）。
- **Q8 architecture.md §7 演进路线表与 subsystems.md 不一致（前者 v0.2 行未列 registry 持久化/watch）？**
  建议以 subsystems.md registry 演进路线为准（本 feature 依据），交付时同步更新 architecture.md §7 v0.2 行措辞，属文档 PR，避免后续歧义。
- **Q9 恢复时已超 TTL 实例是否保留（如标记 unhealthy 观察）？**
  建议默认**直接丢弃**（FR-4）：宕机 > TTL 后实例注册信息已失效，保留只会制造假健康并被 sweep 立即剔除，徒增复杂度；WARN 日志 + 丢弃计数已保证可观测。

## 8. 影响面（涉及的模块/文件）

**代码改动（全部限定在 velox-registry）**：

| 文件 | 改动 |
|------|------|
| `velox-registry/src/registry.rs` | `Instance` 心跳时间改/增绝对时间戳（可序列化）；版本号维护；注册/注销/剔除挂接落盘与 watch 唤醒 |
| `velox-registry/src/store.rs`（新建） | 快照落盘与启动恢复，复用 velox-config `store.rs` 既有模式（原子写、损坏跳过、spawn_blocking） |
| `velox-registry/src/api.rs` | 新增 watch 路由与 handler（长轮询挂起/唤醒）；v0.1 路由不动 |
| `velox-registry/src/config.rs` | 新增 `[persistence]` 配置节（enabled/data_dir/flush_interval_secs）+ env 覆盖 + 校验 |
| `velox-registry/src/main.rs` | 装配：启动恢复、刷盘后台任务（带优雅退出）、关停时最终落盘与 watch 唤醒 |
| `velox-registry/src/error.rs` | 新增错误变体（如落盘失败、非法 watch 参数）到统一错误结构映射 |
| `velox-registry/tests/`（新建目录） | 持久化重启恢复、watch 长轮询、兼容性回归等集成测试 |
| `velox-registry/config/velox-registry.toml`、`velox-registry/README.md` | 新配置项、新 API、v0.2 说明同步 |

**文档同步**：`velox-mono/docs/subsystems.md`（velox-registry API 概览与演进路线状态）、`velox-mono/docs/architecture.md` §7（v0.2 行措辞，见 Q8）。

**明确不受影响**：`velox-mesh/*`、`velox-config/*`、`velox-demo/*` 零改动（第 6 节范围外）。

**现状证据基准**：`velox-registry` v0.1 功能面（内存注册表 + 心跳 TTL 剔除 + HTTP API）于提交 `5299f66` 落地；`26974b6` 对其追加了剔除任务优雅退出通道（`velox-registry/src/main.rs` 的 shutdown watch），未改变 API 与存储形态——当前 develop 分支即为本文档调研的现状。
