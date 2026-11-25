# 测试用例：velox-registry v0.2 —— 状态持久化 + watch 推送（registry 侧）

> feature slug: `registry-persistence-watch`
> 输入：`01-requirements.md`（GATE1 PASS，AC-1…AC-14 已冻结）＋ 只读测试基建侦察（无 F-TA-P 笔记，本文档第 2 章为本次自行侦察结论）
> 范围：仅 velox-registry（持久化重启恢复 / watch 长轮询 / v0.1 API 兼容回归）；mesh/config/demo 零改动

## 1. 测试范围与分层策略（单测/集成/e2e 边界）

| 层 | 位置 | 覆盖内容 |
|---|------|---------|
| 单元测试 | `velox-registry/src/*` 内嵌 `#[cfg(test)] mod tests` | 纯逻辑：快照序列化 roundtrip、损坏文件判定、配置校验（`flush_interval_secs=0` fail fast）、TTL 判定纯函数、版本号递增规则 |
| 集成测试 | `velox-registry/tests/`（新建，NFR-6 要求） | axum `Router` + `tower::ServiceExt::oneshot` 驱动 HTTP 层 + 真实持久化 Store + 临时目录：重启恢复、watch 挂起/唤醒/超时、落盘失败注入、并发竞争、v0.1 兼容回归 |
| e2e | `velox-demo/scripts/acceptance.sh` 扩展 + 复用 `scripts/common.sh` | 真实进程生命周期：kill -9 重启恢复（AC-1/2）、SIGINT 优雅关停断开 watch（AC-9）、刷盘 mtime 观测（AC-3）、mesh 零改动拉取（AC-12）、启动日志字段 grep（NFR-4） |
| 手工/代码审查 | — | AC-14 rename 原子写审查（配合 TC-18 tmp 残留模拟）、AC-13 压测对比（自建轻量压测，见 RQ-2）、B-9/B-10 文档声明项 |

分层边界约定：
- **涉及真实进程信号（kill -9 / SIGINT）与跨进程行为（provider 心跳续命、mesh 拉取）的用例归 e2e**；纯 router 级行为（挂起、唤醒、参数校验、落盘失败）归集成层，可在毫秒级完成、不依赖端口。
- **watch 挂起/优雅关停的集成可测性依赖实现形态**：若 shutdown 通道与刷盘任务可从测试侧注入（`with_graceful_shutdown` 接收任意 future），TC-21/24/26/30 可在集成层完成；否则降级为 e2e（用例中已标注双轨）。
- 每条 AC 至少 1 条 P0 或 P1 用例（见第 4 章矩阵，无缺口）。

## 2. 测试环境与基建（框架、命令、夹具、e2e 手段）

> 本章为本次自行完成的只读侦察结论（F-TA-P 被跳过，无 PREPARE 笔记）。

**框架与既有测试组织**
- 测试执行器：`cargo test`（workspace 统一）；单测为 `#[cfg(test)] mod tests` 内嵌 + `#[tokio::test]`；workspace 共 12 个源文件含内嵌测试（velox-config 4 / velox-registry 1 / mesh 1 / demo 与 consumer 其余）。
- `velox-registry/tests/` **当前不存在**，需随本 feature 新建（需求 NFR-6、影响面表已列）。
- HTTP 层测试样板：`velox-registry/src/api.rs` 内嵌测试用 `Router` + `tower::ServiceExt::oneshot`（无真实端口）；dev-dep `tower` 已在 `[workspace.dependencies]`，**无需新增依赖**。
- 持久化测试样板（直接可复用）：`velox-config/src/store.rs` tests —— 临时目录 + `Uuid::new_v4()` 命名隔离、drop 后重新 open 断言 roundtrip（`persists_across_restart`）、损坏文件跳过语义同型用例可参照。
- 时间敏感用例可用 `#[tokio::test(start_paused = true)]`（workspace tokio 已启用 time 能力，`main.rs` 在用）。

**构建/测试命令（CI 门槛）**
- `scripts/test.sh`：fmt + `clippy --workspace --all-targets -- -D warnings` + build + test 全绿（完整门槛）。
- `scripts/test.sh --unit`：只跑 `cargo test`；单 crate：`cargo test -p velox-registry`。

**e2e / 集成验证手段**
- `velox-demo/scripts/acceptance.sh`：host 模式（**docker-compose 为 Linux `network_mode: host` 专用，macOS 开发机不可用，e2e 一律走 host 脚本**），curl + jq 断言；固定端口 8100/8101/8200/8300/9000/9001/9100；已内置 kill -9 宕机剔除先例与 `wait_json_eq` 轮询断言工具，可直接复用其模式扩展「重启恢复」步骤。
- `scripts/run.sh start/stop`：管理 core 三件套（registry(:8300)→config→mesh）；pid 在 `.run/`、日志在 `logs/<crate>.log`（日志断言 = grep 该文件，`RUST_LOG` 可调级别）。
- 配置覆盖：env `VELOX_REGISTRY__<SECTION>__<FIELD>`（`config.rs` 既有机制），e2e 用 `VELOX_REGISTRY__PERSISTENCE__DATA_DIR=<临时绝对路径>` 隔离落盘，避免污染仓库树（对齐 acceptance.sh 对 velox-config data 的清理做法）。

**风险与局限（侦察结论）**
1. **watch 挂起/关停类用例的集成可测性取决于实现是否暴露可注入 shutdown 通道**；不可注入则 TC-21/24/26/30 走 e2e SIGINT（curl 后台挂起）。
2. **落盘失败注入**：`chmod 555` 只读目录在 root/部分 CI 环境会被绕过 → 优先集成层向 Store 传非法/只读路径注入；e2e 层标注环境依赖。
3. **时间敏感断言**（刷盘 mtime、watch「1s 内唤醒」）受 CI 负载抖动影响，阈值需放宽容差（唤醒断言 ≤ 2s 记通过并注明）。
4. **AC-14 崩溃注入窗口**（「写盘中途 kill」）无法精确命中写盘中间态 → 以 TC-18（tmp 残留模拟）+ 代码审查（rename 语义）组合覆盖，e2e kill 时序尝试为尽力而为。
5. **AC-13 无压测基建**（无 criterion、无压测脚本、无 v0.1 基线留存）→ 需自建轻量压测，见 RQ-2。
6. 测试隔离：集成测试临时目录用 `Uuid` 命名并 finally 清理；e2e 用独立 `VELOX_REGISTRY__PERSISTENCE__DATA_DIR`。

## 3. 用例表（TC-x）

> 列说明：类型 = 单元/集成/e2e/审查；优先级 P0 冒烟级 / P1 核心 / P2 健壮性。前置中「临时目录」均指 Uuid 命名隔离目录。

### A 组：持久化落盘与重启恢复

| TC | 所属 AC/FR | 类型 | 优先级 | 前置条件 | 步骤 | 期望结果 |
|----|-----------|------|--------|---------|------|---------|
| TC-01 | AC-1/FR-1 | e2e | P0 | registry 以 `PERSISTENCE__DATA_DIR=<tmp>` 启动（enabled 默认 true） | ① POST 注册实例 A（含非空 metadata）于 svc；② 记录响应 id/address/port/metadata；③ `kill -9` 进程；④ 同 env 重启 | 启动日志显示恢复 svc 下实例 A；`GET .../instances` 返回 A 且 `id`/`address`/`port`/`metadata` 与 ② 完全一致 |
| TC-02 | AC-2/FR-3 | e2e | P0 | TC-01 完成后 | 以原 `instance_id` PUT `.../instances/{id}/heartbeat` | 200 `{"ok":true}`，无 404；`instances?healthy=true` 仍含 A |
| TC-03 | FR-1/AC-1 | 集成 | P1 | 临时目录 + TTL=30s 的 Registry+Store | ① 注册 A；② sleep 3s；③ drop 后重新 open（模拟重启） | 实例 A 恢复且 `seconds_since_heartbeat ≥ 3`（按绝对时间换算，非重置 0）、`healthy=true` |
| TC-04 | FR-1/AC-1 | 集成 | P0 | 同 TC-03 | ① 注册 A 后**立即**读快照文件断言；② 注销 A 后立即读文件 | ① 文件已含 A（注册即时写穿，不等刷盘周期）；② 文件已不含 A；文件中时间戳为 epoch 秒整数 |
| TC-05 | FR-2/AC-3 | 集成 | P1 | `flush_interval_secs=2`、TTL=30 | ① 注册 A 并等首次刷盘；② 读文件记时间戳 T₀；③ 心跳数次，0.3s 内读文件；④ sleep >2s 再读文件 | ③ 文件时间戳仍为 T₀（心跳未逐次写穿）；④ 文件时间戳已更新且 > T₀（脏快照合并刷盘）；③ 期间心跳响应正常返回 |
| TC-06 | AC-3 | e2e | P1 | 同 TC-01，`PERSISTENCE__FLUSH_INTERVAL_SECS=2`，实例持续每 1s 心跳 | 在 ≥2 个刷盘周期内轮询 `stat` 快照文件 mtime | mtime 至少更新一次且随周期推进；心跳请求全程无 5xx |
| TC-07 | FR-4/AC-4 | 集成 | P0 | 临时目录中手工构造合法快照：实例 A 心跳=now、实例 B 心跳=now−60（TTL=30） | 以 TTL=30 启动恢复 | 列表仅含 A；B 不出现在任何实例列表；WARN 日志含丢弃计数=1 及 B 标识 |
| TC-08 | FR-4 | 集成 | P1 | 同 TC-07 同一快照文件 | ① 以默认 TTL=30 启动 → 记结果；② 再以 env `HEARTBEAT_TTL_SECS=1` 重启恢复 | ① A、B 均恢复；② A、B 全部丢弃（判定用本次启动 TTL，非落盘时 TTL） |
| TC-09 | FR-4/B-4/B-5 | e2e | P1 | 短 TTL（`HEARTBEAT_TTL_SECS=6`）+ provider（心跳 2s）运行 | ① 等 provider 注册并落盘；② `kill -9` registry；③ sleep 8s（>TTL）；④ 重启 registry | 恢复后 `GET /api/v1/registry/services` 为空列表；启动日志丢弃计数=宕机前实例数；provider 心跳持续 404（B-5 现状行为，仅记录不判缺陷） |
| TC-10 | FR-5/AC-5 | 集成 | P0 | `PERSISTENCE__ENABLED=false` + 临时目录路径 | ① 注册 A、心跳、查询；② 检查 data_dir 是否创建/有文件；③ drop 重启 | ① 全部操作与 v0.1 行为一致；② 目录不创建、无落盘文件；③ 重启后注册表为空（不恢复） |
| TC-11 | AC-5 | e2e | P0 | 无配置文件启动（`Config::load(None)` 路径），cwd=干净临时目录 | 启动后检查 cwd 下 `./data/` 与 `registry-state.json`（文件名以 AA 定案 Q6 为准） | 启动成功；data 目录与快照文件生成（默认 enabled=true 生效）；注册后文件更新 |
| TC-12 | FR-5/AC-5 | 单元 | P0 | — | `Config::load` 分别传入 `flush_interval_secs = 0`、`-1`、非数字（TOML 与 env 两种途径） | 全部返回 Err；错误信息含配置项名（如 `flush_interval_secs`）与实际非法值 |
| TC-13 | FR-6/AC-6 | 集成 | P0 | data 目录分别预置：① 截断半写 JSON；② 合法 JSON 但字段缺失/类型错；③ 空文件（三场景分跑） | 启动恢复 → `GET /healthz`、`GET .../services` | 三场景均：启动成功、healthz 200、注册表为空；WARN 日志含损坏**文件路径**与错误原因 |
| TC-14 | FR-6 | 集成 | P1 | data_dir 指向不存在路径 | 启动恢复 | 目录被自动创建，启动成功、可正常注册 |
| TC-15 | FR-6/B-3 | 集成 | P0 | data_dir 指向一个已存在的**普通文件**（无法创建目录）；或权限拒绝路径（非 root 环境） | 启动 | 启动失败（fail fast）；错误信息含路径与原因（如 `Not a directory`/`Permission denied`） |
| TC-16 | FR-7/AC-7 | 集成 | P0 | Store 指向只读目录（chmod 555，非 root） | ① POST 注册 A；② GET 列表；③ 检查进程与日志 | ① 注册响应为 **5xx**（按 Q3 建议默认：500 + `persist_failed`，待 AA 定案回填，见 RQ-1）；② A 已入内存、列表可查；③ ERROR 日志出现；进程存活、后续查询正常 |
| TC-17 | FR-7 | 集成 | P1 | 同 TC-16 场景 | 已注册实例发心跳；另发查询请求 | 心跳 200（心跳不落盘、不因落盘失败报错）；查询正常 |
| TC-18 | NFR-5/AC-14 | 集成 | P1 | data 目录预置：① 有效旧快照；② 同名 `.tmp` 半写残留（模拟写盘中断） | 启动恢复 | 恢复读**旧有效快照**（不读 tmp、不整表清空）；首次落盘后 tmp 残留被正常 rename 覆盖、目录无临时文件滞留。辅以代码审查确认「写临时文件 + rename」实现（AC-14 审查项） |
| TC-19 | FR-2/B-8 | e2e | P1 | registry 运行中，`FLUSH_INTERVAL_SECS=30`（长周期放大窗口） | ① 注册 A；② 立即 SIGTERM；③ 等进程退出后检查快照文件 | 优雅退出（exit 0）；文件含 A（关停最终刷盘）；若刷盘失败仅日志、不影响退出（尽力而为语义） |

### B 组：watch 长轮询

| TC | 所属 AC/FR | 类型 | 优先级 | 前置条件 | 步骤 | 期望结果 |
|----|-----------|------|--------|---------|------|---------|
| TC-20 | FR-8/AC-8 | 集成 | P0 | 已注册实例使当前版本 index=n | `GET watch?index=n-1&timeout_secs=5` | **立即返回**（<1s）；HTTP 200；响应 `index`=n；快照含既有实例 |
| TC-21 | FR-8/AC-8 | 集成 | P0 | 当前版本 index=n；发起 `watch?index=n&timeout_secs=30` 挂起 | 挂起期间另一连接注册新实例 X | 挂起请求在 1s 内返回（CI 容差 ≤2s）；响应 `index`=n+1；快照含 X |
| TC-22 | FR-10/AC-11 | 集成 | P0 | — | 分别请求 `watch?index=0&timeout_secs=0`、`timeout_secs=301` | 均返回 400 + 统一错误结构（`{"code":...,"message":...}`） |
| TC-23 | FR-10/AC-11 | 集成 | P0 | 当前版本 index=n | `watch?index=n`（**不传** timeout_secs），观察 3s | 请求保持挂起（3s 内无响应、连接未断）＝按默认 30s 处理；随后注入一次变更验证挂起期间唤醒正常（不必真等 30s） |
| TC-24 | FR-8/FR-10 | 集成 | P1 | 无任何变更；`timeout_secs=2`（合法小值） | `watch?index=n&timeout_secs=2` | 约 2s 后返回 HTTP 200，`index` 仍为 n（原版本号），快照为当前快照；不报错 |
| TC-25 | FR-8/B-7 | 集成 | P1 | ① 挂起态下 kill 重启 registry（版本号回落）；或② 直接以 index=n+100 请求 | 携带 index **大于**当前版本的请求 | 立即返回（不挂起）当前快照，响应以**新的当前 index** 纠正客户端（重启后版本从 0 重计语义） |
| TC-26 | FR-9/AC-10 | 集成/e2e | P1 | TTL=2s、sweep=1s（短周期加速收敛）；实例 A 已注册并停发心跳；挂起 `watch?index=n&timeout_secs=30` | 等 sweep 移除 A | 挂起请求被唤醒；响应 index=n+1；快照不含 A |
| TC-27 | FR-9 | 集成 | P1 | 挂起 `watch?index=n&timeout_secs=30` | 另一连接 DELETE 实例 | 挂起请求 1s 内唤醒；index=n+1；快照不含被注销实例 |
| TC-28 | FR-9/AC-10 | 集成 | P1 | 当前版本 n；挂起 `watch?index=n` | 连续多次 GET `/healthz`、`/services`、`/instances`、`?healthy=true` | 3s 观察窗内挂起请求不被唤醒；再发普通 watch（index<n）确认 index 仍=n（纯查询不递增版本号） |
| TC-29 | FR-9/Q4 附注 | 集成+审查 | P2 | 挂起 `watch?index=n`；实例 A 心跳临近 TTL | 观察 healthy 翻转窗口（TTL 到期至 sweep 执行之间）不触发唤醒 | A 的 healthy 标志在列表中翻转为 false，但 index 不递增、挂起不唤醒（健康翻转不独立触发，Q4 附注默认方案）；辅以代码审查确认 bump 仅挂接于注册/注销/剔除三事件 |
| TC-30 | FR-8/AC-9 | e2e | P0 | registry 运行中；后台 curl 发起挂起 `watch?index=n&timeout_secs=30` | 向进程发 SIGINT（ctrl_c 等价）；观察 curl 连接与进程 | 挂起连接立即被唤醒/断开（curl 秒退，非等满 30s）；进程正常退出（exit 0）；日志无 panic |
| TC-31 | FR-8/B-6 | 集成 | P1 | 发起挂起请求后**主动中止**该连接（模拟客户端断开）；中止期间另一连接注册实例 X | 重新携带原 index=n 请求 | 新请求立即返回（不挂起），快照含 X、index 正确；服务端无残留会话（再次挂起行为正常） |
| TC-32 | NFR-1 | 集成 | P1 | 50 个并发挂起 watch（index=当前） | 挂起保持期间并发执行 20 次 healthz/列表查询并测延迟 | 查询全部正常返回且延迟无异常放大（tokio worker 未被长轮询占死）；随后注册一次实例验证 50 个挂起全部被唤醒 |
| TC-33 | FR-8/B-11 | 集成 | P1 | — | ① 并发注册 50 个同服务实例；② 期间多个并发挂起 watch；③ 结束后全量查询 + watch(index=0) | ① 50 实例全部成功、id 全唯一；② 无 panic/死锁；③ 快照含全部实例、index 单调递增无回退 |
| TC-34 | FR-2 | 集成 | P2 | 并发心跳（多实例高频）与周期刷盘并行 | 刷盘期间多次读取快照文件并解析 | 每次读到的文件心跳时间戳不早于先前读取（落盘串行化，文件不回退旧快照）；文件始终为合法 JSON |

### C 组：v0.1 兼容回归与可观测性

| TC | 所属 AC/FR | 类型 | 优先级 | 前置条件 | 步骤 | 期望结果 |
|----|-----------|------|--------|---------|------|---------|
| TC-35 | FR-11/AC-12 | 集成 | P0 | 开启持久化 + watch 的新版本构建 | 全端点回归：healthz；注册；heartbeat；deregister；list services（排序）；`instances?healthy=true` 过滤；未知服务/未知实例错误 | 路径/方法/请求响应 JSON 结构/状态码与 v0.1 完全一致；未知服务 404 + `service_not_found`；未知实例 404 + `instance_not_found`；v0.1 既有内嵌测试（`register_and_query_flow` 等）保持通过 |
| TC-36 | FR-11/AC-12 | e2e | P0 | velox-demo 全链路（mesh 不改一行代码） | 运行 `velox-demo/scripts/acceptance.sh`（registry 开持久化/watch 默认配置） | 既有断言链全部通过：mesh 快照拉取、consumer 经 mesh 转发、LB 轮询、SIGKILL 剔除、SIGTERM 注销 |
| TC-37 | FR-11 | 单元 | P0 | — | `Config::load(None)` 断言默认值 | `server.addr`、`registry.heartbeat_ttl_secs`、`sweep_interval_secs` 与 v0.1 相同；新增 persistence 节默认 enabled=true/`./data`/5s（Q1 建议） |
| TC-38 | NFR-3/AC-13 | e2e | P1 | 自建轻量压测脚本（EXECUTE 阶段编写，见 RQ-2 手段局限） | ① 对 v0.1 代码采基线；② 对 v0.2 同条件压测：注册 1000 实例 + 5s 心跳稳态 + 并发查询，采 P99 | 注册/心跳/查询 P99 ≤ 基线 × 2；刷盘无写放大风暴（单次全量快照百 KB 量级）。结果标注参考级（非专业压测工具） |
| TC-39 | NFR-4/AC-14 | e2e | P1 | registry 以含存量数据 data_dir 启动一次（含 1 个超时实例） | grep `logs/registry.log` 启动段 | 日志含：持久化启用标志、data_dir、恢复服务数/实例数、丢弃超时实例计数、跳过损坏文件计数（结构化字段） |
| TC-40 | FR-8/FR-10 | 集成 | P2 | — | watch 端点：① POST；② `index=abc`；③ `timeout_secs=abc` | ① 405（方法不允许，axum 默认）；②③ 400 + 统一错误结构。注：**不带 index 参数**的缺省行为需求未定义（RQ-3），定案前本条不断言该分支 |
| TC-41 | B-12/FR-1 | 集成 | P2 | — | 注册 metadata 含 UTF-8 中文/空格/特殊字符的实例 → 落盘 → 重启恢复 | 恢复后 metadata 与注册值逐字节一致；快照文件为合法 JSON（无字符集损坏） |

## 4. AC 覆盖矩阵（AC × TC）

| AC | 映射 TC | P0/P1 缺口 |
|----|---------|-----------|
| AC-1 | TC-01(P0)、TC-03(P1)、TC-04(P0) | 无 |
| AC-2 | TC-02(P0) | 无 |
| AC-3 | TC-05(P1)、TC-06(P1) | 无（P1 覆盖；「响应不等待刷盘」由 TC-05③ 观察 + 代码审查） |
| AC-4 | TC-07(P0)、TC-08(P1)、TC-09(P1) | 无 |
| AC-5 | TC-10(P0)、TC-11(P0)、TC-12(P0) | 无 |
| AC-6 | TC-13(P0) | 无 |
| AC-7 | TC-16(P0)、TC-17(P1) | 无（期望值依赖 RQ-1 定案回填） |
| AC-8 | TC-20(P0)、TC-21(P0)、TC-25(P1) | 无 |
| AC-9 | TC-30(P0) | 无 |
| AC-10 | TC-26(P1)、TC-27(P1)、TC-28(P1) | 无 |
| AC-11 | TC-22(P0)、TC-23(P0)、TC-24(P1) | 无 |
| AC-12 | TC-35(P0)、TC-36(P0)、TC-37(P0) | 无 |
| AC-13 | TC-38(P1) | 无（手段缺失见 RQ-2） |
| AC-14 | TC-18(P1)、TC-39(P1) | 无（精确崩溃窗口以「tmp 残留模拟 + 代码审查」组合，见第 2 章风险 4） |

结论：14 条 AC 全部映射，每条至少 1 条 P0 或 P1 用例，无覆盖缺口。

## 5. 边界场景覆盖对照（需求第 5 节 B-1…B-12）

| 边界 | 覆盖用例/说明 |
|------|--------------|
| B-1 落盘失败 | TC-16（注册 5xx + 内存优先 + ERROR）、TC-17（心跳不受影响） |
| B-2 恢复文件损坏/半写 | TC-13（三形态损坏逐场景）、TC-18（tmp 残留不污染恢复） |
| B-3 data 目录不可创建 | TC-15（fail fast 含路径） |
| B-4 宕机 > TTL 全丢弃 | TC-09（含 provider 心跳 404 现状记录） |
| B-5 provider 无重注册 | TC-09 附加观察项（仅记录行为，不判缺陷；修复属 velox-demo 范围外） |
| B-6 watch 客户端断开/重连 | TC-31（无服务端会话残留） |
| B-7 版本号回落（重启） | TC-25（index > 当前 → 立即返回 + 纠正） |
| B-8 关停竞态（最终刷盘 + watch 唤醒） | TC-19（最终刷盘尽力而为）、TC-30（挂起全断开 + exit 0） |
| B-9 时钟回拨 | **不设用例**：需求明示单机假设下不防御、文档声明已知限制；交付时核对 README 含该说明 |
| B-10 多进程共享 data_dir | **不设自动化用例**：部署约束（单实例独占），属文档审查项；交付时核对 README/部署文档明示 |
| B-11 并发注册/并发 watch 唤醒 | TC-33（并发注册 + index 单调）、TC-32（并发挂起不拖垮服务）、TC-34（刷盘不回退） |
| B-12 国际化/字符集 | TC-41（metadata 特殊字符 roundtrip） |

## 6. 需求问题清单（RQ-x）

> 原则：只记录、不臆断修正需求；处置权在 GATE2（QA）与流水线。

### RQ-1（低）：AC-7 的 5xx 具体状态码与错误 code 待 AA 定案，用例期望需回填
- **描述**：AC-7 写「注册按 Q3 定案语义响应」，Q3 仅给建议默认（5xx）；GATE1 评审 QA-6 已指出应具体化（如 500 + `persist_failed`），但截至本工序尚未定案。
- **证据**：`01-requirements.md` §7 Q3、§4 AC-7；`01-requirements-review.md` QA-6（「定案时明确如 500 + persist_failed，并纳入 error.rs 变体清单」）。
- **影响**：TC-16 当前按建议默认「500 + `persist_failed`」预写期望；若 AA 定案为其他状态码/code（如 503），执行前需回填 TC-16（及 error.rs 相关断言），不影响用例结构。同性质依赖：watch 响应字段名/文件名（Q4/Q6）均为建议形态，TC 已标注「以 AA 定案为准」。

### RQ-2（中）：AC-13 压测验收手段缺失——无压测工具、无 v0.1 基线、采集方法未定义
- **描述**：AC-13 要求「1000 实例、5s 心跳压测，P99 ≤ v0.1 同条件基线 × 2」，但仓库无任何压测资产（无 criterion、无负载脚本），v0.1 基线数据未留存，「同一压测条件下对比」的具体采集方法（并发数、采样窗口、测量点）需求未定义。
- **证据**：`01-requirements.md` §4 AC-13、§3 NFR-3；`scripts/`（test/common/build/run）与 `velox-demo/scripts/`（仅 acceptance.sh）资产清单。
- **影响**：AC-13 无法以现有 CI 门槛自动判定。建议二选一：① 流水线批准 EXECUTE 阶段自建轻量压测脚本（curl 并发或独立 rust bin，先对 v0.1 采基线），结果标注参考级；② 明示 AC-13 降级为验收期人工专项。当前用例 TC-38 按 ① 预设，若裁决为 ② 则 TC-38 移出自动化范围。

### RQ-3（低）：watch 请求未携带 index 参数时的行为未定义
- **描述**：FR-8 端点形态为 `watch?index={n}&timeout_secs={t}`，AC-11 仅定义 `timeout_secs` 缺省按 30s 处理；客户端**不传 index** 的行为（404/400/按 index=0 立即返回）需求未写。新客户端首次订阅是该场景的自然入口。
- **证据**：`01-requirements.md` §2 FR-8、§4 AC-8/AC-11（仅覆盖「携带 index」与「timeout_secs 缺省」）。
- **影响**：影响 TC-40 断言范围与实现分支。建议定案为「缺省按 index=0 处理 → 立即返回当前快照」（与 B-7「index 过旧立即返回」语义一致）；定案前 TC-40 不断言该分支。
