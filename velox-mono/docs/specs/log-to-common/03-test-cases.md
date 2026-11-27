# 功能测试案例（v0 草案）：log-to-common（velox-log 重构为 velox-common）

> 状态：**v0 草案**——基于原始功能诉求「重构 velox-log，修改为 velox-common，log 只是其中一个组件」，尚未与 01-requirements.md 的 AC 对齐（该文档尚不存在）。本草案与 F-SA 并行消费同一原始诉求，后续 F-TA-D 以冻结 AC 为准升级 v1 定稿。

---

## 1. 功能点与案例总览（FC-x 案例组）

| 功能点 | 描述（业务视角） | 案例编号 |
| -------- | ------------------ | ---------- |
| FC-1 | **workspace 结构重构本体**：velox-log 不再作为独立 crate 存在（目录与 workspace member 移除）；新目录 `velox-common/` 入驻 workspace，日志能力作为其中一个组件（模块）迁入；目录/导出结构为后续公共组件预留扩展位 | TC-01、TC-02 |
| FC-2 | **log 组件能力零回退**：rotate/filter/report/bounded 四能力及测试语义全保留——硬边界轮转（3×200 MiB）、等级链解析（RUST_LOG > `VELOX_*__LOG__LEVEL` > 文件 > warn）、无反馈运维报告、lossy 队列（1024 条）；28 例既有测试随迁且全绿 | TC-03~TC-06 |
| FC-3 | **五包接入点切换且运行时行为不变**：velox-config、velox-registry、velox-mesh、velox-demo provider/consumer 的 import 与初始化改走 velox-common（或其 log 组件再导出）；五个二进制日志行为（落盘文件名/等级链/env 覆盖）与重构前完全一致 | TC-07~TC-09 |
| FC-4 | **依赖图与工程卫生**：workspace 依赖 path 更新、Cargo.lock 同步、源码与活文档零 velox-log 残留、CI 四门槛（fmt/clippy/build/test）全绿、工作树纪律不被破坏 | TC-10~TC-12、TC-16 |
| FC-5 | **端到端活体验证**：五进程日志隔离落盘、demo acceptance 全链路、既有 flaky 用例基线隔离 | TC-13~TC-14 |
| FC-6 | **mono 文档同步**：活文档（ADR/规范/subsystems.md/development.md）中 velox-log 引用更新；历史规格文档处置口径待仲裁（OP-3/OP-4） | TC-15 |

覆盖关系：FC-1~FC-6 共 16 条功能案例（TC-01~TC-16）。实现相关判定口径（如再导出路径形态、扩展位形态）暂留白，待 AC 冻结后在 TAD 定稿落位。

---

## 2. 功能案例表（TC-x）

> 列说明：功能点=所属 FC 组；场景=业务场景；操作=可执行验证动作；预期结果=可观察判定。标 **【留白】** 处为实现相关预期，待 AC/架构定稿后补齐判定口径。

| 编号 | 功能点 | 场景 | 操作 | 预期结果 | 优先级 |
| ------ | -------- | ------ | ------ | ---------- | -------- |
| TC-01 | FC-1 | velox-log 独立 crate 退场 | ① 检查根 `Cargo.toml` workspace `members`；② 检查 `velox-log/` 目录是否存在；③ `cargo metadata`/构建树确认无 velox-log 节点 | ① members 不含 `velox-log`、含 `velox-common`；② `velox-log/` 目录不复存在；③ 依赖图中 velox-log 消失、velox-common 出现 | P0 |
| TC-02 | FC-1 | velox-common 目录与组件扩展位 | ① 检查 `velox-common/` 目录结构与 crate 清单（Cargo.toml/src 布局）；② 检查导出面：log 是否为 velox-common 的一个子模块/再导出组件；③ 评估结构是否可容纳后续公共组件（新增组件不扰动 log） | ① velox-common 为 workspace 成员、crate 元信息（version/edition/rust-version）继承 workspace；② log 以组件形态挂入（具体路径形态**【留白】**：`velox_common::log::…` 模块路径 vs 顶层再导出，待 AC 定）；③ 结构预留扩展位可判定（形态**【留白】**：如组件模块化布局/占位，验收口径由 AC 冻结） | P1 |
| TC-03 | FC-2 | 28 例既有日志测试随迁全绿 | ① 定位随迁后的测试资产（crate 内 `mod tests` 或等价位置）；② `cargo test -p velox-common`（或等价口径，见 OP-5）执行；③ 比对 28 例用例名与 addlog 基线清单（见侦察笔记 §3.4） | ① 测试资产完整随迁、无遗失；② 28 例全部通过；③ 用例集合与基线一致（允许仅 import 路径机械适配，语义断言零改动——适配边界口径见 OP-5） | P0 |
| TC-04 | FC-2 | 轮转硬边界语义不回退 | 以注入小阈值（`RotationLimits`）复跑轮转/保留用例：边界轮转、含活跃共 3 份、单文件硬字节上界（0 超限容差）、超长单事件截断（R=min(M,64 KiB)）、旧超限文件启动规范化收敛 | 与 addlog 基线行为逐项一致：轮转触发点、文件数恒 ≤3、逻辑长度恒 ≤M、截断标记 ` [truncated]`、启动收敛不失败（对齐 addlog TC-04/05/03/07 语义） | P0 |
| TC-05 | FC-2 | 等级链解析语义不回退 | 以 EnvInputs 注入复跑等级链用例：优先级表（RUST_LOG > `VELOX_*__LOG__LEVEL` > 文件显式 > warn）、高层合法覆盖低层非法、空串回落、非法值 fail-fast（错误含实际值与准确来源）、off 级别 | 与 addlog 基线逐项一致（对齐 addlog TC-11~TC-16 语义），含来源绑定记录（Default/File 绝对路径/env 具体变量名），生效来源不冒充 | P0 |
| TC-06 | FC-2 | 运维报告与有界队列语义不回退 | 复跑 report/bounded 用例：轮转/删除/截断/IO 事件 stderr 直达且不回流日志、按类别限频、队列满丢弃计数外显（无背压反压）、生产者停止后无反馈、guard drop 有界等待预算 | 与 addlog 基线逐项一致（对齐 addlog TC-24/25 及报告组语义）；无反馈原则保持（报告不调用 tracing） | P1 |
| TC-07 | FC-3 | 五包 import 接入点切换 | ① `rg 'velox_log'` 全 workspace 源码扫描；② 逐包检查 Cargo.toml 依赖项；③ `cargo build --workspace` | ① 五包（config/registry/mesh/demo provider+consumer）源码无 `velox_log::` 残留，改用 velox-common（或其 log 组件）路径（具体形态**【留白】**，见 OP-1）；② 各包依赖项指向 velox-common；③ 编译零错误零警告 | P0 |
| TC-08 | FC-3 | 五二进制运行时日志行为不变 | 分别启动五个二进制（默认配置与 env 覆盖各一轮抽查）：断言落盘路径/文件名、等级链行为、`VELOX_*__LOG__DIR`/`__LOG__LEVEL` 覆盖 | ① 落盘 `logs/<包名>.log`：`velox-mesh.log`/`velox-config.log`/`velox-registry.log`/`demo-provider.log`/`demo-consumer.log`（**文件名五份不变**——stem 取自各二进制包名，共享库更名不影响）；② 等级链/env 覆盖行为与重构前一致；③ 五包 prefix（CONFIG/REGISTRY/MESH/PROVIDER/CONSUMER）解析行为不变 | P0 |
| TC-09 | FC-3 | registry 集成测试夹具随迁适配 | ① 检查 `velox-registry/tests/common/mod.rs` 对 velox-common 新路径的引用；② 跑 registry 集成套件（`cargo test -p velox-registry`） | ① 夹具改用新路径（`LogConfig::default()`/`ResolvedLevel::default()` 等价物）；② registry 集成套件全绿，无因接入切换引入的失败 | P1 |
| TC-10 | FC-4 | workspace 依赖图一致性 | ① 根 `Cargo.toml` `[workspace.dependencies]`：velox-log path → velox-common path；② Cargo.lock 同步核对；③ `cargo build --workspace --locked` | ① 依赖表更新且统一从 workspace 引用（无散落版本）；② Cargo.lock 无 velox-log 残留条目、velox-common 条目正确；③ `--locked` 构建通过 | P0 |
| TC-11 | FC-4 | 源码与活文档零残留 | rg 双词（velox-log 与 velox_log）分层扫描：① 源码与 Cargo.toml；② 活文档（adr/architecture.md/development.md/subsystems.md 及 specs 下**在产**文档）；③ 历史规格（specs/addlog 等） | ①② 零残留（命中点全部更新）；③ 历史规格处置依 OP-3/OP-4 仲裁口径执行（建议默认：原样保留，不计残留） | P0 |
| TC-12 | FC-4 | CI 四门槛全绿 | ① `cargo fmt --all --check`；② `cargo clippy --workspace --all-targets -- -D warnings`；③ `cargo build --workspace`；④ `cargo test --workspace --no-fail-fast`（全量口径，本机 cargo 1.97 默认 fail-fast） | 四命令全部通过；与 `scripts/test.sh` CI 门槛口径一致（test 脚本是否需同步 `--no-fail-fast` 见 OP-6） | P0 |
| TC-13 | FC-5 | 五进程日志隔离与落盘（活体） | 从仓库根并行启动五服务（core 三件套经 run.sh + demo 两裸进程），各触发 ≥1 条可归属事件，清点 `logs/` 并抽查内容归属 | 五份实名日志独立存在、内容与进程对应、无串写交错、无多余副本（对齐 addlog TC-01/TC-02 手段） | P1 |
| TC-14 | FC-5 | demo acceptance 全链路（活体 e2e） | `bash velox-demo/scripts/acceptance.sh`（前置：7 端口空闲、curl/jq 在位）；执行前后记录 `defaults_match_contract` 既有 flaky 基线状态 | ① acceptance 全链路 PASS（健康检查→注册发现→热更→mesh 转发→扩容 LB→宕机剔除→优雅注销）；② 五服务日志落盘正常（脚本已依赖 `logs/<bin>.log` 实名机制）；③ `defaults_match_contract` 若命中 flaky，对照预检基线判定归属（demo 既有问题不计本 feature 缺陷） | P1 |
| TC-15 | FC-6 | mono 活文档引用同步 | 检查：① `docs/adr/0002-log-rotation.md`；② `docs/subsystems.md`（三处「子系统日志（addlog）」注释）；③ `docs/development.md:67`；④ 其他提及 log 机制的活文档 | ①②③④ 中涉及 velox-log crate 名的引用已同步更新为新结构口径；addlog 等历史术语是否顺带统一**【留白】**（待 AC 定）；ADR 处置方式依 OP-4（建议默认：新增 ADR + 旧 ADR 加 superseded 标注，不改写正文） | P1 |
| TC-16 | FC-4 | 工作树纪律 | `git status` 核对：① `velox-config/src/limit.rs` 既有 rustfmt 改动；② 未跟踪项 `_pipeline-state.md`、`logs/`、`data/`、`velox-mono/docs/brd/` | ① limit.rs 的既有改动不混入本 feature 提交；② 四类未跟踪内容不入库不被触碰（验收时仅状态核对，不做清理动作） | P2 |

---

## 3. 测试基建侦察笔记（框架、命令、夹具、e2e 手段）

### 3.1 重构对象：velox-log crate 现状

- 位置 `velox-log/`，workspace path member；`src/` 六文件共 3346 行：`lib.rs`(295) / `rotate.rs`(672) / `filter.rs`(242) / `report.rs`(370) / `bounded.rs`(113) / `tests.rs`(1654)。
- 公共 API 面（五包实际消费）：`LogConfig`、`ResolvedLevel`（含 `filter()`/`source_display()`）、`resolve_level(&LogConfig, Option<&Path>, prefix)`、`absolutize(&Path)`、`init(crate_name, &LogConfig, &ResolvedLevel) -> LogGuard`；常量 `MAX_FILE_BYTES`(200 MiB)/`KEEP_FILES`(3)/`MAX_EVENT_BYTES`(64 KiB)/`QUEUE_EVENTS`(1024)；内部模块 `bounded`/`filter`/`report`/`rotate` 为 pub mod。
- 依赖：tracing、tracing-subscriber、tracing-appender(=0.2.4)、serde、anyhow——均为 workspace 统一版本。重构后这些依赖归属 velox-common。
- 五包等级链 prefix 均为各包专有（CONFIG/REGISTRY/MESH/PROVIDER/CONSUMER），由各包 config.rs 传入，与共享库 crate 名解耦——共享库更名不触及该机制。

### 3.2 五包接入点清单（TAE 逐点核对依据）

| 包 | 依赖声明 | import / 调用点 | init 调用 |
| ---- | ---------- | ------------------ | ----------- |
| velox-config | Cargo.toml:11 | src/config.rs:4,101,106-107（prefix `CONFIG`） | src/main.rs:35 |
| velox-registry | Cargo.toml:11 | src/config.rs:10,129,134-135（prefix `REGISTRY`）；**tests/common/mod.rs:39-40**（测试夹具） | src/lib.rs:94 |
| velox-mesh | Cargo.toml:11 | src/config.rs:4,103,108-109（prefix `MESH`） | src/main.rs:43 |
| velox-demo/provider | Cargo.toml:11 | src/config.rs:7,132,137-138（prefix `PROVIDER`） | src/main.rs:38 |
| velox-demo/consumer | Cargo.toml:11 | src/config.rs:7,127,132-133（prefix `CONSUMER`） | src/main.rs:39 |
| 根 Cargo.toml | members 列表 + `[workspace.dependencies]` velox-log path | — | — |

注意：registry 是唯一在**测试代码**中直接引用 velox_log 的包（tests/common/mod.rs 夹具构造），接入切换必须覆盖测试资产，不能只改 src。

### 3.3 测试框架与命令

- 框架：Rust 内建 `#[test]`；velox-log 测试集中 crate 内 `src/tests.rs`（28 例 `#[test]`，无 async 测试）。
- CI 门槛：`scripts/test.sh` = fmt + clippy(--workspace --all-targets -D warnings) + build + test；另有 `--lint/--fast/--unit` 降级模式。
- 基线命令（本任务给定口径）：
  - `cargo build --workspace`
  - `cargo test --workspace`（**本机 cargo 1.97 默认 fail-fast，全量口径须 `--no-fail-fast`**）
  - `cargo fmt --all --check`
  - `cargo clippy --workspace --all-targets -- -D warnings`
  - `bash velox-demo/scripts/acceptance.sh`
- 执行口径建议：T0 用 `cargo test --workspace --no-fail-fast` 一把拿全量；分 crate 快筛用 `cargo test -p velox-common`（重构后包名，见 OP-5）。

### 3.4 28 例既有测试基线清单（随迁完整性比对依据）

`production_constants_and_injection`、`logconfig_raw_conversion_records_presence`、`level_chain_priority_table`、`level_chain_edge_cases`、`strict_syntax_whitelist`、`writer_rotates_and_caps_file_count`、`writer_exact_fill_rotates_on_next_record`、`writer_retention_removes_oldest`、`restart_appends_history_preserved`、`init_normalizes_oversize_and_legacy_copies`、`oversize_record_truncated_within_cap`、`identity_check_reopens_after_unlink`、`fault_remove_oldest_retries_same_step`、`fault_move_one_retries_without_double_shift`、`fault_open_failure_recovers_via_open_fresh`、`fault_write_error_keeps_prefix_and_reopens`、`fault_short_write_continues_within_record`、`fault_write_zero_transitions_reopen`、`fault_interrupted_retry_budget_then_reopen`、`fault_flush_failure_reopens_no_rollback`、`reopen_truncates_oversize_path`、`full_chain_event_cap_and_rotation_via_dispatch`、`queue_full_drops_counted_no_backpressure`、`no_feedback_after_producers_stop`、`guard_drop_returns_within_budget_when_worker_blocked`、`std_report_sink_rate_limits_by_category`、`fixture_child`、`live_rotation_concurrent`（28 例）。

### 3.5 夹具与可复用资产

- **FaultFs/FaultController**（tests.rs 内建）：按 op×调用序号注入 IO 故障（remove/rename/open/write/flush/metadata/set_len），含读关门/写关门并发门——故障语义用例核心夹具，随迁不可丢。
- **TestReportSink**：报告事件收集 sink，report 语义断言用。
- **临时目录夹具**：TempLogDir 带 drop 清理。
- **活体子进程自举夹具**：`current_exe()` 自举 + `VELOX_LOG_FIXTURE=1` 环境门（`fixture_child`/`live_rotation_concurrent`），真实 subscriber + NonBlocking + TCP 负载的并发轮转活体验证——重构后需确认该环境门机制仍按 crate 名/约定生效。
- **registry tests/common/mod.rs**：跨包测试夹具直接构造 `velox_log::LogConfig::default()`——接入切换的测试侧触点。
- **addlog 基线资产**：`docs/specs/addlog/03-test-cases.md`（TC-01~TC-25 v1 定稿）与 `04-test-report.md`——本 feature 行为零回退判定的语义参照，可复用其操作步骤设计（不直接复用编号）。

### 3.6 e2e 手段

- `velox-demo/scripts/acceptance.sh`：host 模式全链路（健康检查×6 → 注册/发现 → 直连 → 配置热更 → mesh 转发一致 → 扩容轮询 LB → SIGKILL 剔除 → SIGTERM 注销）；固定端口 8100/8101/8200/8300/9000/9001/9100；依赖 curl/jq；脚本自身已依赖「进程自身写 `logs/<bin>.log`」机制（步骤 4.6 显式清理 demo-provider-2 旧日志副本）——本 feature 不得破坏该契约。
- `velox-registry/tests/registry-e2e.sh`、`registry-perf.sh`：registry 专项脚本，本 feature 无关但全量回归时在 workspace 范围内。
- `scripts/run.sh` / `build.sh` / `common.sh`：core 三件套启停与构建；`scripts/README.md` 有 `RUST_LOG` 使用文档——文档同步核对点之一。

### 3.7 mono 文档引用点盘点（TC-15 核对清单）

- 活文档：`docs/adr/0002-log-rotation.md`（标题、D1 决策、workspace 依赖配置示例、`cargo tree -p velox-log` 证据——velox-log 字面多处）；`docs/subsystems.md` L61/L111/L171（三处「子系统日志（addlog）」配置注释，addlog 术语）；`docs/development.md:67`（「addlog，ADR-0002」机制描述）。
- 历史规格：`docs/specs/addlog/01~05`、`docs/specs/instance-concurrency-limit/04/05` 内 velox-log 引用——历史交付记录，处置口径见 OP-3。
- `architecture.md`、根 `README.md`：未发现 velox-log 字面引用（已核对）。

### 3.8 已知风险与既有问题（非本 feature）

- **既有 flaky**：`velox-demo/consumer/src/config.rs::defaults_match_contract`（env 全局状态互踩类，demo 范围既有）——执行轮先跑基线预检，命中时对照归属，不计本 feature 缺陷（OP-6）。
- **fail-fast 陷阱**：本机 cargo 1.97 `cargo test --workspace` 默认 fail-fast，首个失败 crate 会截断全量口径——全量回归必须 `--no-fail-fast`。
- **velox-config/src/limit.rs 既有未暂存改动**（rustfmt）：与本 feature 无关，验收时不得混入提交（TC-16）。
- 未跟踪内容不入库不触碰：`_pipeline-state.md`、`logs/`、`data/`、`velox-mono/docs/brd/`。

---

## 4. 开放问题（OP-x）

> OP 清单仅随 v1 定稿流入 GATE2 仲裁，不影响 GATE1。

| 编号 | 描述 | 影响 |
| ------ | ------ | ------ |
| OP-1 | 五包接入路径口径：velox-common 对 log 组件采用「模块路径 `velox_common::log::…`」还是「顶层再导出 `velox_common::LogConfig`」（或两者并存） | 影响 TC-07/TC-09 的操作与断言细节、五包 diff 面、后续组件扩展位的判定形态（TC-02）。建议默认：log 组件模块化 + 顶层再导出并存，具体由 AC/架构定 |
| OP-2 | 「velox-log 不再作为独立 crate 存在」的字面口径：是否允许保留薄再导出壳 crate（deprecation shim），还是目录与 crate 完全移除、零残留 | 影响 TC-01/TC-11 判定口径。建议默认：完全移除（workspace 内部库无外部消费者，无兼容包袱） |
| OP-3 | 历史 feature 规格（specs/addlog/01~05、instance-concurrency-limit/04/05）中 velox-log 引用是否回溯修订：改则破坏历史证据链，不改则仓库存在旧名引用 | 影响 TC-11 残留口径分层。建议默认：只改活文档（ADR/规范/开发文档/subsystems.md），历史规格原样保留并在其中不加注 |
| OP-4 | ADR-0002（log rotate 决策）处置方式：原位改写 vs 新增 ADR 记录本重构决策 + 旧 ADR 头部加 superseded 标注 | 影响 TC-15 判定方式与文档类 AC 的可验收形态。建议默认：新增 ADR + superseded 标注，不改写旧 ADR 正文 |
| OP-5 | 28 例测试随迁的组织口径：整体随迁 crate 内 `mod tests`，还是部分上移 workspace `tests/`；测试文件内 import 路径机械适配是否视为「语义零改动」的允许范围；分 crate 执行命令以重构后包名（`-p velox-common`）为准 | 影响 TC-03 执行口径与零回退比对的判定边界。建议默认：整体随迁 crate 内、仅允许 import 路径机械适配、断言语句逐例不变 |
| OP-6 | `scripts/test.sh` 的 test 阶段是否同步加 `--no-fail-fast`（本机 cargo 1.97 fail-fast 与全量口径冲突）：属工程脚本微调，是否纳入本 feature 范围 | 影响 TC-12 执行命令与 CI 门槛口径一致性。建议默认：纳入（一行改动），否则 TAE/FINAL 全量回归须绕过脚本直跑命令并留痕 |
| OP-7 | 既有 flaky `defaults_match_contract` 的执行轮基线协议：重构前后各跑基线预检记录现状，执行轮命中时按基线判定归属（demo 范围既有，非本 feature 引入） | 影响 TC-14 缺陷归属判定。建议默认：双向基线预检 + 命中即对照归属，不阻断本 feature 结论（必要时记录为遗留问题交 demo 责任方） |
