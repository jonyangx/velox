# 需求评审：log-to-common（F-GATE1）

> 工序：F-GATE1（需求评审关口），QA 执行，v1 首轮（评审对象为 01-requirements.md v1，SA 首轮产物，无前置评审反馈）。
> 评审基线：develop @ `cf75e33`，工作树含 `velox-config/src/limit.rs` 既有改动（与 01 声明一致）。
> 评审方式：文档逐条评审 + 仓库只读抽查实证（未改动任何文件，本文件为唯一写入）。
> 抽查时点：本评审执行时点，行号基于 `cf75e33` 工作树。
> 并行工序 F-TA-C 的 03-test-cases.md（v0 草案）不在本次评审范围；OP-1~OP-6 实际仲裁留待 GATE2，本关口只检查其是否附建议默认方案。
> 登记溯源：本评审初版已由 QA pane 会话完成并落盘（taskId QA-1795693777498-43da28c9ca3de79c）；编排者以 QA-1795738519538-7c9897c1ca856f84 重派同一工序时，经读工件核实本评审对同一评审对象有效——01-requirements.md SHA256 前 16 位 `1b0dc1d6c0a20f9a`、226 行、基线 develop@cf75e33 均未变，QA-1/QA-2 关键证据复验零漂移（5 处连字符注释行号精确一致、4 常量消费方 grep 仍零命中、AC-3 模式未变）——按共用协议沿用本结论，不盲目重做。

## 1. 评审结论

**REWORK（驳回返工，第 1/2 轮）**。

理由：01 整体质量高——FR/AC 判定方式、边界承接、范围外、OP 默认方案、需求忠实度五项通过，行号级声明经只读抽查几十处几乎全部精确吻合；但 §8 影响面存在**实测遗漏**：五包源码注释中各有一处连字符 `velox-log` 字面（共 5 处）未列入触点清单，且 AC-3 的自查 grep 只查下划线形式，无法捕获该形式残留——按 FR-5 层①「零残留」口径，这两点会使 DA 交付在残留检查上不可判自洽，须补正后复审。

## 2. 逐项检查结果

### 2.1 FR/AC 可判定真伪

**结论：基本通过，AC-3 判定命令不完备（QA-1 的一部分）。**

- FR-1~FR-7：均有客观判定方式。FR-1（ls/根 Cargo.toml）、FR-2（API 对照+28 例单测）、FR-3（grep+Cargo.toml+build）、FR-4（grep lock+`--locked` 构建）、FR-5（分层 grep）、FR-6（文档逐点核对）、FR-7（结构审视+禁空壳占位）——逐条可执行。✅
- AC-1~AC-8：Given/When/Then 完整，命令客观可复现。AC-7「审视新增平级组件改动面」偏定性，但被「无空壳占位组件」「无需改动 log 内部与五包接入」两个可验证条件约束，可判定。AC-6 的「不再以 velox-log 为现行 crate 名描述」结合 OP-4 默认方案（新增 ADR+0002 注记）可判定。✅
- **缺陷**：AC-3 When① 的命令 `grep -rn "velox_log" --include="*.rs" …` 仅匹配下划线形式；五包 config.rs 注释中的连字符 `velox-log §4.4` 字面（见 QA-1）在该命令下不可见，导致 FR-3 的直接验收标准查不出层①全部残留形式。AC-5 层①（`"velox-log\|velox_log"`）可兜底，但 FR-3↔AC-3 自身应自洽。

### 2.2 边界与异常覆盖

**结论：通过。**

§5 八条场景逐条核对承接关系：

| 场景 | 承接 | 核实 |
|---|---|---|
| 1 Cargo.lock 失同步 | FR-4/AC-4（同提交重生成+`--locked` 证明） | ✅ |
| 2 只改 src 漏 tests | FR-3 明示 registry 测试触点、AC-3 检查面显式含 `velox-registry/tests`（实测仅 tests/common/mod.rs:39-40 两处，属实） | ✅ |
| 3 行为字符串误伤 | FR-5 层④/OP-5/AC-5④（report.rs:291 `component=velox-log`、lib.rs:187 `thread_name("velox-log")` 实测存在） | ✅ |
| 4 不受影响点防误改 | §5.4（五处 `init(env!("CARGO_PKG_NAME"),…)` 实测吻合）+ AC-8 diff 核对 | ✅ |
| 5 并行单测环境纪律 | FR-2 注入式设计要求+AC-2（EnvInputs/FaultFs/TestReportSink/RotationLimits::injected 四件套实测存在：filter.rs `EnvInputs`、tests.rs:124 `FaultFs`、report.rs `TestReportSink`、rotate.rs:45 `injected`） | ✅ |
| 6 doc 注释/锚点漂移 | FR-2 末条+§5.6（lib.rs:1 自述实测存在） | ✅ |
| 7 构建缓存 | §5.7 以干净命令结果为准 | ✅ |
| 8 crates.io 同名 | §5.8+范围外 6 | ✅ |

rotate/迁移/残留清理等关键异常均有 AC 或 FR 承接，无悬空场景。

### 2.3 范围外明确

**结论：通过。**

§6 七条与「本期只做结构重构与 log 组件迁移」一致：不新增/迁移其他组件（=1）、不动日志能力本身（=2，且以 OP-5 默认为条件闭环）、不回溯历史规格（=3）、不留兼容壳（=4，与意图原文口径一致）、纪律文件不触碰（=5）、不发布/不升级（=6）、不重构五包 config.rs 结构仅最小 diff（=7）。无范围蔓延，无漏项。

### 2.4 待确认问题附建议默认方案

**结论：通过。**

OP-1~OP-6 每条均附可执行的建议默认方案：OP-1 顶层再导出（diff 最小、与 FR-7 不冲突）、OP-2 完全移除（无外部消费者）、OP-3 历史工件原样保留（证据链）、OP-4 新增 ADR+0002 注记（ADR 不可变惯例）、OP-5 行为字符串保留原值（行格式是文档化契约）、OP-6 addlog 术语不动。每条给出了改判时的联动口径（如 AC-5③④ 的条件分支），GATE2 可直接裁决。

### 2.5 影响面准确（只读抽查实证）

**结论：绝大部分精确吻合，存在一处实质遗漏（QA-1）。**

已核实吻合项（证据为只读命令实测）：

| 声明 | 实测 |
|---|---|
| `velox-log/src/` 六文件共 3346 行（295/672/242/370/113/1654） | ✅ wc -l 精确一致 |
| 28 例 `#[test]` 全部集中于 tests.rs | ✅ |
| 根 `Cargo.toml:9` members 含 `"velox-log"`、`:26` path 依赖 | ✅ |
| Cargo.lock 6 处（181/200/1537/1541/1567/1585） | ✅ 行号逐一命中 |
| 五包 `Cargo.toml:11` `velox-log.workspace = true` | ✅ 逐包命中 |
| 全部 `velox_log::` 触点行号（config/mesh/registry/provider/consumer 的 use、absolutize、resolve_level、init 共 17 处） | ✅ 行号逐一命中 |
| registry 为唯一测试侧引用包（tests/common/mod.rs:39-40） | ✅ 全仓 grep 证实 |
| adr/0002 六处字面（行 1/18/41/46/57/58） | ✅ 逐一命中 |
| subsystems.md:61/111/171 三处 addlog 注释、development.md:67 | ✅ |
| architecture.md、根 README.md、acceptance.sh、全部 *.sh 零 velox-log/velox_log/addlog 引用 | ✅ |
| `_pipeline-state.md` 含 velox-log 2 处（64/65 行） | ✅ |
| specs/addlog 约 62 处、instance-concurrency-limit/04:81 与 05:86 | ✅ 62 处精确、两行命中 |
| NFR-1 锚点（lib.rs:1/14/39-45/93/104/122/126/136/149/182-189/219/245-261/266、report.rs:7/291、rotate.rs/bounded.rs/filter.rs 头注释） | ✅ 全部实存且语义相符 |
| FR-2 API 面（LogConfig/ResolvedLevel+filter()/source_display()/LevelSource、resolve_level、absolutize、init、LogGuard、4 常量）与 `pub mod` 四模块+`pub(crate)` 条目 | ✅ 实存 |
| NFR-4 依赖面（tracing/tracing-subscriber/tracing-appender/serde/anyhow） | ✅ crate Cargo.toml 实测 |

**遗漏**（QA-1，证据）：

```text
velox-config/src/config.rs:101   // 由 resolve_level 直接读 var_os（保留空串语义，见 velox-log §4.4）
velox-registry/src/config.rs:129 // 同上
velox-mesh/src/config.rs:103     // 同上
velox-demo/provider/src/config.rs:132 // 同上
velox-demo/consumer/src/config.rs:127 // 同上
```

五处连字符 `velox-log` 字面位于五包 `src` 注释中，按 FR-5 层①口径（五包 src 零残留）属须清零触点，但 §8.1「须改动」清单未列、AC-3 自查命令查不出（见 2.1）。AC-5 层① 可兜底，但影响面清单自称「现状核实」且其余行号精确到行，此处遗漏使清单不自洽。

### 2.6 需求忠实度

**结论：通过（一处措辞瑕疵，QA-2）。**

- 原文「重构 velox-log，修改为 velox-common，log 只是其中一个组件」→ G1/G2/FR-1 完整承接；拼写笔误处理方式声明清晰。✅
- 「velox-log 不再独立存在」→ G2/FR-1/FR-4/OP-2/范围外 4 闭环。✅
- 「不回退」→ G3/NFR-1 不变量表（锚点实测）/AC-2/AC-8 闭环。✅
- 「只做结构重构与 log 组件迁移」→ FR-7/AC-7（结构性预留、禁占位）/范围外 1 闭环。✅
- 「预留扩展位」→ G4/FR-7/AC-7，且明确「结构性而非占位性」。✅
- velox-mono 文档引用同步 → G5/FR-6/AC-6，处置粒度到行。✅
- 无漏译、无加戏：NFR-3 工作树纪律（limit.rs 不混入）基于现场事实（git status 实测相符），FR-4 lock 一致性为 rename 必然工程要求，属合理细化。✅

## 3. 问题清单

| # | 描述 | 证据 | 严重级别 | 处置建议 |
|---|---|---|---|---|
| QA-1 | §8.1 影响面遗漏五包 config.rs 注释 5 处连字符 `velox-log §4.4` 字面（层①须清零触点）；AC-3 When① grep 仅匹配下划线形式，查不出该残留，FR-3 验收标准不完备 | 2.5 节证据块（五文件行号）+2.1 节命令比对；AC-5 层①可兜底但不能替代 FR-3 自洽 | 中（不影响行为正确性，但直接影响 DA 触点清单完整性与 GATE3 残留验收） | 见 §4 修改清单第 1、2 条 |
| QA-2 | FR-2「对外 API 面（现状核实，消费方实际使用的全集）」中 4 个常量（MAX_FILE_BYTES/KEEP_FILES/MAX_EVENT_BYTES/QUEUE_EVENTS）经全仓 grep 核实**无任何消费方引用**，非「消费方实际使用」 | `grep -rn "velox_log::MAX\|velox_log::KEEP\|velox_log::QUEUE" --include="*.rs"` 零命中 | 低（保护意图合理——公开 API 等价性；AC-2 判定方式客观不受影响） | 见 §4 修改清单第 3 条 |
| QA-3 | 提示级：OP-4 默认方案含「新增 ADR」，§8.2 文档清单未列该新增文件产出项 | OP-4 默认方案 vs §8.2 仅列 0002 处置 | 提示（OP 未仲裁前不落清单可接受） | GATE2 若采纳 OP-4 默认，影响面须补新增 ADR 文件项；本轮不必改 |

## 4. SA 修改清单（REWORK 放行条件）

1. **§8.1 补触点**：表中补一行——五包 `config.rs` 注释 5 处「见 velox-log §4.4」连字符字面：`velox-config/src/config.rs:101`、`velox-registry/src/config.rs:129`、`velox-mesh/src/config.rs:103`、`velox-demo/provider/src/config.rs:132`、`velox-demo/consumer/src/config.rs:127`，归属 FR-5 层①清零范围。
2. **AC-3 判定命令完备化**：When① 的 grep 模式由 `"velox_log"` 扩为 `"velox-log\|velox_log"`（或显式注明与 AC-5 层①合并执行同一命令），确保注释形式残留可被 FR-3 自身验收捕获。
3. **FR-2 措辞修正**：「消费方实际使用的全集」改为「消费方实际使用的 API 全集 + crate 公开常量 4 项」，消除常量无消费方引用的表述与实测矛盾；保护面（6 API+4 常量等价）不变。
4. （QA-3 为提示级，不作为本轮放行条件。）

## 5. 复审说明

SA 按上述清单修订 01-requirements.md（v2）后重派 F-GATE1 复审；复审范围以本轮问题清单与修改清单为焦点，其余部分不重复全量评审。返工预算：本轮记 1/2。

## 6. 复审记录（v2 返工轮，GATE1 复审）

> 复审对象：01-requirements.md **v2**（227 行，SHA256 前 16 位 `161566e203fc795a`），基于合并后基线 develop @ `638f5eb`。
> 复审口径：按 §5 约定，以首轮问题清单与 §4 修改清单为焦点，不重复全量评审。
> 角色说明：返工修订由本会话按评审清单代执行 SA 职责（3 项均为机械修订、无设计自由度），复审独立逐项核对。

### 6.1 修改清单逐项核对

| # | 修改项 | 核对结果 | 证据 |
|---|---|---|---|
| 1 | §8.1 补五包注释 5 处连字符触点 | ✅ 通过 | v2 §8.1 新增行（v2:204）：`velox-config/src/config.rs:119`、`velox-registry/src/config.rs:129`、`velox-mesh/src/config.rs:103`、`velox-demo/provider/src/config.rs:132`、`velox-demo/consumer/src/config.rs:127`，与 `638f5eb` 实测逐一吻合（config.rs 注释触点 101→119 系 e1a8738 合并插入 cache 覆盖块所致，已同步） |
| 2 | AC-3 判定命令完备化 | ✅ 通过 | v2 AC-3 When①（v2:124）扩为 `grep -rn "velox-log\|velox_log" --include="*.rs" …`；Then①（v2:125）同步声明「零 velox-log/velox_log 残留（连字符注释字面在检查面内）」，FR-3 自洽性恢复 |
| 3 | FR-2 措辞修正（QA-2） | ✅ 通过 | v2 FR-2（v2:33）改为「消费方实际使用的 API 全集 + crate 公开常量 4 项」，与全仓 grep 实测（常量消费方零引用）一致；保护面（6 API+4 常量等价）不变 |

### 6.2 基线同步核对（超清单修订，逐项复核）

返工修订同时将 v1 基线事实同步至合并后现状（`e1a8738`「velox-config SQLite 存储迁移」并入所致），逐项实测通过：

| 项 | v1 | v2 | 实测 |
|---|---|---|---|
| 基线 | develop @ `cf75e33`，limit.rs 在途改动 | develop @ `638f5eb`（合并提交），limit.rs 在途改动已消解 | ✅ git 实况一致；stash 冗余性经两 hunk 比对确认后清理 |
| NFR-3 limit.rs 条目 | 「既有 rustfmt 改动原样保留」 | 「不触碰 limit.rs（改动已随合并消解），提交 diff 仍须不含该文件」 | ✅ 纪律语义保持，事实更新 |
| velox-config 代码锚点 | config.rs:4,106-107；main.rs:35 | config.rs:4,**124-125**；main.rs:**36** | ✅ 实测命中（124-125/36） |
| Cargo.lock 条目行号 | 181/200/1537/1541/1567/1585 | 181/200/**1626/1630/1656/1674** | ✅ grep 实测命中（181/200 未变） |
| subsystems.md 锚点 | :61/:111/:171 | :61/**113**/**173** | ✅ 实测命中（e1a8738 在前文插入 SQLite 内容） |
| 根 Cargo.toml:9/26、adr/0002 六处、development.md:67、velox-log 六文件 3346 行、registry 测试触点 | — | 不变 | ✅ 合并未触及，复核仍成立 |

### 6.3 复审结论

**PASS（GATE1 复审通过）**。修改清单 3 项全部按意见落实且与实测一致；基线同步修订经逐项实测无引入性错误；未发现新的可判定性/遗漏问题。FR/AC 可判定性、边界承接、范围外、OP 默认方案、需求忠实度在 v2 下全部成立。F-GATE1 关口通过（首轮 REWORK 1/2 后复审收口），流水线可推进 F-AA ∥ F-TA-D。

遗留提示（不阻塞）：OP-1~OP-6 留待 GATE2 仲裁；QA-3（OP-4 若采纳默认方案须补「新增 ADR 文件」产出项）在 GATE2 仲裁时处置。
