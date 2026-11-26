# 验收报告：addlog —— 子系统日志本地 rotate（velox-log 共享库 + 五包接入）

> 工序：F-GATE3 终验关口 · Feature：addlog · 分支：develop · HEAD：`4b68ebfe11f540c2abd3090d7f180ab5c1340175`
> 验收基准：`01-requirements.md`（冻结 SHA `ce54e325…`）、`02-architecture.md`（SHA `a154e264…`）、`03-test-cases.md`（v1 定稿 SHA `1aaa6f0f…`）、`04-test-report.md`（常规轮 + 终验轮，终验结论=通过）
> QA 证据方式：全部只读取证 + 独立实跑抽查（fmt/clippy/build --locked、velox-log 28 例、workspace 130 例、demo-consumer 隔离复跑、acceptance.sh e2e ×3 轮、活体 logs/ 目录清点与残留进程检查）；QA 本轮零 git 提交。

## 1. 验收结论

**通过（PASS）**——AC-1～AC-14 全部满足且经 QA 独立实跑交叉证实；五件套齐备一致；终验轮合规（T2/T3 于最终提交 `4b68ebf` 实跑、T1 跳过留痕理由成立）；BUG-1 闭环（代码修复实证 + 复验全绿）；提交历史合规、无敏感信息与调试残留。

## 2. AC 核对表

| AC | 内容摘要 | 判定 | 证据（04 报告 + QA 独立实证） |
|----|---------|------|------------------------------|
| AC-1 | 裸进程默认落盘 `logs/<子系统名>.log` | 满足 | TC-01（T1 裸进程真实 TTL WARN 落盘，04 §3）；QA 实测：acceptance.sh 以裸进程启动后 `logs/velox-registry.log` 含真实 WARN（`instance expired, removed` / `恢复时心跳已超 TTL`），时间戳+级别+消息齐备 |
| AC-2 | 5 进程独立文件、无串写 | 满足 | TC-02（04 §3 实名五文件双模式核验）；QA 实测 `ls logs/`：精确 5 实名 `.log` + `demo-provider-2/` 分目录，无多余副本 |
| AC-3 | 硬字节边界 / 0 超限容差 / 超长截断 / 旧文件收敛 | 满足 | TC-04/TC-03（04 §3）；QA 实测 `cargo test -p velox-log` 28/28（含 `writer_exact_fill_rotates_on_next_record`、`oversize_record_truncated_within_cap`、`init_normalizes_oversize_and_legacy_copies`）；`velox-log/src/lib.rs:39` `MAX_FILE_BYTES=200*1024*1024` 与冻结口径一致 |
| AC-4 | 含活跃最多 3 份、最旧删除 | 满足 | TC-05/TC-03；QA 实测 `writer_retention_removes_oldest` 随 28/28 全绿 |
| AC-5 | 轮转并发不崩溃 | 满足 | TC-06 终验升级为通过：BUG-1 修复代码实证（`velox-log/src/tests.rs:1318` `set_nonblocking(false)`、`:1389/:1414` `ChildGuard` + `impl Drop` kill+wait 回收）；TA 终验 23/23 零孤儿（04 §7.3）；QA 实测 `cargo test -p velox-log` 全绿（含 `live_rotation_concurrent`，1.11s） |
| AC-6 | 目录不存在自动创建（含多级父目录） | 满足 | TC-08（T1 真实 velox-config 多级 `<tmp>/a/b/c/logs` 实证，04 §3） |
| AC-7 | `[log].dir` / `VELOX_*__LOG__DIR` 覆盖 | 满足 | TC-09（04 §3 三+1 场景）；QA 实测 acceptance.sh run2/3：`VELOX_PROVIDER__LOG__DIR=logs/demo-provider-2` 生效，`logs/demo-provider-2/demo-provider.log` 存在 |
| AC-8 | 默认 warn：INFO 静默、WARN/ERROR 可见 | 满足 | TC-11/TC-12（04 §3）；QA 实测：默认 warn 下 `velox-registry.log` 全程仅 WARN 行、零 INFO 泄漏 |
| AC-9 | 优先级链 `RUST_LOG > LOG__LEVEL > 文件 > warn`、高合法覆盖低非法 | 满足 | TC-13/TC-14（04 §3 T0+T1 十三场景）；QA 实测 `level_chain_priority_table`/`level_chain_edge_cases` 随 28/28 全绿 |
| AC-10 | 非法值/空串 fail-fast、两套语法分流 | 满足 | TC-15（04 §3 真实二进制 6 场景 rc=1 含值与来源）；QA 实测 `strict_syntax_whitelist` 全绿 |
| AC-11 | 旧配置（无 `[log]`+未知字段）兼容 | 满足 | TC-18（04 §3 T0+T1 实证） |
| AC-12 | CI 四命令全绿 + 默认值/常量单测 | 满足 | QA 独立实跑：`cargo fmt --all --check` ✓、`cargo clippy --workspace --all-targets -- -D warnings` ✓、`cargo build --workspace --locked` ✓、`cargo test --workspace --locked --no-fail-fast` 129/130（唯一失败为范围外既有 flaky，隔离 11/11 绿，见 QA-5）；常量与代码一致（`lib.rs:39/41/45` 200 MiB / 3 / 1024）；`Cargo.toml:26/29` 仅 `[workspace.dependencies]` 引入 velox-log path + `tracing-appender =0.2.4`，`Cargo.lock` 锁 `time 0.3.36`，MSRV `rust-version="1.80"` |
| AC-13 | 单进程逻辑字节 ≤3M + 无未受管副本 | 满足 | TC-07/TC-21/TC-22（04 §3）；QA 实测 3 轮 e2e 后 `logs/` 无 `nohup.out`/`*.stderr.log`（grep 0 匹配），受管文件精确 |
| AC-14 | 文档与实际行为一致 | 满足 | QA 实测：`velox-mono/docs/adr/0002-log-rotation.md` 存在、`development.md:67-70`（rotate/warn/RUST_LOG=info）、`subsystems.md:62/112/172`（core 三件套 [log] 示例）、`scripts/README.md:64-69`、五份实名 TOML 均 `[log] dir="logs" level="warn"`（velox-mesh:20/velox-config:15/velox-registry:27/demo-provider:26/demo-consumer:23）；RQ-A-1 注记实证落位 `02-architecture.md:555`；TC-20 启动行规范（04 §3 T1 实证） |

**结论：AC-1～AC-14 共 14 条全部满足，无不满足、无无法验证项。**

## 3. 交付产出清单

| 产出 | 判定 | 证据 |
|------|------|------|
| ① 需求文档 `01-requirements.md` | 有（194 行） | SHA 复核 `ce54e325…` 与冻结声明一致；AC-1～14 完整可判定 |
| ①' 需求评审记录 `01-requirements-review.md` | 有（190 行） | GATE1 定向复审 PASS（03 头部引用其结论） |
| ② 架构文档 `02-architecture.md` | 有（555 行） | SHA 复核 `a154e264…` 一致；§1~§9 结构完整（含 §9 待确认问题处置） |
| ②' 架构评审记录 `02-architecture-review.md` | 有（269 行） | 第 2 轮 PASS（含 P3-A1/A2/A3 留痕要求） |
| ③ 测试用例 `03-test-cases.md` | 有（178 行） | SHA 复核 `1aaa6f0f…` 一致；TC-01～25；§4 AC 覆盖矩阵 14 行全覆盖「缺口：无」，P0/P1 无缺口；RQ=0、OP-1~10 全闭合 |
| ④ 测试报告 `04-test-report.md` | 有（197 行） | 常规轮覆盖 `a5b75e5` + 「7. 终验轮」覆盖 `4b68ebfe11f5…`；25/25 TC 逐条结论与 §3 用例表对应；缺陷均附复现命令/堆栈/孤儿 PID 证据；**终验轮核对：T2 实跑 130 例=129+1 已知 flaky、T3 acceptance.sh PASS 5/5、T1 跳过附触发判定留痕（区间内仅测试资产改动，活体链路由 T3+BUG-1 复验覆盖）——合规降级留痕成立，未降级未免除**；终验覆盖提交 = 当前 HEAD，其后无本 feature 新提交 |
| ⑤ 代码 + 单测 | 有 | `a5b75e5`（feat(log) velox-log 共享库 + 五包接入 + 脚本 + 文档，45 文件）→ `4b68ebf`（test(log) BUG-1 测试资产整备，仅 `velox-log/src/tests.rs` 65+/18-）；区间内 `4b24a20` 为 F3（instance-concurrency-limit）随行提交，diff 核对不触及 addlog 行为 |

**五件套齐备且非空。04 报告结果统计经 QA 独立实跑逐套复核完全吻合**（velox-log 28/28、velox-config 23/23、velox-registry 38+2+8+11=59/59、velox-mesh 4/4、demo-provider 5/5、demo-consumer 10/11+隔离 11/11；合计 130 例与报告口径一致，无「通过数超套件规模」类可疑项）。

## 4. 分支与提交检查结果

- **分支**：develop，HEAD=`4b68ebfe11f5…`（= 终验轮覆盖提交，终验后无本 feature 新代码提交 ✓）；分支未推送属本仓库工作常态，不计问题。
- **提交历史**：`a5b75e5` feat(log) → `4b24a20` fix(config)（F3）→ `4b68ebf` test(log)；均为 Conventional Commits，正文详实（选型/依赖锁图/验证记录），无 wip/调试残留。
- **提交内容与变更说明一致性**：抽查 `4b68ebf` diff 与其 message 逐项对应（`set_nonblocking(false)` + `ChildGuard` RAII）；`a5b75e5` 抽查五包接入（`velox-config/src/main.rs:30-35` spawn_blocking init + `_log_guard`）、`scripts/run.sh:43-46`（stdout=/dev/null + stderr 匿名管道）、`run.sh:174`（tail -F）、`acceptance.sh:227-234`（旧副本精确清理 + 第二 provider 分目录）均与说明一致。
- **敏感信息检查**：`git diff 28c9114..4b68ebf` 全量扫描 api-key/secret/token/password/私钥块 0 命中；新增代码调试残留（println!/TODO/FIXME）0 命中。
- **工作树**：本 feature 范围内仅 `docs/specs/addlog/04-test-report.md` 未跟踪（按既定收口策略待 GATE3 后统一 `docs(specs)` 提交，见 QA-4）；`M velox-config/src/limit.rs` 经 diff 核实为纯 rustfmt 换行（6+/5-，无行为变化），属 F3 在途工件，不计入本 feature；其余未跟踪项（`_pipeline-state.md`、F2/F3 报告、`velox-mono/docs/brd/`）均属其他 feature，原样保留未触碰。

## 5. 问题清单

| 编号 | 描述 | 证据 | 严重级别 | 处置建议 |
|------|------|------|---------|---------|
| QA-1 | **范围外观察（非 addlog 缺陷）**：QA 复跑 `acceptance.sh` 3 轮中 1 轮（首轮）步骤 4.7 fail——SIGKILL 第二 provider 后 20s 断言窗内 registry 健康实例未回落为 1；该轮 `logs/velox-registry.log` 在运行窗口内**无任何** `swept expired instances` WARN（对比 run2/3 均如期出现且全 PASS）。指向 velox-registry TTL sweep 任务偶发未在窗口内触发（TTL=6s/sweep=2s 验收配置）。 | QA 首轮输出 `[fail] registry demo-provider 健康实例数回落=1：20s 内未等到`；run2/run3 `SCRIPT_EXIT=0`、0 fail；registry 日志 run1 窗口（08:32:04 UTC 起）无 swept 行，run3 窗口 08:34:12/08:34:18 两行 swept WARN | P3（范围外） | 不阻塞本验收（addlog 仅改日志初始化，不改 registry sweep 行为；属 registry-persistence-watch 交付域）。建议 registry 范围跟进：排查 sweep 定时任务在并发/负载下的调度延迟与存活路径（如 tokio interval tick 丢失、持久化刷盘互斥等） |
| QA-2 | P3-A1 尽力语义留痕确认：TC-24（guard 2s 测试预算）、TC-25（120ms 观测窗）不代表 worker 卡死/`kill -9` 下报告必达 | 04 §3 TC-24/25「附 P3-A1 留痕」+ §5 观察 1；GATE2 评审要求 | P3（留痕） | 维持观察；如后续需求强化可作输入 |
| QA-3 | 两项非缺陷观察沿用：TC-21④ `run.sh` 健康检查失败文案未显式注明「启动错误已输出到 stderr」；TC-20 core 三件套 `started` 行无 `config` 路径（文件来源可经 `source=file:<绝对路径>` 追溯） | 04 §5 观察 2/3；`scripts/run.sh:49-53` 实查 | P3（体验优化） | 列后续优化备选项，不阻塞 |
| QA-4 | **docs 收口待办（流程项）**：`04-test-report.md` 与本报告 `05-acceptance.md` 为未跟踪文件，按既定策略待 GATE3 后由编排以单个 `docs(specs)` 提交收口 | `git status`：`?? docs/specs/addlog/04-test-report.md`（05 由本轮新建） | 流程（放行条件） | GATE3 通过后执行：`docs(specs): addlog 测试报告与验收文档` 单提交收口（04+05 一并入库），不混入其他 feature 工件 |
| QA-5 | 范围外既有 flaky（留痕）：`demo_consumer::config::tests::defaults_match_contract` workspace 并行 env 竞态（期望 9100 / 实测 19999） | QA 实测复现同签名（`cargo test --workspace --no-fail-fast` 唯一失败项）；隔离 `cargo test -p demo-consumer -- --test-threads=1` 11/11 绿 | P3（范围外） | F2/F3 已登记，非本 feature 引入；建议 demo 范围做测试 env 隔离修复 |
| — | **BUG-1 终态：已闭环** | 修复代码实证（tests.rs:1318/1389/1414）+ TA 终验 23/23 零孤儿（04 §7.3）+ QA 独立跑 28/28 全绿；无阻塞交付的未修复缺陷 | — | 无需处置 |

## 6. 放行条件清单（结论为通过，以下为移交事项而非放行障碍）

结论为 **通过**，无阻碍放行的缺陷。移交编排执行的事项：

1. **docs 收口（QA-4）**：以单个 `docs(specs): addlog 测试报告与验收文档` 提交将 `04-test-report.md` + `05-acceptance.md` 入库；严禁携带 `velox-config/src/limit.rs`、`docs/specs/_pipeline-state.md` 及其他 feature 工件。
2. **范围外跟进（QA-1）**：registry sweep 偶发延迟交由 registry-persistence-watch 范围排查；QA-5 demo flaky 交由 demo 范围修复。
