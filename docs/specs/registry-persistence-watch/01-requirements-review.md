# 需求评审报告：velox-registry v0.2「状态持久化 + watch 推送」（GATE1 关口评审）

> feature slug: `registry-persistence-watch`
> 被审文档：`docs/specs/registry-persistence-watch/01-requirements.md`（F-SA 产出：11 FR / 6 NFR / 14 AC / 12 边界场景 / 9 待确认问题）
> 评审角色：QA（关口评审，非终验；本文件为关口评审意见，非 04-acceptance.md）
> 评审日期分支：develop @ dff2006

## 0. 关口结论

**PIPELINE registry-persistence-watch GATE1 PASS**

一句话理由：11 FR / 14 AC 全部可判定，边界场景覆盖到位，范围外划分清晰，9 个待确认问题全部附可实施的建议默认方案，影响面描述经只读抽查代码逐条核实准确（仅 1 处轻微措辞失准 QA-1，及 5 处澄清级建议，均不阻塞，建议由 AA 定案时吸收）。

## 1. 逐项评审清单

### 1.1 FR/AC 可判定真伪 —— 结论：满足

- 14 条 AC 均为 Given/When/Then 结构，触发条件与期望结果可客观观测：AC-1（kill -9 重启后实例恢复、字段一致）、AC-2（同 id 心跳 200）、AC-4（超 TTL 实例不恢复 + WARN 计数）、AC-5（默认值/关闭/非法值三分支）、AC-8（挂起 1s 内返回、index 递增、旧 index 立即返回）、AC-11（timeout 非法值 400）等均可写成集成测试断言。
- FR-8 版本号语义（index 回落按服务端重启处理、客户端 index 过大立即返回并纠正）写入 FR 即可判定，处理了长轮询最易含糊的点。
- 挑出的含糊表述（均不阻塞，见 QA-5）：FR-1「丢数据窗口可控」、FR-2「显著小于」、NFR-2「应在亚秒级」、NFR-3「不产生写放大风暴」。建议 AA 定案时量化（如：结构性变更响应返回前完成落盘；`flush_interval_secs < heartbeat_ttl_secs` 作为校验或部署约束；恢复耗时上限；单次刷盘耗时上限）。

### 1.2 边界与异常覆盖 —— 结论：满足（2 处补充建议）

逐项核对任务指定场景：

| 场景 | 覆盖位置 | 判定 |
|------|---------|------|
| 落盘失败（磁盘满/权限/只读） | FR-7、B-1、AC-7（内存优先 + ERROR + 进程不退出） | ✅ |
| 恢复时数据损坏/部分写入 | FR-6、B-2（整文件跳过）、NFR-5 + AC-14（temp+rename 原子写防半写） | ✅ |
| watch 客户端断开/超时 | B-6（无会话状态、重携 index 即可）、FR-10、AC-9（关停唤醒）、AC-11 | ✅ |
| 重启后 TTL 语义 | FR-4（按绝对时间、超时不恢复）、B-4（全丢为正确行为）、B-9（时钟回拨已知限制）、Q9 | ✅ |
| 并发注册/注销与落盘竞争 | B-11 | ⚠️ 覆盖但论证不充分，见 QA-4 |
| 版本号批量变更语义 | FR-9 | ⚠️ 未定义，见 QA-2 |
| 心跳致 unhealthy→healthy 翻转 | FR-9.4 仅讨论 healthy→unhealthy 方向 | ⚠️ 见 QA-3 |
| 关停竞态（最终落盘 + watch 唤醒） | B-8、AC-9 | ✅ |
| 多进程共享 data_dir / 与 velox-config 共目录 | B-10、Q6 | ✅ 边界划分准确 |
| data 目录不可创建 | FR-6、B-3（fail fast 含路径） | ✅ |

### 1.3 范围外是否明确 —— 结论：满足

- mesh 消费侧：G2 明示「消费方接入不在本期」，§6.1 明确 velox-mesh/velox-config/velox-demo 零改动，与 architecture.md §3.3「mesh→registry 周期拉取（pull），push 模式留待 v0.3+」一致（architecture.md:61）。
- Raft/etcd/zone：§6.3 与 subsystems.md registry 演进路线「v0.3 多节点仲裁（Raft）/ etcd 后端适配 → v0.4+ 多机房 zone 感知」逐项对应。
- 与 velox-config 边界：FR-3/FR-6 引用其恢复时序与降级语义仅为模式复用；Q6/B-10 划清了 data_dir 共目录语义。
- 单机落盘不做分布式一致性：§6.4 明示。
- Q8 还主动识别并给出了 architecture.md §7 与 subsystems.md 演进路线不一致（前者 v0.2 行确无 registry 持久化/watch，architecture.md:128 核实属实）的文档同步方案——加分项。

### 1.4 待确认问题 —— 结论：满足

Q1–Q9 逐条核对，全部附建议默认方案且可判定、可实施：Q1（默认开启）、Q2（脏标记+周期刷盘，含 200 次/秒写穿不可接受的量级论证）、Q3（落盘失败 5xx，与 velox-config put 失败返回 Err 语义对齐——store.rs 文档注释核实属实）、Q4（全局单端点长轮询）、Q5（30s/300s/400）、Q6（./data + registry-state.json，与 velox-config `[storage] data_dir = "./data"` 对齐——velox-config/src/config.rs:44 核实属实）、Q7（healthz 不变）、Q8（以 subsystems.md 为准同步 architecture.md §7）、Q9（超时实例直接丢弃）。其中 Q3 建议定案时落到具体状态码与错误 code（见 QA-6）。

### 1.5 影响面准确性（只读抽查代码） —— 结论：满足（1 处措辞失准 QA-1）

需求文档对现状代码的描述逐条与实现比对：

| 文档断言 | 代码证据 | 判定 |
|---------|---------|------|
| `Registry` 为 `RwLock<HashMap<String, HashMap<Uuid, Instance>>>`，心跳用 `Instant` | velox-registry/src/registry.rs:26-33 | ✅ |
| provider 心跳失败仅 WARN 重试、无重注册 | velox-demo/provider/src/lifecycle.rs:167 `warn!(..., "心跳失败，下个周期重试")`；register 仅启动期（:131） | ✅ |
| 心跳默认 5s | velox-demo/provider/src/config.rs:50 `heartbeat_interval_secs: 5` | ✅ |
| mesh 默认 5s 全量拉取、失败降级用最后快照 | velox-mesh/src/config.rs:38 `poll_secs: 5`；discovery.rs:3,93-94 降级语义 | ✅ |
| FR-11 所列 6 个 v0.1 端点与错误 code | velox-registry/src/api.rs:21-34 路由一一对应；error.rs `service_not_found`/`instance_not_found`；心跳返回 `{"ok":true}`（AC-2 依据成立） | ✅ |
| 统一错误结构 `{"code","message"}` | velox-registry/src/error.rs:32-36 | ✅ |
| 配置机制：TOML + env 覆盖 + serde(default) + 非法值 fail fast | velox-registry/src/config.rs:12,60-72 | ✅ |
| velox-config：损坏文件跳过告警不阻断启动；put 内存已更新但落盘失败返回 Err；RwLock+tokio Mutex+spawn_blocking；恢复在接客前 | velox-config/src/store.rs:51,89-90（skip corrupted）、71-72（put 失败 Err）、4-9（并发模式注释）、41-47（open 恢复时序） | ✅ |
| main.rs shutdown watch 通道（26974b6 追加） | git show 26974b6 -- velox-registry/src/main.rs 新增 `watch::channel` + sweeper select 退出 | ✅ |
| v0.1 于 5299f66 落地 | git log：`5299f66 feat(registry): v0.1 内存注册表 + 心跳 TTL 自动剔除` | ✅ |
| `velox-registry/tests/` 当前不存在（NFR-6） | `ls velox-registry/tests` → No such file | ✅ |
| 所需依赖已被 workspace.dependencies 覆盖、预期零新增 | 根 Cargo.toml：tokio/axum/serde/serde_json/toml/tracing/uuid；原子写可用 std fs 实现 | ✅ |
| §8 store.rs 行「复用 velox-config 既有模式（**原子写**、损坏跳过、spawn_blocking）」 | velox-config/src/store.rs:172 为 `std::fs::write` 直接覆盖，**无** temp+rename 原子写 | ❌ 见 QA-1 |

## 2. 问题清单（均非阻塞）

| # | 严重级别 | 描述 | 证据 | 处置建议 |
|---|---------|------|------|---------|
| QA-1 | 低 | §8 影响面表称复用 velox-config「原子写」既有模式，失实：velox-config 现状为 `std::fs::write` 直接覆盖（store.rs:172），原子写是本期新增（NFR-5/AC-14 已正确要求） | velox-config/src/store.rs:172 | 措辞改为「复用损坏跳过/启动期恢复/spawn_blocking/串行落盘锁模式；原子写为本期新增」，避免误导设计阶段寻找不存在的复用代码 |
| QA-2 | 低中 | 版本号递增的批量语义未定义：一次 sweep 移除多个实例、或注册与剔除同窗口发生时 +1 还是 +N 未规定；AC-8 仅覆盖单实例注册 | FR-9、AC-8 | FR-9 明确「单次变更批次 +1」（建议）或「每实例 +1」，并说明 AC-8 断言前提 |
| QA-3 | 低中 | 心跳致 unhealthy→healthy 翻转未覆盖：实例超 TTL 但未被 sweep（sweep 5s vs TTL 30s，最长 5s 窗口）时迟到心跳使其恢复健康，watch 快照内容（健康实例列表）变化但不触发版本号递增，客户端最长需等满 timeout_secs 才能收敛 | FR-9.4、B-6 | 明确该翻转是否触发递增（建议：触发），或在文档中明示该收敛延迟为已知取舍 |
| QA-4 | 低 | B-11 断言「并发注册/注销与落盘竞争无新增风险」论证不足：结构性变更即时写穿与周期脏刷盘并发时，若 flush 快照读取时序不当，旧快照可覆盖新快照（心跳时间戳回退）。velox-config 已有成熟纪律（flush_lock 串行化 + 锁内读快照防文件回退） | velox-config/src/store.rs:127-152 flush 注释 | B-11 明确要求沿用该时序纪律（快照在串行锁内读取），而非仅「沿用 v0.1 锁语义」 |
| QA-5 | 低 | 含糊表述：FR-1「丢数据窗口可控」、FR-2「显著小于」、NFR-2「应在亚秒级」、NFR-3「写放大风暴」 | 第 2/3 节 | AA 定案时量化为可校验数值或约束 |
| QA-6 | 低 | Q3 建议「返回 5xx」未落到具体状态码与错误 code（FR-10 已为 400 给出统一结构先例） | FR-7、Q3 | 定案时明确如 500 + `persist_failed`，并纳入 error.rs 变体清单（§8 已预留该文件改动） |

## 3. 放行意见

通过进入下一工序（AA 架构设计）。QA-1–QA-6 不阻塞，建议 AA 在定案 Q2/Q3/Q4 时一并吸收（QA-1 为措辞修正，QA-2/QA-3/QA-4 为边界语义补全，QA-5/QA-6 为量化与具体化）。

