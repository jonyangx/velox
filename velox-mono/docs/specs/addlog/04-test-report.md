# 测试报告：addlog —— 子系统日志文件 rotate / 目录指定 / 等级调整

> Feature slug：`addlog` | 工序：F-TA-E（MODE=EXECUTE，常规执行轮）| 日期：2026-11-26
> **覆盖提交：`a5b75e5`**（`feat(log): 子系统日志本地 rotate（velox-log 共享库 + 五包接入）`；当时仓库 HEAD=`4b24a20`，其后 F3 提交 `4b24a20` 仅涉及 `velox-config` 并发上限校验与文档，未改动 `velox-log` 或 addlog 行为）
> **输入基准**：`01-requirements.md`（冻结 SHA `ce54e325…`）、`02-architecture.md`（定稿 SHA `a154e264…`）、`03-test-cases.md`（v1 定稿 SHA `1aaa6f0f…`，TC-01～TC-25 / AC-1～AC-14）、`02-architecture-review.md`（GATE2 PASS，含 P3-A1 执行期留痕要求）
> **测试基线声明**：当前工作树存在既有在途改动——`M velox-config/src/limit.rs`（纯 rustfmt 换行，无行为变化）及未跟踪规格/状态/报告文件（`_pipeline-state.md`、F2 验收与测试报告、F3 测试报告、`velox-mono/docs/brd/`），全部原样保留未提交；本轮所有测试与脚本验证均在此基线上运行，全部自建临时目录位于系统 `/tmp` 且已清理，仓库 `.gitignore` 覆盖 `logs/`/`.run/`/`data/`。

---

## 1. 测试结论

**有条件通过**

- **产品功能与冻结 AC 达成度**：`velox-log` 共享库与五个运行时二进制（`velox-mesh`、`velox-config`、`velox-registry`、`demo-provider`、`demo-consumer`）的本地文件落盘、硬字节边界轮转（200 MiB / 含活跃共 3 份 / 0 字节超限容差 / 64 KiB 超长截断 / 重启旧文件收敛）、目录自动创建与覆盖、等级优先级链（`RUST_LOG > VELOX_*__LOG__LEVEL > 文件 > warn`、高合法覆盖低非法、空串与 `warnn` 严格拒绝、`RUST_LOG` 裸 target 兼容、`off` 静默）、旧配置兼容、无反馈运维报告、IO 故障状态机、`scripts/run.sh` 去无界副本、`velox-demo/scripts/acceptance.sh` 第二 provider 分目录与旧副本清理、以及 mono ADR-0002/文档同步全部实测满足（**AC-1～AC-14 产品行为零缺陷**）。
- **“有条件”原因（新增 1 项测试资产缺陷）**：捕获可复现的 **BUG-1（中，测试资产）**——`velox-log/src/tests.rs` 的 TC-06 活体夹具 `tests::live_rotation_concurrent` 在 macOS/BSD 上因 `listener.set_nonblocking(true)` 被 `accept()` 继承且 `handle_fixture_conn` 未还原阻塞模式，并发调度下偶发 `clientX emit 响应异常` panic（本轮 T0 首跑即复现 1 次，6 并发压测复现 1/18），且父测试缺少 `Child` RAII Drop 清理，每次失败确定性泄漏 `fixture_child` 孤儿进程（本轮实证残留 PID 24907/27150）。交 DA 修复测试夹具后于终验轮复验。
- **跨包已知项**：workspace 回归仅复现既有 `demo_consumer::config::tests::defaults_match_contract` env 竞态 flaky（`--test-threads=1` 隔离 11/11 全绿），非本 feature 引入。

---

## 2. 档位执行说明（T0~T3 触发判定与耗时）

| 档位 | 执行状态 | 触发条件判定理由 | 执行内容与结果 | 实际耗时 |
|---|---|---|---|---|
| **T0 基础** | **已执行** | 必跑政策；验证 `velox-log` 28 例单测/集成/故障注入/活体夹具及五包配置测试 | `cargo test -p velox-log` 首轮 **27/28**（`tests::live_rotation_concurrent` FAILED → 定位为 **BUG-1**）；随后 `cargo test -p velox-log -- --test-threads=8` ×2 轮 **28/28 ×2 绿**、全套件连续 15 轮 **28/28 绿**；6 并发压力定向复现 BUG-1（1/18，完整捕获堆栈与孤儿进程证据） | ~35s（含复现压测） |
| **T1 冒烟** | **已执行** | **触发成立**：`a5b75e5` 改造了全部 5 个运行时二进制的配置加载与日志启动装配（`spawn_blocking` init + `JoinHandle` 关停 + 实名 5 份 TOML），需真实二进制进程级验证启动、等级链、fail-fast 与 `RUST_LOG=info` 字段 | 启动全部 5 个真实二进制（隔离端口 18501～18545、隔离 `/tmp` cwd/logs/data、清空外层 `RUST_LOG`/`VELOX_*`）：TC-01 裸进程真实 TTL WARN 落盘、TC-02/17/20 五进程隔离与 `RUST_LOG=info` 初始化字段全检 + 默认 `warn` 静默、TC-08/09/10 多级 mkdir/覆盖/占用文件 fail-fast、TC-12 真实业务 `error!` 落盘、TC-13/14/15/16 等级链 13 场景（含 `warnn`/空串拒绝/`RUST_LOG=warnn` 接受/`off`）、TC-18 旧配置+未知字段——**全部通过** | ~18s |
| **T2 全量回归** | **已执行** | **触发成立**：变更横跨全部 5 个业务 crate + 新增共享 `velox-log` crate + 根 `Cargo.toml`/`Cargo.lock` + 两套脚本 + mono 文档，变更面广 | `cargo fmt --all --check`、`cargo clippy --workspace --all-targets -- -D warnings`、`cargo build --workspace --locked` 三项全绿；`cargo test --workspace --locked --no-fail-fast`（共 130 例）：除已知 `demo_consumer::config::tests::defaults_match_contract` flaky 外 **129/129 全绿**（`demo-consumer --test-threads=1` 隔离 **11/11 全绿**）；TC-19 依赖锁图与 TC-23 文档/五份实名 TOML/ADR-0002 逐项核对通过 | ~10s |
| **T3 e2e** | **已执行** | **触发成立**：`a5b75e5` 改造了 `scripts/run.sh`（stdout=/dev/null + stderr 匿名管道 + `tail -F`）与 `velox-demo/scripts/acceptance.sh`（第二 provider 分目录 + 旧副本清理），需真实脚本级端到端验证 | TC-21：`bash scripts/run.sh start` → `status` → `logs registry -n 5` → `restart` ×2 → `stop`，实证 `logs/` 仅 3 个受管 `.log` 文件、无 `nohup.out`/`*.stderr.log`、`velox-registry.log` 含真实 WARN；TC-22：预置旧副本 `logs/demo-provider-2.log` 与 `.stderr.log` 后运行 `bash velox-demo/scripts/acceptance.sh`：**PASS**，旧副本自动精确清理，第二 provider 落入 `logs/demo-provider-2/demo-provider.log`，5 进程受管文件与真实 WARN 均成立，退出无残留进程 | ~28s（run.sh 3s，acceptance 25.1s） |

> 说明：本 feature 无 TC-38 类性能比值专项（NFR-1 磁盘硬上界与 NFR-2 异步不拖垮数据面由 TC-06/07/24 覆盖）；T0～T3 全部实跑，无跳档、无降级。

---

## 3. 用例执行结果（对照 03-test-cases.md 25TC / 14AC 矩阵）

| TC 编号 | 功能点 / 对应 AC | 执行层级 | 结果 | 实测证据摘录 |
|---|---|---|---|---|
| TC-01 | 默认落盘（真实 WARN 探针）/ AC-1 | T1/e2e | **通过** | 干净 `/tmp` cwd 裸启动 `velox-registry`（TTL=2s/sweep=1s、默认 warn、非 `run.sh`），注册 `probe-svc` 后不续约；`<cwd>/logs/` 仅生成 `velox-registry.log`，捕获真实 WARN `instance expired, removed service="probe-svc"`，无 INFO 泄漏 |
| TC-02 | 5 进程隔离（实名五份）/ AC-2 | T1/e2e | **通过** | 实名五份 toml 并行启动 5 个二进制，共享同一隔离 `logs/` 目录：精确生成 `velox-mesh.log`、`velox-config.log`、`velox-registry.log`、`demo-provider.log`、`demo-consumer.log`（无多余运行名副本、无 ANSI、无串写）；`RUST_LOG=info` 与默认 `warn` 双模式均核验通过 |
| TC-03 | 重启续写与旧文件收敛 / AC-3、AC-4 | T0/集成 | **通过** | `velox-log` 单测 `restart_appends_history_preserved` 与 `init_normalizes_oversize_and_legacy_copies` 稳定全绿（续写不清空、超限文件启动前 `set_len(M)`+WARN、精确清理 `.log.N(N≥3)` 与 `<stem>.stderr.log` 不误删他包） |
| TC-04 | 精确阈值与硬上界（0 容差）/ AC-3 | T0/单元+集成 | **通过** | `writer_rotates_and_caps_file_count`、`writer_exact_fill_rotates_on_next_record`、`oversize_record_truncated_within_cap`、`full_chain_event_cap_and_rotation_via_dispatch` 全绿（0 字节超限容差、UTF-8 前缀+` [truncated]` 截断、`.1→.2` 固定推进） |
| TC-05 | 保留上限（含活跃共 3）/ AC-4 | T0/单元 | **通过** | `writer_retention_removes_oldest` 稳定通过（滚动 ≥4 次总数恒 ≤3、最旧 `.2` 删除、ReportSink 可观测） |
| TC-06 | 轮转并发稳定（活体夹具）/ AC-5 | T0/e2e 自举 | **偶发失败（BUG-1）** | `tests::live_rotation_concurrent` 在默认线程首跑失败、15 轮单独串跑与 8 线程 ×2 轮通过，但 6 并发压力下稳定复现 1/18：`velox-log/src/tests.rs:1545 client6/client0 emit 响应异常` 并泄漏 `fixture_child` 孤儿进程（PID 24907/27150）。根因为测试夹具本身在 macOS `accept` 继承 `O_NONBLOCK` 及缺少 `Child` Drop Guard（详见 §5 BUG-1），非 `velox-log` 轮转实现问题 |
| TC-07 | 磁盘占用上界（逻辑字节口径）/ AC-13、G2-1 | T0 + T3 | **通过** | T0 `writer_rotates_and_caps_file_count`/`full_chain_event_cap_and_rotation_via_dispatch` 断言所有受管文件逻辑字节均 ≤M 且总数 ≤3（逻辑总和 ≤3M）；T3 `run.sh` 与 `acceptance.sh` 全目录清点确认无 `stderr.log`/`nohup.out`/未受管副本 |
| TC-08 | 目录自动创建 / AC-6 | T1/集成 | **通过** | 真实 `velox-config` 指向不存在多级目录 `<tmp>/a/b/c/logs` 启动，经完整 `velox_log::init` 自动创建全部父目录并正常写入 `velox-config.log` |
| TC-09 | 目录覆盖 / AC-7 | T1/集成 | **通过** | 真实 `velox-config` 实测：`[log].dir = X` 落 `X/velox-config.log`；同时设 `VELOX_CONFIG__LOG__DIR = Y` 时覆盖落 `Y/velox-config.log`；均不设时默认落 `<cwd>/logs/`（TC-01 实证） |
| TC-10 | 目录创建失败 / FR-3、E1 | T1/集成 | **通过** | 真实 `velox-config` 将 `VELOX_CONFIG__LOG__DIR` 指向已有普通文件 `<tmp>/occupied_file`：`rc=1` fail-fast，stderr 含完整路径与底层原因（`创建日志目录 ... 失败` / `File exists (os error 17)`），不静默降级 |
| TC-11 | 默认 warn / AC-8 | T1 + T3 | **通过** | TC-01 裸进程与 TC-02 五二进制在无配置/env 下全部 INFO 静默（0 条 `INFO`/`日志初始化完成`），TC-01 与 TC-21/22 `velox-registry.log` 真实业务 WARN 可见 |
| TC-12 | ERROR 级默认可见（正常 `error!` 探针）/ AC-8 | T1/e2e | **通过** | 真实 `velox-config` 默认 `warn` 下，以 `data/ioerr.json` 目录诱发 `StoreError::Io` 触发正常业务 `tracing::error!`：`velox-config.log` 精确落盘 `ERROR velox_config::error: config store internal error`（同时 INFO 保持静默，未使用写失败 stderr 代替） |
| TC-13 | 三渠道生效与严格优先级 / AC-9 | T0 + T1 | **通过** | T0 `level_chain_priority_table` + T1 真实二进制 3 场景全绿：仅文件 `info`（`source=file:<abs_path>`）、仅 `VELOX_CONFIG__LOG__LEVEL=info`（`source=env:VELOX_CONFIG__LOG__LEVEL`）、三渠道并存（`RUST_LOG=debug` 胜出、`source=env:RUST_LOG value=debug`） |
| TC-14 | 高层合法覆盖低层非法、空串回落 / AC-9 | T0 + T1 | **通过** | T0 `level_chain_edge_cases` + T1 真实二进制 3 场景全绿：文件 `warnn`+`LOG__LEVEL=info` 成功（来源 `env:VELOX_CONFIG__LOG__LEVEL`）；文件 `warnn`+`LOG__LEVEL=""`+`RUST_LOG=debug` 成功（来源 `env:RUST_LOG`）；文件 `info`+`RUST_LOG=""` 回落文件 `info` |
| TC-15 | 非法值 fail-fast 与两套语法分流 / AC-10 | T0 + T1 | **通过** | T0 `strict_syntax_whitelist`/`level_chain_edge_cases` + T1 真实二进制 6 场景全绿：文件 `warnn`/空串、`LOG__LEVEL=warnn`/空串（含 `RUST_LOG=""` 并存）均 `rc=1` 且 stderr 含实际值与准确来源（`file:<abs_path> key=log.level` 或 `env:VELOX_CONFIG__LOG__LEVEL`）；`RUST_LOG=warnn` 作为裸 target 合法接受；`RUST_LOG=svc=warnn` `rc=1` 拒绝 |
| TC-16 | off 级别 / FR-4、E13 | T1/e2e | **通过** | 真实 `velox-config` 以 `VELOX_CONFIG__LOG__LEVEL=off` 启动并触发真实 429 业务 WARN：进程正常运行、HTTP 429 生效，日志文件保持 0 字节（业务日志完全静默） |
| TC-17 | 五二进制口径一致 / FR-5 | T1/e2e | **通过** | 全部 5 个运行时二进制在 `RUST_LOG=info` 与默认 `warn` 双模式下表现完全一致，均经统一 `velox_log::init` 输出同构 `日志初始化完成` 字段且默认 `warn` 静默 INFO |
| TC-18 | 旧配置兼容 / AC-11 | T0 + T1 | **通过** | T0 `logconfig_raw_conversion_records_presence` + T1 真实 `velox-config` 加载无 `[log]` 节且含未知字段 `unknown_legacy_key = 42` 的 TOML：启动成功，行为等同内置默认 `logs/warn` |
| TC-19 | CI 与默认值/常量单测 / AC-12 | T0 + T2 | **通过** | ① fmt/clippy/`build --locked`/`test --workspace --locked` 除已知 demo flaky 外全绿；② `production_constants_and_injection` 验证 `MAX_FILE_BYTES=209715200`、`KEEP_FILES=3`、`record_cap=65536`、`QUEUE_EVENTS=1024` 与 `RotationLimits::injected`；③ 5 份实名 TOML 均无 `max_bytes/keep_files` 等生产轮转键；④ 依赖仅 `[workspace.dependencies]` 引入 `velox-log` path 与 `tracing-appender = "=0.2.4"`（`default-features = false`），`Cargo.lock` 锁定 `time = 0.3.36` |
| TC-20 | 启动日志规范符合性 / AC-14、RQ-2 | T1/e2e | **通过** | ① 默认 `warn` 下 5 个二进制启动 INFO 均静默（符合 RQ-2 定案）；② 显式 `RUST_LOG=info` 下 5 个二进制均输出完整 `日志初始化完成`（`source=env:RUST_LOG`、`value=info`、`dir=<绝对路径>`、`max_file_bytes=209715200`、`keep_files=3`、`event_cap_bytes=65536`、`queue_events=1024`）及各自 `started` 行（含版本/监听地址/关键配置）；③ 初始化失败（TC-10/15）stderr 始终可见（备注：core 三件套原有 `started` 行未打印 `config` 路径，文件来源下由 `source=file:<绝对路径>` 提供） |
| TC-21 | `scripts/run.sh` 适配（无本地副本）/ FR-6、G2-1 | T3/e2e | **通过** | `run.sh start` → `status` → `logs registry -n 5`（读到真实 WARN）→ `restart` ×2 → `stop` 全通；`logs/` 仅含 `velox-config.log`/`velox-mesh.log`/`velox-registry.log`，无 `nohup.out`/`*.stderr.log`；`run.sh:159` 已改用 `tail -F` 跟随轮转；失败分支不 tail `.stderr.log`（见 §5 非缺陷观察②） |
| TC-22 | `acceptance.sh` 链路与第二实例 / FR-5、G2-1 | T3/e2e | **通过** | 预置旧副本 `logs/demo-provider-2.log` 与 `.stderr.log` 后运行 `bash velox-demo/scripts/acceptance.sh`：**PASS**（25.1s）；旧副本被自动清理；第二 provider 经 `VELOX_PROVIDER__LOG__DIR=.../logs/demo-provider-2` 写入 `logs/demo-provider-2/demo-provider.log`（无运行名副本）；`logs/velox-registry.log` 落盘真实 TTL WARN；退出无残留进程 |
| TC-23 | 文档一致性（含 RQ-A-1 注记）/ AC-14 | T2/手工 | **通过** | 逐项核对：五份实名 TOML（`velox-mesh`/`velox-config`/`velox-registry`/`demo-provider`/`demo-consumer`）均含 `[log] dir="logs" level="warn"` 且无生产轮转键；`velox-mono/docs/adr/0002-log-rotation.md`（D1～D8、依赖锁图 `time 0.3.36`、MSRV、截断/收敛/有界 guard 语义）、`development.md`、`subsystems.md`、`scripts/README.md`、`velox-demo/README.md` 及三 core crate README 全部同步一致；RQ-A-1 注记已在 `02-architecture.md:555` 明确落位 |
| TC-24 | IO 故障状态机与关停保证 / G2-5、P3-A1 | T0/集成 | **通过**（附 P3-A1 留痕） | FaultFs 全部 11 项故障/恢复/身份检查/队列满/有界 guard 测试全绿（`fault_remove_oldest_retries_same_step`、`fault_move_one_retries_without_double_shift`、`fault_open_failure_recovers_via_open_fresh`、`fault_write_error_keeps_prefix_and_reopens`、`fault_short_write_continues_within_record`、`fault_write_zero_transitions_reopen`、`fault_interrupted_retry_budget_then_reopen`、`fault_flush_failure_reopens_no_rollback`、`identity_check_reopens_after_unlink`、`reopen_truncates_oversize_path`、`queue_full_drops_counted_no_backpressure`、`guard_drop_returns_within_budget_when_worker_blocked`）。**P3-A1 留痕**：guard 阻塞分支以 2s 测试预算验证有界返回（appender 握手 100ms/1000ms），不代表 worker 卡死或 `kill -9` 下报告必然可达 |
| TC-25 | 无反馈运维报告 / G2-4、P3-A1 | T0/集成 | **通过**（附 P3-A1 留痕） | `no_feedback_after_producers_stop`（M=256B 小于单条报告长度、真实 dispatch+队列，生产者停止后连续观测窗计数与文件长度完全静止）与 `std_report_sink_rate_limits_by_category`（注入时钟验证 60s 限频与首次立即输出）均通过。**P3-A1 留痕**：静止验证依赖 120ms 观测窗，符合尽力语义 |

### 冻结 AC 覆盖汇总

| 冻结 AC | 关联 TC | 本轮实测结论 |
|---|---|---|
| AC-1（裸进程默认落盘） | TC-01 | **通过** |
| AC-2（5 进程独立文件隔离） | TC-02 | **通过** |
| AC-3（硬字节边界/截断/旧文件收敛） | TC-04、TC-03 | **通过** |
| AC-4（含活跃最多 3 份） | TC-05、TC-03 | **通过** |
| AC-5（轮转并发不崩溃） | TC-06 | **有条件通过**（活体夹具自身存在 BUG-1，轮转逻辑正常） |
| AC-6（目录自动创建） | TC-08（反面 TC-10） | **通过** |
| AC-7（目录文件/env 覆盖） | TC-09 | **通过** |
| AC-8（默认 warn + ERROR 可见） | TC-11、TC-12 | **通过** |
| AC-9（优先级链与高合法覆盖低非法） | TC-13、TC-14 | **通过** |
| AC-10（非法值/空串 fail-fast 与语法分流） | TC-15 | **通过** |
| AC-11（旧配置兼容与未知字段） | TC-18 | **通过** |
| AC-12（CI 与常量/默认值） | TC-19 | **通过** |
| AC-13（逻辑字节总和 ≤3M + 无未受管副本） | TC-07、TC-21、TC-22 | **通过** |
| AC-14（文档与启动日志规范） | TC-23、TC-20 | **通过** |

---

## 4. 回归结果

- **静态门槛与锁图检查**（HEAD=`4b24a20` 工作树基线）：
  - `cargo fmt --all --check`：通过
  - `cargo clippy --workspace --all-targets -- -D warnings`：通过（0 warnings）
  - `cargo build --workspace --locked`：通过
  - `Cargo.lock` 锁定核查：`tracing-appender = 0.2.4`、`time = 0.3.36`
- **workspace 单元与集成测试**（`cargo test --workspace --locked --no-fail-fast`，共 130 例）：
  - `velox-log`：**28/28**（另：首跑单独 `cargo test -p velox-log` 时 `live_rotation_concurrent` 偶发失败 1 次，详见 §5 BUG-1；8 线程 ×2 轮与单包连跑 15 轮均 28/28 绿）
  - `velox-config`：**23/23**
  - `velox-registry`：**59/59**（lib 38 + compat 2 + persistence 8 + watch 11）
  - `velox-mesh`：**4/4**
  - `demo-provider`：**5/5**
  - `demo-consumer`：多线程并发下 10/11，仅已知既有 env 竞态 flaky `config::tests::defaults_match_contract` 失败；隔离复测 `cargo test -p demo-consumer -- --test-threads=1` **11/11 全绿**（非本 feature 引入，不计入本次回归失败）。
- **端到端脚本回归**：
  - `bash scripts/run.sh` 启停/日志/双重启周期：全部通过，退出无残留进程。
  - `bash velox-demo/scripts/acceptance.sh`（零修改运行，耗时 25.1s）：**PASS**（步骤 5/5 全部断言通过，退出无残留进程）。
- **新增回归失败清单**：`velox-log::tests::live_rotation_concurrent` 偶发失败（BUG-1，测试资产缺陷）。

---

## 5. 缺陷清单

| 编号 | 严重级别 | 类型 | 描述与根因 | 复现步骤与实测证据 | 修复建议（交 DA） |
|---|---|---|---|---|---|
| **BUG-1** | **中** | 测试资产（`velox-log/src/tests.rs`） | TC-06 活体自举测试 `tests::live_rotation_concurrent` 存在两处并发/清理缺陷：① `fixture_child`（`velox-log/src/tests.rs:1280`）对 `TcpListener` 设置了 `set_nonblocking(true)`；在 macOS/BSD 上 `accept()` 返回的 `TcpStream` **默认继承 `O_NONBLOCK`**，而 `handle_fixture_conn`（`:1315-1324`）未调 `stream.set_nonblocking(false)`，高并发调度下客户端字节尚未到达时 `stream.read(&mut buf)` 立刻返回 `Err(WouldBlock)` 并被 `_ => return` 静默丢弃关闭空连接，导致客户端 `read_to_end` 收到空串 panic；② `live_rotation_concurrent`（`:1390-1580`）未对 `child` 套 RAII Drop Guard，客户端线程 panic 后跳过 `/stop` 与 `child.wait()`，**确定性遗留 `fixture_child` 孤儿进程**（违反 TC-06 第⑤条「无孤儿进程」）。 | **复现命令**：高并发运行 `cargo test -p velox-log tests::live_rotation_concurrent`（例如 6 并发进程跑 18 轮，或直接 `cargo test -p velox-log` 并发调度触发）。<br>**期望**：全部通过且失败/成功路径均零孤儿进程。<br>**实际**：本轮 T0 首跑 `cargo test -p velox-log` 即失败（27/28）；6 并发 18 轮复现 1/18，堆栈：`thread '<unnamed>' panicked at velox-log/src/tests.rs:1545:17: client6 emit 响应异常`、`client0 emit 响应异常`；随后 `pgrep -fl velox_log` 检出 2 个残留孤儿进程（PID `24907`、`27150`，均带 `--exact tests::fixture_child`），由 TA 人工 `kill -9` 清理。 | ① `handle_fixture_conn` 入口显式调用 `let _ = stream.set_nonblocking(false);`；② 为 `live_rotation_concurrent` 的 `Child` 加 RAII Drop Guard（`Drop` 中执行 `try_wait`→`kill`→`wait`），保证任意断言失败均不遗留孤儿进程。 |

> 非缺陷观察项留痕（不构成缺陷，供后续参考）：
> 1. **QA GATE2 P3-A1 留痕**：TC-24（`guard_drop_returns_within_budget_when_worker_blocked`，2s 测试预算）与 TC-25（`no_feedback_after_producers_stop`，120ms 观测窗）均通过，按 P3-A1 如实标注其属尽力语义的时间预算验证，不替代 worker 死锁/`kill -9` 场景的必达保证。
> 2. **TC-21④ 失败提示文案**：`scripts/run.sh:49-53` 健康检查失败分支已正确移除 `.stderr.log` 读取（改为 `tail -n 20 "$VELOX_LOG_DIR/$crate.log"`），stderr 也已通过匿名管道实时回显，但提示文案未显式写出「启动错误已输出到 stderr」。
> 3. **TC-20 启动行 `config` 路径**：`demo-provider`/`demo-consumer` 的 `started` 行输出 `config="..."`，core 三件套原有 `started` 行未输出 `config` 路径（但在文件配置来源下 `velox-log` 的 `source=file:<绝对路径>` 可直接追溯配置文件路径）。
> 4. **跨包既有 flaky**：`demo_consumer::config::tests::defaults_match_contract` 在 workspace 并行测试中复现失败、单线程隔离 11/11 通过，已在 F2 轮次登记，非本 feature 引入。

---

## 6. 未执行/无法验证用例及原因

**无**（TC-01～TC-25 全部 25 条用例均已执行并取得明确实测结果；其中 TC-06 因测试夹具自身 BUG-1 偶发失败并残留孤儿，待 DA 修复后由终验轮复验关闭）。

---

## 7. 终验轮（F-TA-F）

> 覆盖提交：`4b68ebfe11f540c2abd3090d7f180ab5c1340175`（`test(log): 修复活体夹具 O_NONBLOCK 继承空读与子进程孤儿泄漏（BUG-1）`，仅改 `velox-log/src/tests.rs`，65+/18-；当时仓库 HEAD=`4b68ebfe11f540c2abd3090d7f180ab5c1340175`，develop 分支）；**终验结论：通过**

### 7.0 执行环境与核对

- 执行环境：macOS（darwin），cargo/rustc **1.97.1**，分支 develop @ `4b68ebfe11f540c2abd3090d7f180ab5c1340175`（终验时 HEAD，其后无本 feature 新代码提交）；工作树基线与常规轮一致（`M velox-config/src/limit.rs` 既有 rustfmt 改动原样保留、未跟踪规格/报告文件原样保留，**本轮零提交**）。
- 核对结论：常规轮覆盖 `a5b75e5`（当时 HEAD=`4b24a20`），**未包含针对 `4b68ebf` 的 T2+T3 实跑记录** → 本轮 T0 / BUG-1 复验 / T2 / T3 全部无条件实跑；`4b68ebf` 改动面仅测试资产（`velox-log/src/tests.rs`），不触及任何产品代码与二进制行为。
- 终验前预检：`pgrep` 无 `fixture_child` 孤儿、无 velox/demo 业务进程残留。

### 7.1 档位执行（T0/T1/T2/T3）

| 档位 | 执行方式 | 命令与范围 | 结果 | 耗时 |
|---|---|---|---|---|
| **T0 基础** | 实跑 | `cargo build --workspace`；`cargo fmt --all --check`；`cargo clippy --workspace --all-targets -- -D warnings` | **三项全绿**（增量缓存，产物 up-to-date，确认 DA 整备后基线未被破坏） | ~0.3s |
| **BUG-1 复验（本轮重点）** | 实跑 | `cargo test -p velox-log live_rotation_concurrent`：①单跑 ×5 轮；②6 并行 × 3 轮（18 次调用，对齐常规轮 BUG-1 复现压力条件）；每轮后 `pgrep -f fixture_child` 全程监测 | **23/23 全绿、0 panic、全程零孤儿进程残留（含并发压力与失败路径监测）** → **BUG-1 闭环**（见 §7.3） | ~1min |
| **T1 冒烟** | **跳过（触发判定留痕）** | `4b68ebf` 仅改测试资产，不涉及启动装配/配置加载/服务间链路；常规轮已在 `a5b75e5` 完成 5 真实二进制全场景 T1 活体验证，`a5b75e5→4b68ebf` 间无产品代码改动 → 触发条件不成立（终验后补跑见 §7.7） | — | — |
| **T2 全量回归** | 实跑 | `cargo test --workspace --no-fail-fast`（本机 cargo 1.97 默认 fail-fast，按规程显式 `--no-fail-fast` 取全量口径） | **130 例 = 129 通过 + 1 已知 demo flaky**（明细见 §7.2）；失败项隔离复跑全绿 | 4.9s |
| **T3 e2e** | 实跑 | `bash velox-demo/scripts/acceptance.sh`（零修改运行） | **PASS**（5/5 断言，exit 0）；终验后置检查（TC-07 口径）：`logs/` 精确 5 个受管实名文件 + `demo-provider-2/` 分目录、无 `nohup.out`/`*.stderr.log` 未受管副本、`velox-registry.log` 落盘真实业务 WARN 10 条（含 `instance expired, removed service="demo-provider"`）、退出零残留进程、`fixture_child` 零孤儿 | 25.3s |

> T3 补充说明：五二进制真实日志链路在本轮由 acceptance.sh 于终验 HEAD 实际启动并落盘（AC-1/2/8/13 活体证据在最终提交上刷新）；TC-06 活体并发轮转场景由 BUG-1 复验的 23 次实跑覆盖。终验轮 T2/T3 **未降级、未免除**。

### 7.2 T2 全量口径明细（130 例 = 129 通过 + 1 已知 flaky）

| 测试目标 | 通过/总数 | 备注 |
|---|---|---|
| `velox-log`（lib） | **28/28** | 含 TC-06 活体夹具 `live_rotation_concurrent`（T2 内 1.12s 完成） |
| `velox-config` | **23/23** | — |
| `velox-registry`（lib 38 + compat 2 + persistence 8 + watch 11） | **59/59** | 另 2 个 bin 目标 0 例 |
| `velox-mesh` | **4/4** | — |
| `demo-provider` | **5/5** | — |
| `demo-consumer` | **10/11** | 唯一失败 `config::tests::defaults_match_contract`：`assertion left == right failed, left "0.0.0.0:19999" / right "0.0.0.0:9100"`——**范围外既有 env 竞态 flaky**（workspace 并行测试间端口 env 串扰，F2/F3 轮已登记，非 addlog 引入）；**隔离复跑 `cargo test -p demo-consumer -- --test-threads=1` → 11/11 全绿** |

> flaky 处置留痕：按 DA 风险提示如实复现并留痕（复现签名与常规轮/DA 验证完全一致），隔离复跑验证通过，**不计入 addlog 缺陷、不影响终验结论**。

### 7.3 BUG-1 复验结论（闭环）

- **复验设计**：① `cargo test -p velox-log live_rotation_concurrent` 单跑 5 轮；② 对齐常规轮复现条件（6 并行 × 3 轮 = 18 次，常规轮该条件下曾复现 1/18）；③ 每轮及结束后 `pgrep -f fixture_child` 监测；④ 对全部输出 `grep panicked` 扫描。
- **结果**：23 次调用 **23/23 通过、0 panic、0 孤儿**；常规轮曾复现的 `clientX emit 响应异常`（accept 继承 `O_NONBLOCK` 导致 `WouldBlock` 空读）与 `fixture_child` 孤儿泄漏在本轮（含原复现压力条件）**均未再出现**。
- **结论**：DA 修复有效——① `handle_fixture_conn` 入口 `set_nonblocking(false)` 还原阻塞模式使 5s 读超时真正生效；② `ChildGuard` RAII Drop 在所有退出路径（含断言失败/panic）执行 `try_wait→kill→wait` 回收。**BUG-1 关闭（闭环）**；TC-06 由常规轮「有条件通过」升级为「**通过**」，AC-5 终验通过。

### 7.4 AC-1～AC-14 终验结论

| AC | 终验证据（本轮于 `4b68ebf`） | 终验结论 |
|---|---|---|
| AC-1（裸进程默认落盘） | acceptance.sh 5 进程真实落盘 + logs/ 清点 | **通过** |
| AC-2（5 进程独立隔离） | logs/ 精确 5 实名文件 + `demo-provider-2/` 分目录，无串写、无未受管副本 | **通过** |
| AC-3（硬字节边界/截断/收敛） | `velox-log` 轮转/截断/收敛单测随 T2 28/28 全绿 | **通过** |
| AC-4（含活跃 ≤3 份） | `writer_retention_removes_oldest` 等随 T2 全绿 | **通过** |
| AC-5（轮转并发不崩溃） | TC-06 活体夹具 BUG-1 复验 23/23 + 零孤儿（§7.3） | **通过**（升级） |
| AC-6（目录自动创建） | 目录创建用例随 T2 全绿 | **通过** |
| AC-7（目录文件/env 覆盖） | 目录覆盖用例随 T2 全绿；acceptance.sh `VELOX_PROVIDER__LOG__DIR` 覆盖生效（`demo-provider-2/`） | **通过** |
| AC-8（默认 warn + ERROR 可见） | T3 实测：默认 warn 下 INFO 静默，`velox-registry.log` 真实业务 WARN 10 条落盘 | **通过** |
| AC-9（优先级链） | 等级链用例随 T2 全绿（velox-config 23/23、velox-log 28/28 含 chain/edge） | **通过** |
| AC-10（非法值 fail-fast 分流） | strict_syntax/edge_cases 用例随 T2 全绿 | **通过** |
| AC-11（旧配置兼容） | 兼容用例随 T2 全绿 | **通过** |
| AC-12（CI 与常量/默认值） | 本轮 T0 三项 + T2 全量（除范围外 flaky）全绿；常量/默认值单测全绿 | **通过** |
| AC-13（逻辑字节上界 + 无未受管副本） | T3 目录清点无 `nohup.out`/`*.stderr.log`；受管文件口径断言随 T2 全绿 | **通过** |
| AC-14（文档与启动日志规范） | 文档自常规轮 TC-23 静态核对后未改动（终验 HEAD 无文档变更），结论沿用；启动日志规范经 acceptance.sh 5 进程实跑复核 | **通过** |

**AC-1～AC-14 终验全部通过，无缺口。**

### 7.5 遗留与观察项

1. **范围外既有 flaky（留痕，不阻塞）**：`demo_consumer::config::tests::defaults_match_contract` 在 workspace 并行下存在 env 竞态（期望端口 9100、实测 19999），F2/F3 轮已登记；建议后续在 demo 范围做测试 env 隔离修复（如显式清除端口 env 或单线程标记），不属 addlog 交付内容。
2. **常规轮非缺陷观察沿用**：TC-21④ 失败提示文案未显式注明「启动错误已输出到 stderr」、TC-20 core 三件套 `started` 行无 `config` 路径、P3-A1 时间预算尽力语义留痕——终验 HEAD 对上述均无代码改动，维持原状。
3. **无新增缺陷**；TC-01～TC-25 无未执行/无法验证用例；BUG 清单终态 = BUG-1（已闭环）。

### 7.6 04 报告归档策略评估（仅建议，本轮未执行任何 git 提交）

- **现状**：`docs/specs/addlog/04-test-report.md`（常规轮 + 本终验轮）与同批 `05-acceptance.md` 均为未跟踪文件；前两个 feature（instance-concurrency-limit、registry-persistence-watch）的 04/05 文档同样未跟踪——仓库先例是收口期一次性 `docs(specs)` 提交（如 `28c9114`）。
- **建议**：GATE3 通过后，将 `04-test-report.md` 与 `05-acceptance.md` 合并为**单个 `docs(specs): addlog 测试报告与验收文档` 提交**收口；不建议本轮先行单独 test/docs 提交——常规轮与终验轮记录同属一个评审单元，GATE3（QA）若要求补正可一次性落盘，避免碎片化提交历史。
- **本轮工作树纪律执行情况**：仅向本文件追加终验轮章节（保持未跟踪）；`velox-config/src/limit.rs` 既有 rustfmt 改动未触碰；未提交运行数据与 `docs/specs/_pipeline-state.md`；临时产物仅 `/tmp` 下测试日志（已随会话清理）。

### 7.7 补充验证：T1 真实链路 `scripts/run.sh` 生命周期（终验后补跑，GATE3 前补实）

- **补跑动因**：§7.1 中 T1 按触发判定跳过（`4b68ebf` 仅测试资产）；为在终验 HEAD 上刷新 TC-21 脚本链路活体证据，按任务文本「T1 真实链路场景本轮按需补跑」口子补跑 `run.sh` 完整生命周期。
- **最终采信结果（干净复跑，PASS，2.2s）**：`start`（3 服务 healthz 就绪，pid 55857/55873/55887）→ `status`（3 服务运行中）→ `logs registry -n 3`（读到真实 TTL WARN）→ `restart`（stop+start 双周期，新 pid 55942/55950/55958 全部就绪）→ `stop`（全部停止）；后置检查：零残留进程、`.run/` pidfile 全清、`logs/` 无 `nohup.out`/`*.stderr.log` 未受管副本。**TC-21 证据在 `4b68ebf` 上刷新通过**。

**前两次尝试作废留痕（证据不采信，均非产品/脚本缺陷）**：

1. **尝试①（受并发作业互扰，作废）**：补跑期间编排者 tmux 会话 `velox-pipeline` 于 16:30 派发的并行 CLI 作业恰在运行其自身 `velox-demo/scripts/acceptance.sh`（其核心服务启动于 16:32:04，registry 恢复 WARN `age_secs=509` 与本报告 16:23 验收数据吻合）。表现为本方 `start` 报「已在运行」（pid 51696/51712/51724，非本方启动）、`restart` 后 `stop` 报「未在运行」（新起服务被并发作业的下一阶段接管/回收）。本方**未 kill 任何并发作业进程**，等待其自然退出并自清（.run/ 清空、零残留）后才复跑；终验本体证据（T0/BUG-1 复验/T2/T3，均完成于 16:23 前）时间上先于并发作业，**不受影响**。
2. **尝试②（测量管道构造伪挂起，作废）**：以 `bash scripts/run.sh start 2>&1 | tail -4` 方式调用导致外层命令 300s 超时。根因：`run.sh` 按设计将服务 stderr 经匿名管道直达调用端（`nohup ... 2> >(exec cat >&2) &`），常驻服务存活期间 `cat` 持住调用端管道写端，`tail` 等不到 EOF——属调用方管道用法问题，**run.sh 自身健康等待有界**（`wait_healthz`：50×(curl -m 1 + 0.2s) ≈ ≤60s/服务，`scripts/common.sh:76`），registry 于 16:35:04 正常启动落盘可证。超时 killpg 附带终止了本方启动的服务（同进程组），遗留 3 个 stale pidfile，已由 `run.sh stop` 正确清空（pidfile 卫生验证通过）。
3. **使用注记（非缺陷观察项）**：`run.sh start/restart` 的 stdout/stderr 在**非 tty 管道场景**（如 `| tail`）下会因服务继承 stderr 管道而保持打开直至服务退出，属「stderr 直达调用端」设计的自然结果；自动化场景建议重定向到文件而非管道接 `tail`。终端交互用法不受影响（TC-21 常规轮与本次干净复跑均正常返回）。
