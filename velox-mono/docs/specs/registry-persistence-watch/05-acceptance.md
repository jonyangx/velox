# 验收报告：registry-persistence-watch（GATE3 终验）

> 工序：F-GATE3（QA 终验关口，只读验收）；日期：2026-11-26。
> 验收对象：feature `registry-persistence-watch`（velox-registry v0.2 状态持久化 + watch 长轮询，registry 侧）。
> 代码基线：分支 `develop`，HEAD=`d78061d`（`d78061d..HEAD` 为空，其后本 feature 无新代码提交）。
> 执行期证据唯一来源：`docs/specs/registry-persistence-watch/04-test-report.md`（常规轮 §1~6 覆盖 `af28e65` + §7 终验轮覆盖 `d78061d`）。本关口不重跑构建/测试，仅核验其完备性、一致性与证据链。

---

## 1. 验收结论

**有条件通过（CONDITIONAL）。**

一句话理由：`d78061d` 产品功能面与兼容性验收全绿（AC-1~12、AC-14 全部满足，T2 四轮全量回归、T3 脚本级 e2e 四场景、acceptance PASS、T0 59/59 均有终验轮实跑记录），唯一未闭合项 AC-13（TC-38）因测试资产缺陷 BUG-4 按协议诚实记「无法验证」而非降阈值放水，缺陷归属清晰（BUG-3/BUG-4 均为测试资产而非产品代码），故以逐条可执行的放行条件交付 RQ-2 性能证据与资产修复义务，而不驳回已验证的产品交付。

---

## 2. 五件套核验记录

### 2.1 SHA 重算结果（本关口独立重算，非采信转录值）

| 工件 | 本关口重算 SHA-256（前 16 位） | 任务锁定值 | 判定 |
|---|---|---|---|
| 01-requirements.md | `7285db59ce8dfe17`（完整 `7285db59ce8dfe170c86880d60df3574cb9739620a4a28148f2ae1e773205996`） | `7285db5994e3d6899…`（**不符**） | 见 2.2 锁异常记录 |
| 02-architecture.md | `f4f3355770f2cd1e`（完整 `f4f3355770f2cd1ef5a20537b8433097b250f40bd1de15539da19091d9cd99c4`） | `f4f3355770f2cd1e…` | **逐字一致** ✅（GATE2 第 2 轮 PASS 版本） |
| 03-test-cases.md | `cd145d5994e3d689`（完整 `cd145d5994e3d6899ee9ed70c0b0c93edefd3ede6ade46d25c0775958a707576`） | `cd145d5994e3d689…` | **逐字一致** ✅（TAD WARN 两项已仲裁：RQ-3 已定案、RQ-2=性能证据） |
| 04-test-report.md | `92473b382974cd7f`（202 行；常规轮 + §7 终验轮） | （任务未给锁值，系本关口证据源） | 结论「有条件通过」，内部一致性核验见 §8 |
| 代码+单测 | 提交链 `28c9114 → af28e65 → d78061d`，HEAD=`d78061d` | 同 | git 核验一致 ✅ |

### 2.2 SHA 锁异常记录（01-requirements.md，判定为任务文本转录错误，非工件漂移——不据此 FAIL，如实留痕）

任务给定的 01 锁定值 `7285db59**94e3d6899ee9ed70c0b0c93edefd3ede6ade46d25c0775958a707576**` 与实际 `7285db59**ce8dfe17…205996**` 不符。本关口独立复核三点取证，全部属实：

1. **机械构成证明**：任务给定值 = 01 实际哈希前 8 位 `7285db59` + 03 实际哈希第 9~64 位 `94e3d6899…707576`（与 03 现行哈希逐字比对一致）。真实 SHA-256 不可能共享 56 位后缀，唯一合理解释是任务文本撰写时以 03 的哈希后段误接 01 的前缀——转录错误，而非被审文件被改。
2. **评审链自证**：GATE1 评审记录自身记载的完整哈希（`01-requirements-review.md:156`：`7285db59ce8dfe17…205996`）与当前文件实际哈希**逐字一致**；AA 输入声明（`02-architecture.md:5`）与 GATE2 第 2 轮评审核验记录（`02-architecture-review.md:234`：`R=7285db59…996，与 G1 定向 PASS 记录一致`）亦吻合。GATE1 定向 PASS、SA 修订、GATE2 REWORK→PASS 的冻结链所指向的即当前文件。
3. **git 零漂移**：`git log -- 01-requirements.md` 仅 `28c9114`（首次入库）与 `af28e65`（SA 定向修订版随 feature 提交）两个提交；`git diff af28e65..HEAD -- <该文件>` 为空，且该文件不在当前 git status 修改列表——自 `af28e65` 后工作树零改动。

**结论**：被审文件与 GATE1/GATE2 评审版本逐字节相同，反漂移目的已满足；异常定性为任务文本转录错误，**不构成验收失败项**，如实记录备查。

### 2.3 评审链与规格链核验

- GATE1 定向 PASS：`01-requirements-review.md:11,162`（`PIPELINE registry-persistence-watch GATE1 PASS`）。
- GATE2 第 2 轮 PASS：`02-architecture-review.md` 末节验证记录（重算 A 哈希、阈值红线复核：AC-8 严格 <1s、AC-13 每端点 ≤2、FR-4 严格 `age > ttl`、FR-5/AC-5 默认 `./data`）+ 末行 `PIPELINE … GATE2 PASS`。
- TAD WARN 仲裁落位：RQ-3（缺省 index 分支语义）已由 GATE2 定案并落入 `02-architecture.md`/TC-40 目标，实现与单测对齐（`default_index_immediate_three_scenarios`、`explicit_zero_immediate_when_version_positive` 等）；RQ-2 定性为性能证据待交付，本关口按裁决点 A 处置。

---

## 3. AC 核对表

> 证据缩写：**S** = `cargo test -p velox-registry`（59/59，测试名逐一在代码中实存）；**T3** = 终验轮脚本级 e2e 四场景（`registry-e2e.sh`，BUG-1 修复后全 PASS，报告 §7.3）；**A** = `velox-demo/scripts/acceptance.sh` PASS（零修改）；**代码** = 本关口只读抽查源码实证。逐条结论均以 04-test-report.md §7 终验轮（覆盖 `d78061d`）为准。

| AC | 内容摘要 | 结论 | 证据出处 |
|---|---|---|---|
| AC-1 | kill -9 后恢复 svc/实例 A，字段一致 | **满足** | T3 场景 A（报告 §7.3：重启恢复 id/address/metadata 一致、`restored_services=1/instances=1`、落盘 age=0s≤TTL60）；S `restart_recovers_instances_with_same_id`（TC-01/03） |
| AC-2 | 恢复后同 id 心跳 200 无 404 | **满足** | T3 场景 A（同 id heartbeat 200 `{"ok":true}`）；S persistence 同 id 恢复用例（TC-02） |
| AC-3 | 周期刷盘 + 心跳不等待落盘 | **满足** | S `dirty_state_machine`、`heartbeat_flush_is_periodic_and_not_blocking`（相位/合并/不阻塞，TC-05/06）；附注：活体 mtime 3 周期观察面报告记「归后续、非阻断」，列入遗留清单（非阻断观察项） |
| AC-4 | 恢复时严格 age>TTL 丢弃 + WARN 计数 | **满足** | S `restore_drops_instances_beyond_ttl`、`restore_dropped_lazy_cleanup`（TC-07/08/09，含 age==ttl 保留 / age==ttl+1 丢弃边界）；代码 `registry.rs:125-126`（`age > ttl_secs` 严格大于） |
| AC-5 | 默认 enabled=true、`./data`；enabled=false 不落盘；flush=0 启动失败 | **满足** | S `persistence_defaults`（true/./data/5）、`disabled_persists_nothing`、`empty_data_dir_rejected_even_when_disabled`、`cross_field_flush_ge_ttl_only_when_enabled`；代码 `config.rs:67-68`（默认值）、`config.rs:135`（flush==0 无论 enabled 拒绝）；T3 场景 D 默认/env 两种启动均接客（TC-10/11/12/37） |
| AC-6 | 损坏快照整文件跳过 + WARN + 正常接客 | **满足** | S `corrupted_snapshot_skipped_and_serves`、`corrupted_variants_are_skipped`（TC-13：截断/缺字段/空文件/未知版本变体） |
| AC-7 | 写失败 500 persist_failed、内存优先、进程不退 | **满足** | S `write_failure_memory_first_then_retry_succeeds`（TC-16，注入后恢复自动重试，G2-1） |
| AC-8 | watch 变更 ≤1s 唤醒；index 旧立即返回 | **满足** | S watch `default_index_immediate_three_scenarios`（<500ms 断言）、注销/sweep/复活唤醒 <1s 断言（`tests/watch.rs:4,30,60,72`）、`explicit_equal_suspends_then_times_out…`；T3 场景 C 挂起 watch 活体唤醒（TC-20/21/25/40） |
| AC-9 | SIGINT 优雅关停唤醒全部挂起 watch、正常退出 | **满足** | S `shutdown_wakes_suspended_watchers` + T3 场景 C（真进程 SIGINT 5s 内实测 0s 退出、**退出码 0**、挂起 watch 返回、退出前最终落盘）；代码 `api.rs:228-231`（shutdown.changed() 唤醒分支）（TC-19/30） |
| AC-10 | 剔除唤醒 watch 且快照更新；纯查询不 bump | **满足** | S `sweep_batch_bumps_once_and_wakes`、`deregister_wakes_queries_do_not`、`revive_heartbeat_watches_exactly_once`（TC-26/27/28/29） |
| AC-11 | timeout_secs 非法 400、缺省精确 30 | **满足** | S `param_errors_all_400_invalid_parameter`、`timeout_range_validation`、`default_index_branch_is_immediate`；代码 `api.rs:163-170`（0/>300 拒绝、缺省精确 30、上限 300）（TC-22/23/24） |
| AC-12 | v0.1 契约零破坏、mesh 零改动 | **满足** | S compat `v01_endpoint_contracts_unchanged`（TC-35）；A acceptance.sh PASS（终验轮 23.3s，步骤 5/5，mesh 周期拉取/转发/LB/TTL 正常，TC-36）；T3 场景 D config 同 cwd 隔离（TC-37） |
| AC-13 | 压测 P99 ≤ v0.1 基线 ×2 | **无法验证** | TC-38 已按冻结协议全量执行（§7.4），因三重协议阻断记「无法验证」：BUG-4 测试资产缺陷（心跳锁步轮转致稳定池大面积 404，两版对称 4410/4341）、register/deregister 样本 905/895<1000、heartbeat/instances_healthy 存在请求错误。执行诚实：未降阈值、未宣布通过、未以不足样本宣布违约。处置见裁决点 A |
| AC-14 | 启动日志字段 + 原子写防半写（代码审查+崩溃注入） | **满足** | 崩溃注入：TC-18b **真实写中 kill 两轮均首轮命中 tmp 写窗口**（终验轮 T3 场景 B 第 1 轮命中），正式快照仅旧/新完整 JSON；代码审查 `store.rs:159-171`（temp + `sync_all` + `rename`）；启动日志字段实证（§3 TC-39：enabled/data_dir/flush_interval_secs/restored_services/instances/dropped_expired/skipped） |

**覆盖缺口抽查**：对照 `03-test-cases.md` §4 AC 覆盖矩阵，14 条 AC 均有 P0/P1 用例映射；41TC 中终验轮 ✔ 40 项（TC-06 活体观察面为非阻断补充面、TC-38 见裁决点 A），无未映射 AC，无未记录的执行缺口。报告 §3 汇总与逐行一致。

---

## 4. 裁决点裁决与理由

### 裁决点 A（AC-13 / RQ-2 / TC-38）：**CONDITIONAL**

- **裁决**：接受报告「无法验证」的诚实结论，以放行条件交付（修复 BUG-4 → 按同一冻结协议重跑 TC-38 → 交付 RQ-2 证据，**阈值不降：分端点 P99 比值每项 ≤2**），不驳回交付。
- **理由（对照参考事实）**：
  1. **功能面全绿**：AC-1~12、AC-14 全部满足且经终验轮针对最终代码 `d78061d` 实跑取证（§3 表），AC-13 是唯一未闭合项，且属非功能性能证据而非功能缺陷。
  2. **执行诚实**：TA 按 03 冻结协议全量执行（真 v0.1 基线 `28c9114` 隔离检出、独立 target、预热 30s、采样 60s×两版、nearest-rank P99、原始逐请求样本留档），三重阻断下**未降阈值、未宣布通过、亦未以不足样本宣布违约**——与协议「缺测/错误/样本不足不得标通过」「存在错误则先记录缺陷/无法验证」逐字吻合。
  3. **缺陷归属测试资产且证据链闭合**：BUG-4 根因（32 worker 锁步 `hb_idx` 自增，本关口实证 `registry-perf.sh:151,158,168` 各自 `+1` 递增 → 槽位覆盖 ~5-6/s 而非 200/s 均摊）为结构性负载驱动缺陷；v0.1/v0.2 错误对称（4410/4341）、首个 404 于心跳序号 4794（≈采样 30s）、错误延迟 0-4ms 与堆算吻合——复跑在修复前无判定价值，TA 不无谓复跑的处置正确。
  4. **为何非 REWORK**：REWORK 的实质是 DA 修复两份测试脚本后重走 TAF——但产品代码无需改动，已验证的绿面（T2 四轮、T3 四场景、acceptance、59/59）无重走价值；CONDITIONAL 的放行条件已把修复与复跑义务逐条钉死（§5），验收标准未被稀释。REWORK 仅在「BUG-4 修复后复测仍无法满足协议前置」或「产品代码需随之改动」时升级。

### 裁决点 B（register 写穿 P99 11×/12× 风险信号）：**接受为已知架构特征，列入遗留关注清单**

- **裁决**：接受 TA 判定——P50=5ms（v0.1 为 0）与 FR-4/AC-4「结构变更写穿落盘（响应完成即文件更新）」的同步落盘成本一致，属**真实的架构性延迟特征**；本轮不构成 AC-13 判定（AC-13 本轮无法验证，且其阈值是 P99 比值）。
- **理由**：AC-4 的写穿语义要求响应返回时快照已落盘，异步化（先响应后落盘）将直接破坏「注册成功 ⇒ 已落盘」的强语义（需求 Q3 定案、G2-1），**不采纳异步化方向**。该信号的正确归宿是知情与跟踪，而非本轮裁决。
- **处置**：①写入本报告遗留关注清单；②附运维建议：对 registry 写盘延迟做监控告警（如快照写耗时/磁盘延迟指标、`persist_failed` ERROR 日志接告警），部署时关注磁盘健康；③与裁决点 A 联动——**BUG-4 修复后的 TC-38 复测若 register/deregister P99 比值仍 >2，不得静默通过**，按放行条件 3 的分支处理（DA 写路径缓解设计如 fsync 策略分级，或需求侧对 AC-13 端点口径的显式裁决），届时重新过 GATE3 复核。

### 裁决点 C（BUG-3，e2e 脚本 D 场景终局关停无强杀兜底）：**测试资产缺陷，纳入放行条件**

- **裁决**：定性确认——本关口实证 `registry-e2e.sh:272-273`（D 场景终局 `wait_exit 5 || true`，无 `kill -9` 兜底）与 `:252-253`（其他位置有兜底）的对比；`:11` 宣称「启动前检查空闲」而 `:95` 证实无独立端口占用预检。与报告 §7.6 一致：SIGINT 关停 >5s 时 registry+config 确定性双泄漏，泄漏进程占用隔离端口会使后续运行产生「注册 500」误测路径（报告 §7.3 首轮失败实录即外来孤儿进程所致，非产品缺陷）。
- **处置**：纳入放行条件 1（DA 修复 + 复跑验证），并在修复前要求每次脚本运行后人工核查 `pgrep -fl velox` 清理（报告 §7.7 已留痕此纪律）。不单独构成驳回理由：泄漏发生在测试资产，且已被发现、留痕、有修复路径。

---

## 5. 放行条件清单（CONDITIONAL——逐条可执行）

| # | 责任方 | 义务 | 验收标准 |
|---|---|---|---|
| 1 | DA | 修复 BUG-3：`velox-registry/tests/registry-e2e.sh` D 场景终局关停（现 :272-273）改为 `wait_exit 5 \|\| kill -9` 兜底；脚本启动前增加隔离端口占用预检（如 `lsof -iTCP:$REG_PORT -sTCP:LISTEN` 非空即 die，:95 注释路径兑现 :11 宣称） | 修复后脚本连续 2 轮四场景全 PASS，且每轮结束后 `pgrep -fl velox` 零残留进程 |
| 2 | DA | 修复 BUG-4：`velox-registry/tests/registry-perf.sh` 心跳目标改为跨步跳槽（如 `hb_idx = w + k×CLIENT_CONCURRENCY`，k 递增）或共享原子游标，保证 200/s 均摊覆盖全部 1000 槽位 | 复测运行中稳定池零预期 404（协议前置「稳定心跳池不因 churn 产生预期 404」成立），register/deregister 有效样本各 ≥1000 |
| 3 | TA | 修复到位后按 03 §TC-38 同一冻结协议重跑 TC-38（v0.1=`28c9114` 隔离基线、同机同 release profile、预热≥30s、采样可延长以满足 ≥1000 有效样本、nearest-rank P99、原始样本留档），在 04-test-report.md 增补终验复测记录并交付 RQ-2 证据表（分端点 P50/P99/比值）。**阈值不降：每端点 v0.2/v0.1 P99 ≤2**；分支处理：若 register/deregister 比值仍 >2 → 不得判 AC-13 通过，按裁决点 B 第③条升级（DA 写路径缓解设计或需求侧端点口径裁决 + GATE3 复核），并保留 register 写穿延迟特征如实记录 | 04-test-report.md 含针对修复后代码的 TC-38 复测章节：无任何端点请求错误、样本量达标、比值表完整；AC-13 据实判定（满足，或按分支升级路径处置） |
| 4 | DA/流程 | 文档收口提交：将 `docs/specs/registry-persistence-watch/04-test-report.md` 与本报告 `05-acceptance.md` 以 docs(specs) 提交入库（当前 04 为未跟踪文件，属流水线文档工序时序，非缺陷；提交前不应有本 feature 新代码提交混入） | 提交存在且仅含规格/报告文档；`git log` 无 d78061d 之后本 feature 代码改动 |
| 5 | DA（可并入条件 4 提交） | velox-mono 文档同步：`velox-mono/docs/subsystems.md` velox-registry 章节（API 概览含 watch 端点、演进路线 v0.2 状态）与 `velox-mono/docs/architecture.md` §7 v0.2 行措辞（需求 Q8）；若上游决策显式延后，以决策记录替代本条 | 文档 diff 入库，或上游显式延后决策记录存在 |

条件 1~3 完成并经 TA 复核留痕后，本 feature 的 CONDITIONAL 状态解除；届时 AC-13 据实判定，无需重走已验证绿面（T2/T3/acceptance 对未改产品代码持续有效，条件 1/2 仅改测试脚本）。

---

## 6. 分支与提交检查记录

- **分支与链**：分支 `develop`；HEAD=`d78061d`；`git log d78061d..HEAD` 为空——终验轮覆盖提交即当前 HEAD，**其后无本 feature 新代码提交** ✅（满足「终验轮之后无新代码」红线）。
- **提交链**：`28c9114`（docs(specs)：需求/架构/用例 + GATE1 评审）→ `af28e65`（feat(registry)：v0.2 持久化 + watch，含本 feature 五份规格）→ `d78061d`（test(registry)：BUG-1/BUG-2 测试资产修复）。链条完整、顺序合理：规格先行、功能实现、测试资产整备。
- **范围核验**：`af28e65` 触及 20 个文件 = 本 feature 5 份规格（01/01-review/02/02-review/03）+ `velox-registry/` 全部（src 7 文件、tests 4.rs+2.sh、README、config toml）——与需求 §8 影响面（仅 velox-registry）一致，**零改动 mesh/config/demo** ✅（`velox-demo/` 工作树零改动实证）。`d78061d` 仅 2 文件（`src/config.rs` tests + `tests/registry-e2e.sh`），与「纯测试资产修复」说明一致。
- **提交信息质量**：两条均为 Conventional Commits（`feat(registry):`、`test(registry):`），正文结构化说明实现要点、GATE 引用、验证证据，与 diff 内容核对一致（如 d78061d 声称 config.rs diff 全在 mod tests——与本关口 BUG-2 修复位置抽查吻合）。
- **敏感信息/调试残留**：两提交 diff grep 无 `dbg!`/`println!`/`todo!`/`unimplemented!`/password/secret/token/api_key 命中 ✅。
- **运行数据**：两提交均未含 data/logs/.run 类文件；`.gitignore:27,30,31` 覆盖 `data/`、`.run/`、`logs/`；报告声明运行数据全在系统临时目录 ✅。
- **工作树在途改动归属说明**（均非本 feature 代码产物）：`docs/specs/instance-concurrency-limit/{01,02}-*.md` 修改与未跟踪评审/用例文档 = F1/F3 feature 在途规格工件；`velox-config/src/{config.rs,limit.rs}` = 纯 rustfmt 格式化（报告基线含之且全绿佐证无行为影响）；`docs/specs/_pipeline-state.md`、`docs/specs/addlog/`（02-architecture.md:5 声明的行文参考材料）= 流水线工作材料；`velox-mono/docs/brd/` = 其他流程 BRD 材料；`docs/specs/registry-persistence-watch/04-test-report.md` 未跟踪 = 本 feature 测试报告尚未走 docs 提交工序（时序问题，放行条件 4 收口）。

---

## 7. 遗留问题盘点与建议

| 项 | 状态 | 归属与处置 |
|---|---|---|
| BUG-1（e2e 端口 env 未注入） | **已修复且验证关闭** | 终验轮 6 处 `SERVER__ADDR` 注入实查（本关口 grep 计数 6 一致）+ 复跑四场景全 PASS（§7.3） |
| BUG-2（config 单测 env 竞态） | **已修复且验证关闭** | 7 个 `Config::load` 用例纳入 `with_env_cleaned`；并行 8 线程累计 4 轮全绿（§7.5） |
| BUG-3（e2e D 场景无强杀兜底 + 无端口预检） | **已记录待办 → 放行条件 1** | 测试资产缺陷，DA 修复；修复前每次运行后人工 `pgrep -fl velox` 清理 |
| BUG-4（perf 心跳锁步轮转） | **已记录待办 → 放行条件 2/3** | 测试资产缺陷，DA 修复 + TA 重跑 TC-38 交付 RQ-2 证据（阈值不降） |
| AC-13/RQ-2 证据交付 | **责任方=TA（复测）+DA（前置修复）**，义务钉入放行条件 3 | 若复测 register/deregister 仍 >2 → 升级端点口径裁决（裁决点 B③），不静默通过 |
| register/deregister 写穿延迟特征（P50=5ms、P99 11×/12×） | **已知架构特征，遗留关注** | 裁决点 B：监控写盘延迟告警；BUG-4 修复后复测联动观察；不采纳破坏写穿语义的异步化 |
| demo_consumer `defaults_match_contract` 既有 flaky | **上游既有问题，非本 feature 引入** | 与 DA 报告/常规轮记录一致（纯 HEAD 基线可复现、隔离必绿）；同 BUG-2 类 env 竞态测试资产问题，交 DA 与 BUG-2 同批修复（demo 范围，不阻断本 feature） |
| velox-mono 文档同步（subsystems.md + architecture.md §7 Q8） | **待办 → 放行条件 5** | 需求 §8 文档同步面；上游若决策延后须留决策记录 |
| TC-06 活体 mtime 3 周期观察面 | **非阻断观察项** | 集成层已证周期/合并不阻塞语义；可在后续脚本级运行顺带观察 |
| TC-39 DEBUG 级日志细节（watch 唤醒/超时） | **非阻断观察项** | INFO 级启动/恢复/关停字段已实证；DEBUG 细项后续验证 |
| 需求 B-5（provider 无重注册） | **范围外既有限制** | 需求明示属 velox-demo 范围外，registry 文档已声明部署约束（README 已同步） |

---

## 8. 取证记录（本关口只读取证动作与交叉核验）

1. **SHA 重算**：`shasum -a 256` 对 01/02/03/04 四文档独立重算（§2.1）；01 锁异常三点取证独立复核（§2.2）。
2. **git 取证**（全部只读）：`git status --porcelain`、`git log --oneline -5`、`git log d78061d..HEAD`（空）、`git show --stat af28e65/d78061d`、`git diff af28e65..HEAD -- <三规格>`（空）、提交元数据与全文、diff 敏感串扫描（零命中）、`.gitignore` 覆盖核对。
3. **报告内部一致性核验**：
   - 结果统计交叉验算：§7.4 原始样本 16331（v0.1）/16107（v0.2）与分端点有效样本精确吻合（9674+2431+2416+905+905=16331；9541+2398+2378+895+895=16107）——**统计不可疑** ✅；
   - 套件规模一致性：38 lib + 2 compat + 8 persistence + 11 watch = 59（本关口 grep `#[test]/#[tokio::test]` 逐文件计数一致）；终验轮 R1 95/95 分目标加总一致；常规轮 93 passed/2 failed 加总一致；
   - 报告声称的 11 个关键测试名逐一在代码中实存（`restart_recovers_instances_with_same_id`、`corrupted_snapshot_skipped_and_serves`、`shutdown_wakes_suspended_watchers`、`dirty_state_machine` 等）；
   - BUG-3 现场状态与报告逐字吻合（`registry-e2e.sh:272-273` 无兜底 vs :252-253 有兜底；:11 宣称空闲预检 vs :95 无预检）；
   - BUG-4 现场状态与报告吻合（`registry-perf.sh:151,158,168` 锁步 `hb_idx` 自增模式，堆算 ~200/32≈6 槽/s 与报告 ~5/s 同量级）；
   - BUG-1 修复在位（e2e 脚本 `SERVER__ADDR` 6 处，与 §7.3 记录一致）。
4. **冻结项落位抽验**（实现与测试双落位）：FR-4 严格 `age > ttl`（`registry.rs:125-126` + 边界用例）；FR-5/AC-5 默认 true/`./data`/5（`config.rs:67-68` + `persistence_defaults`）；FR-6/校验域 fail-fast（`config.rs:127-135` flush==0 无论 enabled 拒绝、空 data_dir 拒绝、跨项仅启用态拒绝）；AC-8 ≤1s（`tests/watch.rs` <500ms/<1s 断言，严于阈值）；AC-9 shutdown 唤醒（`api.rs:228-231` + 真进程 SIGINT exit 0）；NFR-5 原子写（`store.rs:159-171` temp+sync_all+rename）；NFR-6 文档同步（README/velox-registry.toml persistence/watch 章节实存）。
5. **协议符合性核验**：TC-38 执行与 `03-test-cases.md:112` 冻结协议逐条对照（真基线 commit、隔离检出、预热/采样时长、样本量要求、nearest-rank、错误即无效、不降阈值）——报告执行与结论口径一致。
6. **评审链核验**：GATE1 PASS 记录（01-review:11,162）、GATE2 第 2 轮 PASS 记录（02-review 末节 + 末行标记）、02-architecture.md:5 输入哈希声明；TAD 仲裁两项落位核验。
7. 本关口仅新建本文件，未改动任何其他文件，未运行构建/测试/服务，未执行任何 git 写操作。

---

**验收结论重申：有条件通过（CONDITIONAL）**——放行条件见 §5（DA 修 BUG-3/BUG-4、TA 按冻结协议重跑 TC-38 交付 RQ-2 证据且阈值不降、文档收口提交、mono 文档同步或上游显式延后）。功能面与测试资产整备成果予以确认，AC-13 证据义务未解除前不解除本关口。

PIPELINE registry-persistence-watch GATE3 CONDITIONAL 功能面全绿（AC-1~12/14 满足、T2 四轮+T3 四场景+acceptance+59/59 终验实跑）；AC-13 因测试资产缺陷 BUG-4 诚实记无法验证，裁决 CONDITIONAL 而非 REWORK；放行条件：①DA 修 BUG-3（e2e 强杀兜底+端口预检，2 轮全 PASS 零残留）②DA 修 BUG-4（perf 心跳跨步轮转，稳定池零 404、样本≥1000）③TA 按冻结协议重跑 TC-38 交付 RQ-2 证据，阈值不降（每端点 P99≤2；register/deregister 仍超则升级端点口径裁决不静默通过）④04/05 报告 docs 收口提交⑤mono 文档同步或上游显式延后；01 锁值异常经三点取证判定为任务文本转录错误非工件漂移，已如实留痕不构成 FAIL
