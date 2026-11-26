# 测试报告：velox-registry v0.2 状态持久化 + watch 推送（registry 侧）——常规执行轮（F-TA-E）

> Feature：`registry-persistence-watch`；工序：F-TA-E（MODE=EXECUTE，常规轮，TA-F 终验轮之前）。
> **覆盖提交：`af28e65`（feature 最终代码提交；当时仓库 HEAD=af28e65，其后本 feature 无新代码提交）**。
> 测试基线说明：工作树含在途工作改动——F1/F3 规格文档（instance-concurrency-limit 等 spec 文件）与 `velox-config/src/{config.rs,limit.rs}` 纯 rustfmt 格式化改动，全部保留未提交；**测试基线含上述在途工作树改动（无行为变化）**（fmt/clippy/测试对当前树全绿可佐证格式化无行为影响）。运行数据/日志均在系统临时目录，不提交。
> 执行环境：macOS（arm64），bash 5.3.20（满足 scripts/common.sh bash≥4），cargo/curl/jq 就绪；测试端口 18300-18302/18201 与 demo 固定端口均空闲起步。

---

## 1. 测试结论

**有条件通过。**

- 功能断言全部绿：`cargo test -p velox-registry` **59/59**（多轮复跑，含串行复核）；降级活体 e2e（SIGKILL 恢复 / tmp 写窗口崩溃注入 / SIGINT 优雅退出 / 同 cwd 隔离）**20 断言 0 失败**，其中 TC-18b 崩溃注入**首轮即命中 tmp 写窗口**且正式快照无半写生效；`bash velox-demo/scripts/acceptance.sh` **PASS**（零修改）；SIGINT 退出码 **0** 实证。
- 未闭合项（不阻断功能结论，阻断「无条件通过」）：
  1. **BUG-1**（测试资产，高）：`velox-registry/tests/registry-e2e.sh` 未注入监听地址 env，脚本首轮执行即失败——脚本级 e2e 阻断，需 DA 修复后由 TA-F 终验复跑（本轮已用降级驱动等价覆盖，见 §2/T3）；
  2. **BUG-2**（测试资产，中）：velox-registry config 单测存在 env 并发竞态，`cargo test --workspace` 并行下偶发失败（本轮实测复现）——全量回归可信度受损，需 DA 修复；
  3. **TC-38 性能专项（AC-13/RQ-2）**未执行：完整验收级执行按任务指派归 TA-F 终验轮（留痕见 §6）。

## 2. 档位执行说明（T0~T3 触发判定与耗时）

| 档位 | 判定 | 执行情况 | 耗时 |
|---|---|---|---|
| **T0 基础（必跑）** | 总是执行 | **执行**：① `cargo test -p velox-registry` 59/59 绿（lib 38 + compat 2 + persistence 8 + watch 11；另跑 `--lib` ×5 轮复跑全绿）；② 既有 demo 回归 `bash velox-demo/scripts/acceptance.sh`（只运行零修改）PASS（健康检查×6→注册/发现→直连 builtin→配置热更→mesh 转发→扩容 LB→SIGKILL 剔除→SIGTERM 注销，全断言通过）。覆盖 TC 中可自动化=单测/集成与既有链路回归的全部条目 | ①约 3 分钟（含首次编译；纯执行 <6s/轮）；②23s |
| **T1 冒烟** | **触发成立**：本 feature 改配置加载（`[persistence]` 校验、env 空值拒绝）、启动装配（快照恢复/flusher）、watch 服务链路，属启动/配置类变更 | **执行（降级方式）**：构建 + 真进程启动（隔离端口/env 配置加载路径实测）+ healthz + 恢复/watch 冒烟——由降级 e2e A/C/D 场景与 acceptance 全链路共同承担（含默认配置启动、env 注入启动、恢复后接客） | 含于降级 e2e 约 3.5 分钟内 |
| **T2 全量回归** | **严格触发条件不成立**（变更仅 registry，无跨模块公共接口/配置传导）。**选择执行**，理由：(a) 验证 DA 汇报的 demo-consumer 已知 flaky 隔离判定；(b) 确认含在途 rustfmt 改动的工作树基线健康；(c) 执行成本低。终验轮无条件全量回归仍兜底 | **执行**：`cargo fmt --all --check` 绿；`cargo clippy --workspace --all-targets -- -D warnings` 绿；`cargo build --workspace` 绿；`cargo test --workspace`（含 `--no-fail-fast` 复核）暴露 **BUG-2**（registry config flaky 1~2 例，见 §5）+ 已知 demo-consumer flaky 复现（隔离单测通过，按已知 flaky 记录） | fmt/clippy/build <1s（缓存指纹有效）；workspace 测试 <4s/轮 |
| **T3 e2e** | **触发成立**：P0 用例（SIGKILL 宕机恢复 AC-1/2、写盘中途崩溃 AC-14）只能活体验证 | **降级执行**：① 资产脚本 `bash velox-registry/tests/registry-e2e.sh` 首轮执行即失败（**BUG-1**，场景 A「registry 未就绪」）；本任务限定只产出报告、不改脚本，故改用 /tmp 手工降级驱动等价覆盖脚本 A/B/C/D 四场景（TC-01/02/18b/19/30/37b）：**20 断言全 PASS**，TC-18b 崩溃注入首轮命中，证据目录保留（`/var/folders/.../velox-tae-e2e.9aUYvDkT`）。② **TC-38 性能专项未执行**：触发条件成立（AC-13 验收项），但完整验收级执行（基线 commit 28c9114 隔离检出/独立 target、预热≥30s、采样≥60s、每端点≥1000 样本、nearest-rank P99）按任务指派**归 TA-F 终验轮**，此处留痕「归终验轮兜底」，RQ-2 证据待交付 | ①资产脚本 15s（失败即止）+ 降级驱动约 3.5 分钟 + exit-code 补证 5s；②未执行 |

单档位均未超预估 2 倍时间盒；无死磕项。

## 3. 用例执行结果（对照 03-test-cases.md 41TC 矩阵）

> 证据来源缩写：**S** = `cargo test -p velox-registry`（59 例，测试名与 architecture §6.1/6.2 覆盖映射核对）；**D** = 降级活体驱动（/tmp/ta-tae-e2e-degraded.sh，20 断言）；**A** = acceptance.sh；**M** = 补充手工/补证。状态：✔ 通过 / ◐ 部分覆盖（余项归终验） / ⬚ 未执行。

| TC | 功能点 | 证据 | 结果 | 备注 |
|---|---|---|---|---|
| TC-01 | SIGKILL 恢复（AC-1） | D·A场景 | ✔ | kill -9 → 重启恢复 id/address/metadata 一致、恢复日志 `restored_services=1/instances=1`、恢复窗口 age=0s≤TTL60；脚本级复跑归终验（BUG-1） |
| TC-02 | 恢复后同 id 心跳（AC-2） | D·A场景 | ✔ | 同 id heartbeat 200 `{"ok":true}`，落盘 age=0s 窗口内 |
| TC-03 | 集成恢复/固定 epoch 年龄 | S·persistence `restart_recovers_instances_with_same_id` | ✔ | |
| TC-04 | 响应完成即写穿 | S·persistence（写穿断言）+ D·A场景（POST 响应后快照立即可读） | ✔ | |
| TC-05 | 周期合并/dirty 状态机 | S·lib `dirty_state_machine`、`heartbeat_flush_is_periodic_and_not_blocking` | ✔ | 相位/合并/失败重置语义 |
| TC-06 | 周期刷盘活体 3 周期 | S·persistence `heartbeat_flush_is_periodic_and_not_blocking` | ◐ | 集成层通过；活体 mtime 3 周期观察归终验脚本级 |
| TC-07/08/09 | 恢复 TTL 过滤/边界/陈旧反例 | S·persistence `restore_drops_instances_beyond_ttl`、`restore_dropped_lazy_cleanup`、lib 固定 epoch 用例 | ✔ | 严格 age>TTL 丢弃、延迟清理 |
| TC-10 | enabled=false 不落盘 | S·persistence `disabled_persists_nothing` | ✔ | |
| TC-11 | 默认配置启动 | S·lib `persistence_defaults` + D（默认/env 两种启动均 healthz 后接客） | ✔ | true/./data/5 默认实证 |
| TC-12 | 配置校验矩阵 | S·lib `partial_section_takes_container_defaults`、`empty_data_dir_rejected_even_when_disabled`、`cross_field_flush_ge_ttl_only_when_enabled`、`toml_type_errors_rejected`、`env_persistence_overrides`、`env_empty_value_is_rejected_not_ignored`、timeout/参数校验用例 | ✔ | 断言全绿；但存在 **BUG-2 并发竞态**（偶发，见 §5），修复前并行全量回归对该组结论有噪声 |
| TC-13 | 损坏快照整文件跳过 | S·persistence `corrupted_snapshot_skipped_and_serves`、lib `corrupted_variants_are_skipped` | ✔ | |
| TC-14 | 目录递归创建/隔离布局 | S·persistence `recursive_dir_creation_isolated_layout` | ✔ | `<data_dir>/registry/registry-state.json` |
| TC-15 | 目录不可创建启动失败 | S·lib config 用例（路径/权限分支） | ✔ | 初始化失败路径 |
| TC-16 | 写失败 500 persist_failed、内存优先 | S·persistence `write_failure_memory_first_then_retry_succeeds` | ✔ | 注入后恢复自动重试（G2-1） |
| TC-17 | 后台失败无 mutation 重试 | S·lib `dirty_state_machine` + persistence 重试用例 | ✔ | |
| TC-18 | 原子写（18a 清 tmp/审查；18b 真实写中 kill） | D·B场景 + S（store 原子写单测） | ✔ | **18b 首轮命中 tmp 写窗口**：kill -9 后正式快照仍为合法完整 JSON（旧完整或新完整）；脚本级复跑归终验（BUG-1） |
| TC-19 | SIGINT 最终刷盘/退出码 | D·C场景 + M（exit-code 补证） | ✔ | SIGINT 后 <5s（实测 0s）退出、**退出码 0**、退出前最终落盘含实例 |
| TC-20/21 | watch 立即/挂起唤醒 ≤1s | S·watch `default_index_immediate_three_scenarios`、`explicit_equal_suspends_then_times_out_with_current_snapshot`、`stale_and_oversized_index_return_immediately` | ✔ | |
| TC-22/23 | 参数校验 400/默认 30 | S·watch `param_errors_all_400_invalid_parameter`、lib `timeout_range_validation`、`default_index_branch_is_immediate` | ✔ | 缺省 index 三场景/显式 0 分离（G2-2/RQ-3 目标实现核实） |
| TC-24/25 | timeout 挂起/index 纠正 | S·watch `explicit_zero_immediate_when_version_positive`、`explicit_zero_suspends_at_v0_and_wakes_on_register` | ✔ | 重启版本不延续由恢复路径单测覆盖 |
| TC-26/27/28/29 | sweep 批次/注销/查询不 bump/复活 | S·watch `sweep_batch_bumps_once_and_wakes`、`deregister_wakes_queries_do_not`、`revive_heartbeat_watches_exactly_once` | ✔ | |
| TC-30 | shutdown 唤醒挂起 watch | S·watch `shutdown_wakes_suspended_watchers` + D·C场景（真进程 SIGINT，挂起 watch 返回/断开） | ✔ | 注入集成 + 真进程双证据 |
| TC-31/32/33/34 | 断连重连/50 watch/并发一致性/flush 锁 | S·lib `change_channel_notifies` + store 并发/锁用例 + watch 响应形状用例 | ✔ | |
| TC-35 | v0.1 HTTP 契约回归 | S·compat `v01_endpoint_contracts_unchanged` | ✔ | |
| TC-36 | 既有 demo 全链路（零修改） | A·acceptance.sh | ✔ | PASS，mesh 周期拉取/转发/LB/TTL 正常 |
| TC-37 | 默认值 + registry/config 同 cwd 隔离 | S·lib `persistence_defaults` + D·D场景 | ✔ | config 不装载子目录快照、顶层 registry-state.json 对照互不覆盖、双方重启数据完整 |
| TC-38 | 性能专项（AC-13） | — | ⬚ | **未执行，归 TA-F 终验轮**（RQ-2 证据待交付；协议见 03 §TC-38） |
| TC-39 | 启动/运维日志可观测 | D（各场景日志采集）+ S | ✔ | `persistence initialized enabled/data_dir/flush_interval_secs/restored_services/instances/dropped_expired/skipped` 字段实证；DEBUG 级细节归终验 |
| TC-40 | watch 新端点参数合同 | S·watch 全部 11 例（缺省三场景/显式 0 分离/等值挂起/错误 400 映射） | ✔ | |
| TC-41 | metadata 特殊字符 roundtrip | S·lib/registry 用例（注册往返断言） | ✔ | |

**汇总**：✔ 38 项（其中 4 项为「集成已证 + 脚本级复跑归终验」的 ◐→✔ 记录见备注）；◐ 1 项（TC-06 活体观察面）；⬚ 1 项（TC-38）。无失败用例。

## 4. 回归结果

- **范围**：workspace 全量（T2 选择执行，理由见 §2）+ registry 受影响套件多轮复跑。
- **通过率**：workspace 单轮（--no-fail-fast）合计 **93 passed / 2 failed**（demo_consumer 10+1flaky、demo_provider 5✔、velox_config 16✔、velox_mesh 4✔、velox_registry lib 37+1flaky、registry compat 2✔、persistence 8✔、watch 11✔）；registry 独立套件 **59/59**（5 轮 `--lib` 复跑 + 2 轮全量均绿）。fmt/clippy/build 三门槛全绿。
- **失败清单（全部为 flaky 类，非产品断言失败）**：
  1. `velox_registry::config::tests::*`（BUG-2，**本轮新发现**）——workspace 并行下偶发；`--test-threads=8` 定向复现 2 例（`persistence_defaults`、`cross_field_flush_ge_ttl_only_when_enabled`），panic 信息显示跨测试 env 残留值（如读到并发测试设置的 `'-1'`）；串行/单独运行必绿。
  2. `demo_consumer::config::tests::defaults_match_contract`（**已知 flaky**，DA 报告在案）——本轮 workspace 两次运行均复现，隔离单测通过；按已知 flaky 记录并隔离判定，**不计为本次回归失败**、非本 feature 引入。
- 收窄说明：无收窄；T2 为选择执行（严格触发不成立），终验轮无条件全量兜底。

## 5. 缺陷清单

### BUG-1（高 · 测试资产）：registry-e2e.sh 未注入监听地址，脚本级 e2e 全场景阻断

- **现象**：`bash velox-registry/tests/registry-e2e.sh` 场景 A 即失败：`[e2e] ✘ A：registry 未就绪`（15s 后 die）。
- **根因**：`start_registry()`（`velox-registry/tests/registry-e2e.sh:44-61`）与 `start_config()`（:63-67）只注入 TTL/sweep/persistence/data_dir 等 env，**未设置 `VELOX_REGISTRY__SERVER__ADDR` / `VELOX_CONFIG__SERVER__ADDR`**（产品支持该覆盖，`velox-registry/src/config.rs:86` 实证），进程按默认绑定 `0.0.0.0:8300`/`0.0.0.0:8200`（诊断日志 `addr=0.0.0.0:8300` 实录），而脚本轮询 `REG_URL=127.0.0.1:18300`/`CFG_URL=…:18200`，永不就绪。场景 D 的 config 侧同理必失败。
- **复现命令**：`bash velox-registry/tests/registry-e2e.sh`（稳定复现，本轮实跑）。
- **期望**：脚本进程监听 18300/18200，四场景（A/B/C/D）可执行。
- **实际**：进程监听默认端口，脚本场景 A 即止；产品行为本身正常（默认端口 healthz 200 实证）。
- **影响**：TC-01/02/18b/19/30/37b 的**脚本级**证据缺失（本轮以降级驱动等价覆盖，见 §2/T3）；修复后须 TA-F 终验复跑并作为 AC-1/14 脚本级证据。
- **修复建议（DA）**：`start_registry` 增加 `VELOX_REGISTRY__SERVER__ADDR="127.0.0.1:$REG_PORT"`，`start_config` 增加 `VELOX_CONFIG__SERVER__ADDR="127.0.0.1:$CFG_PORT"`。

### BUG-2（中 · 测试资产）：registry config 单测 env 并发竞态，并行全量回归偶发失败

- **现象**：`cargo test --workspace` 下 `velox_registry::config::tests` 偶发 1~2 例失败；单独 `-p` 串行运行 59/59 稳定绿（本轮 5 轮 `--lib` + 2 轮全量套件复跑均绿）。
- **根因**：`velox-registry/src/config.rs` tests 已有 `ENV_LOCK`/`with_env_cleaned`（:177-193），但部分用例在**未持锁**状态下调用 `Config::load` 读进程 env：`persistence_defaults`（:203-211，无锁 `load(None)`）、`cross_field_flush_ge_ttl_only_when_enabled` 第二分支（:288，无锁 `load`）、`empty_data_dir_rejected_even_when_disabled` 第一分支（:247 附近，无锁 `load`）。并发测试持锁 `set_var(ENV_FLUSH, "-1")` 等期间，无锁用例读到残留值 → 断言失败。
- **证据（本轮实跑）**：`cargo test -p velox-registry --lib -- --test-threads=8` 复现 2 例失败，panic 信息 `非法 VELOX_REGISTRY__PERSISTENCE__FLUSH_INTERVAL_SECS：'-1'`（persistence_defaults）与 `assertion failed: Config::load(...).is_ok()`（config.rs:288，cross_field）。
- **复现命令**：`cargo test -p velox-registry --lib -- --test-threads=8`（偶发，多跑几轮）；稳定对照：`cargo test -p velox-registry`（串行必绿）。
- **期望**：任意线程数/并行负载下 registry 套件全绿。
- **实际**：并行下 env 跨测试污染，偶发 1~2 例失败。
- **影响**：workspace 全量回归结果不可靠（CI 噪声），可能掩盖真实回归；不涉及产品行为缺陷。
- **修复建议（DA）**：对 env 敏感的 `Config::load` 断言统一包入 `with_env_cleaned`（与 velox-config/src/config.rs 同模式），或测试内全部读写 env 均持 ENV_LOCK。

### 已知 flaky（非新增缺陷，记录备查）

- `demo_consumer::config::tests::defaults_match_contract`：本轮 workspace 两次运行复现失败、隔离单测通过，与 DA 交付说明（纯 HEAD 基线 6/8 复现、单测隔离通过）一致；同 BUG-2 类（进程 env 竞态测试资产问题），非本 feature 引入，不计为本次回归失败；归 DA 一并修复。

## 6. 未执行/无法验证用例及原因

| 项 | 原因与处置 |
|---|---|
| **TC-38 性能专项（AC-13/RQ-2）** | 完整验收级执行（真 v0.1 基线 28c9114 隔离检出、独立 target、预热≥30s、采样≥60s、每端点≥1000 有效样本、nearest-rank P99）按任务指派**归 TA-F 终验轮执行**。本轮不抽样（抽样结果不构成 AC-13 判定，反而消耗时间盒）。**留痕：归终验轮兜底，RQ-2 证据待交付，AC-13 不因本轮未测而判通过或失败** |
| TC-01/02/18b/19/30/37b **脚本级**复跑 | 资产脚本因 BUG-1 阻断（本轮首跑失败实录）；降级驱动已等价覆盖全部断言（20/20 绿），脚本修复后由 TA-F 复跑取得脚本级证据 |
| TC-06 活体 3 周期 mtime 观察 | 集成层已覆盖周期推进/合并/不阻塞语义；活体观察面归终验脚本级（非阻断） |
| TC-39 DEBUG 级日志细节（watch 唤醒/超时） | 本轮降级驱动采集的启动/恢复/关停日志字段已实证（INFO 级）；DEBUG 细项归终验（非阻断） |

---

**证据目录**：降级 e2e 运行目录 `/var/folders/ny/sl3tct6s4p9789fk4xsytd_r0000gn/T/velox-tae-e2e.9aUYvDkT`（含各场景 registry 日志、快照文件、watch 响应；系统临时目录，不提交）。driver 脚本 `/tmp/ta-tae-e2e-degraded.sh`（不属仓库工件）。本轮仅创建本报告，未改任何代码/脚本/其他文档。

**结论重申**：**有条件通过**——功能与兼容断言全绿（59/59 × 多轮、降级活体 20 断言、acceptance PASS、SIGINT exit0）；待办三件：DA 修 BUG-1/BUG-2（测试资产），TA-F 终验补脚本级 e2e 与 TC-38 性能专项。

---

## 7. 终验轮（F-TA-F）

> **覆盖提交：`d78061d`**（feature 最终代码提交；当时仓库 HEAD=`d78061d`，其后本 feature 无新代码提交。`d78061d` 仅改 `velox-registry/src/config.rs` 的 `mod tests` 与 `tests/registry-e2e.sh`——纯测试资产修复，产品业务逻辑零修改，commit message 自证 + diff 范围核对一致）。
> **终验结论：有条件通过。** 功能与兼容断言全绿（AC-1~12、AC-14 及 41TC 全部可执行项；T2 四轮 + T3 脚本级四场景 + acceptance PASS）；**AC-13（TC-38）按协议记「无法验证」**（perf 负载驱动缺陷 BUG-4 + register/deregister 样本不足，详见 §7.4），附 register 写穿延迟风险信号交 GATE3 裁决；新增测试资产缺陷 BUG-3/BUG-4 交 DA 修复。若按二元口径，「不通过」的构成仅为 AC-13 证据缺失与测试资产缺陷，非产品功能断言失败。

### 7.1 常规轮复用核对（先核对再执行）

- 常规轮（§1~§6）覆盖提交为 `af28e65` < `d78061d`，且常规轮 TC-38 未执行 → **不满足复用条件，本轮针对 `d78061d` 全量重执行**（T0/T1 随 T2、T3 一并重新取证；常规轮的降级活体证据仅作 af28e65 历史记录，不作为本轮依据）。
- 核对 `git log`：`d78061d`（2026-11-26 12:20:40）之后无本 feature 新代码提交；工作树在途改动仅为规格文档与 velox-config rustfmt（**测试基线含上述在途改动（无行为变化）**，与常规轮一致，本轮 fmt/clippy/全部测试对其全绿）。

### 7.2 档位执行（T0/T2 全量回归——无条件执行）

| 项 | 命令 | 结果 | 耗时 |
|---|---|---|---|
| 静态门槛 | `cargo fmt --all --check`；`cargo clippy --workspace --all-targets -- -D warnings`；`cargo build --workspace` | 三项全绿 | <1s（缓存指纹对当前树有效） |
| T2·默认线程 R1 | `cargo test --workspace` | **95/95 全绿**（demo-consumer 11、demo-provider 5、velox-config 16、velox-mesh 4、registry lib 38 + compat 2 + persistence 8 + watch 11）——已知 flaky 本轮未复现 | <1s |
| T2·默认线程 R2 | `cargo test --workspace` | demo-consumer `defaults_match_contract` FAILED（**已知 flaky 复现**，fail-fast 截断于该目标）；其余全绿 | <1s |
| T2·8 线程 R3' | `cargo test --workspace --no-fail-fast -- --test-threads=8` | 仅 demo-consumer 已知 flaky FAILED；**registry 38+2+8+11 全绿**；其余目标全绿 | ~4s |
| T2·8 线程 R4 | `cargo test --workspace -- --test-threads=8` | 同 R3'（fail-fast 截断于同一已知 flaky） | ~1s |
| BUG-2 修复专项 | `cargo test -p velox-registry -- --test-threads=8` ×2 | **59/59 ×2 全绿**（加上 R1/R3' 共 4 轮并行全绿）→ **BUG-2 修复验证通过：config 测试 env 竞态消除，8 线程稳定** | ~4s/轮 |
| 已知 flaky 隔离判定 | `cargo test -p demo-consumer config::tests::defaults_match_contract` | **ok**（隔离必绿） | <1s |
| 既有 demo 回归 | `bash velox-demo/scripts/acceptance.sh`（只运行零修改） | **PASS**（步骤 5/5 全部断言通过） | 23.3s |
| T1 冒烟 | 并入 T3 脚本级 e2e（真进程启动/env 配置加载/healthz/恢复/watch 冒烟）+ acceptance 全链路 | ✔ | 含于 T3 |

已知 flaky 本轮表现留痕：`demo_consumer::config::tests::defaults_match_contract` 在本轮 5 次 workspace 运行中 **3 次复现失败**（R2/R3'/R4，默认与 8 线程均可）、隔离单测必绿——与 DA/TA-E 记录一致（纯 HEAD 基线可复现、非本 feature 引入、与 BUG-2 同类的进程 env 竞态测试资产问题），**不计入本 feature 回归失败**，交 DA 与 BUG-2 同批修复。

### 7.3 档位执行（T3 脚本级 e2e——无条件执行，BUG-1 修复验证）

| 轮次 | 结果 |
|---|---|
| 首轮 | **场景 A 即止**：`✘ A：注册失败`（POST 500）。诊断：**外部孤儿进程占用隔离端口**——`velox-registry`(pid 70562)+`velox-config`(pid 70568) 于 **12:19:28**（DA 整备验证时段）泄漏并监听 18300 等，其 `DATA_DIR=./data` 指向已删除的临时 cwd → 脚本自身 registry 绑定失败退出、`wait_healthz` 误命中孤儿 → 注册 500（persist_failed）。**该 500 为外来进程行为，非 d78061d 产品缺陷**（已实证：清理孤儿后见下轮全 PASS）。此事件暴露 **BUG-3**（见 §7.6） |
| 复跑（清理孤儿后） | **PASS——四场景全部通过（13.3s）**：A SIGKILL 重启恢复 + 同 id 心跳 200（落盘 age=0s ≤ TTL60）；**B 写窗口第 1 轮命中**（tmp 观察 kill -9），正式文件仅旧/新完整，重启恢复正常；C SIGINT 5s 内退出、挂起 watch 唤醒、最终落盘心跳推进（before=1795667099 after=1795667101）；D 同 cwd 隔离成立（config 未装载子目录快照、双方重启数据完整） |
| **BUG-1 修复验证** | **✔ 通过**：全部 6 处启动点（start_registry/start_config + 场景 D 4 处内联）注入 `VELOX_REGISTRY__SERVER__ADDR`/`VELOX_CONFIG__SERVER__ADDR`，进程实际监听 18300/18200，脚本首轮（清理环境后）四场景全 PASS——脚本级 e2e 证据已取得（常规轮降级驱动的替代义务解除） |
| 泄漏复现（BUG-3 实证） | 本轮 PASS 后 D 场景 registry(75829)+config(75836) **残留存活**（`wait_exit 5 \|\| true` 超时无强杀）；二次 SIGINT 后退出；已人工清理（kill -9），环境最终清空 |

T3 脚本级证据对照 03-test-cases.md：TC-01/02（A）、TC-18b（B，第 1 轮命中）、TC-19/30（C）、TC-37b（D）——脚本级全 ✔；TC-06 活体观察面仍归后续（非阻断，集成层已覆盖）。

### 7.4 TC-38 性能专项（RQ-2/AC-13）——无条件执行，结果「无法验证」

**协议冻结参数**：macOS 27.2.1（arm64）；rustc/cargo 1.97.1；v0.2=`d78061d`（当前树 release 构建）；v0.1 基线=`28c9114`（脚本隔离检出 + 独立 `CARGO_TARGET_DIR`，基线自证通过：无 store.rs、无 `[persistence]`）；两版同 release profile、同日志配置（缺省）、同负载计划与客户端（curl，并发 32）；端口 18401/18402、数据/日志/原始样本均系统临时目录。执行耗时 3m48s（预热 30s + 采样 60s × 两版）。
**原始记录**：`/var/folders/ny/sl3tct6s4p9789fk4xsytd_r0000gn/T/velox-registry-perf.TzXuaJjn`（raw/v01、raw/v02 逐请求样本 16331+16107 条 + summary.tsv + 服务日志；不提交）。

**分端点结果与比值（v0.2/v0.1 P99，nearest-rank）**：

| 端点 | v0.1 有效样本/错误 | v0.2 有效样本/错误 | v0.1 P50/P99(ms) | v0.2 P50/P99(ms) | 比值 | 协议判定 |
|---|---|---|---|---|---|---|
| heartbeat | 9674 / **4410**(404) | 9541 / **4341**(404) | 0/2 | 0/3 | 1.50 | **无效**：存在请求错误（协议禁止判通过） |
| services | 2431 / 0 | 2398 / 0 | 0/2 | 0/2 | **1.00** | 满足（唯一完全干净端点） |
| instances_healthy | 2416 / **49**(404) | 2378 / **54**(404) | 0/2 | 0/2 | 1.00 | **无效**：存在请求错误（churn 窗口竞态，两版对称） |
| register | 905(<1000) / 0 | 895(<1000) / 0 | 0/1 | 5/11 | 11.00 | **无效**：样本不足 1000 |
| deregister | 905(<1000) / 0 | 895(<1000) / 0 | 0/1 | 5/12 | 12.00 | **无效**：样本不足 1000 |

**AC-13 结论：无法验证（本轮）。** 三重阻断，均按协议不得判通过、亦不得据此宣布违约成立：
1. **BUG-4（perf 负载驱动结构性缺陷）**：32 worker 锁步轮转（各自 `hb_idx` +1 递增）导致**不同槽位覆盖速度仅 ~5 实例/s**（设计意图 200/s 均摊 = 每实例 5s 续约），而池填充速率 ~50 实例/s——排位 ~430 之后的实例在 TTL60s 内永远等不到续约、被 sweep 剪除 → 心跳 404。证据：v0.1/v0.2 错误对称（4410/4341）、**首个 404 出现在心跳序号 4794（≈采样 30s）**、错误延迟 0-4ms（快速 404 非超时）、与堆算吻合。违反协议前置「稳定心跳池不因 churn 产生预期 404」；延长采样无法修复（结构性），故未复跑。
2. register/deregister 有效样本 905/895 < 1000（协议最低样本量不满足）。
3. instances_healthy/heartbeat 存在请求错误（协议：任何端点错误 → 记录缺陷、本轮不得判 AC-13 通过）。

**风险信号（非判定，交 GATE3 结合 AC-4 语义评估）**：v0.2 register/deregister **P50=5ms、P90=7ms、P99=11/12ms** vs v0.1 0/0/1ms——与 FR-4/AC-4「结构变更写穿落盘（响应完成即文件更新）」的同步 fsync 成本一致；比值 11×/12× 显著超 AC-13 的 ≤2× 阈值。样本不足不能定论，但属于**真实的架构性延迟特征**：若 BUG-4 修复后复测仍如此，AC-13 在 register/deregister 端点可能需要 DA 做写穿语义与延迟的权衡设计（如 fsync 策略），或由 GATE3/需求侧裁决 AC-13 端点口径。本轮不降低阈值、不宣布通过、也不以不足样本宣布违约。

### 7.5 BUG-1/2 修复验证结论（带入终验）

| 缺陷 | 修复验证 | 证据 |
|---|---|---|
| BUG-1（e2e 端口 env 未注入） | **✔ 已修复且验证** | 6 处 SERVER__ADDR 注入实查；复跑四场景全 PASS（§7.3）；场景 B 首轮命中 tmp 写窗口 |
| BUG-2（config 测试 env 竞态） | **✔ 已修复且验证** | registry 套件并行 8 线程累计 4 轮全绿（--no-fail-fast 全量 1 + 专项 ×2 + 默认线程 R1）；7 个 `Config::load` 用例纳入 `with_env_cleaned`（with_env_cleaned 计数 10 处实查）；`--test-threads=8` 定向复现路径不再失败 |

### 7.6 本轮新增缺陷清单（交 DA 修复）

| 编号 | 级别 | 描述 | 证据 | 复现/影响 |
|---|---|---|---|---|
| BUG-3 | 中（测试资产） | registry-e2e.sh **D 场景终局关停无强杀兜底**：`kill -INT …; wait_exit 5 \|\| true` 超时不 `kill -9`（对比 A/B 场景有 `kill -9` 兜底），SIGINT 关停 >5s 时 registry+config 双双泄漏；且脚本头部宣称「启动前检查空闲」但**无隔离端口占用预检**——外来进程占用 18300 时脚本静默误测（本轮首跑 500 误判路径实录） | 本轮 PASS 后 pid 75829/75836 残留（二次 SIGINT 才退出）；DA 验证轮次泄漏 pid 70562/70568（12:19:28 启动，占用 18300 致我首轮失败） | 确定性：修复建议——D 终局加 `wait_exit 5 \|\| kill -9` 兜底；脚本启动前 `lsof -iTCP:$REG_PORT -sTCP:LISTEN` 预检即 die；泄漏进程使后续运行 500 误测 |
| BUG-4 | 高（测试资产） | registry-perf.sh **心跳负载锁步轮转缺陷**：32 worker 各自 `hb_idx`+1 递增 → 不同槽位覆盖仅 ~5/s（意图 200/s 均摊），稳定池大面积超 TTL 被 sweep 剪除 → heartbeat 45% 404（两版对称），违反 TC-38 协议前置「稳定心跳池不因 churn 产生预期 404」 | 首个 404 于心跳序号 4794（≈采样 30s）；错误码全为 404、延迟 0-4ms；v0.1=4410 / v0.2=4341 对称 | 结构性、与采样时长无关；修复建议——心跳目标按跨步跳槽（如 `hb_idx = w + k×CLIENT_CONCURRENCY`，k 递增）或共享原子游标，保证 200/s 均摊到 1000 实例；修复后 TA 复测交付 AC-13 证据 |

### 7.7 降级留痕与局限说明

- TC-38：本轮未复跑延长采样——BUG-4 为结构性缺陷，延长采样不能恢复协议前置条件，复跑无判定价值（避免无意义消耗）；修复后须以相同协议（可延长采样满足 register/deregister ≥1000 样本）重测。
- T3 泄漏清理：本轮及 DA 轮次的泄漏进程均由 TA 人工清理（kill -9）；脚本修复 BUG-3 前每次运行后须人工核查 `pgrep -fl velox` 并清理，否则污染后续运行。
- 其余档位无降级：T2 四轮、T3 脚本级、acceptance 均为完整执行；运行数据全部位于系统临时目录（仓库 `.gitignore` 已覆盖 `data/`、`.run/`、`logs/`，本轮未向仓库写入任何运行数据）。

**终验结论重申**：**有条件通过**——`d78061d` 功能与兼容验收全绿（AC-1~12、AC-14；41TC 可执行项全部通过，含脚本级 e2e 与 8 线程并行稳定性）；AC-13 因 BUG-4 记「无法验证」并附 register 写穿延迟风险信号（11×/12×，样本不足）；BUG-3/BUG-4 测试资产缺陷交 DA，AC-13 复测证据交 GATE3 裁决。
