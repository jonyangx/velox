# 需求关口评审：velox-config 单实例最大并发请求限流

> Feature：`instance-concurrency-limit`；工序：F-GATE1（需求关口，非 GATE2、非终验）。
> 评审人：QA；日期：2026-11-26（本轮为定向复审轮）。
> 基线：`develop @ 28c9114`；实现 `90268ca`、单测 `dff2006`、规格 `46e928a` 的提交与内容已只读核对。提交存在不代表任何关口已通过。
> 本轮只更新本评审文件；不改代码、需求、架构、用例、状态或其他 feature 工件。未执行测试、构建、服务启动或 panic 复现。
> 引用：R=`01-requirements.md`；A=`02-architecture.md`（辅助、未在本轮评审）；T=`03-test-cases.md`，均位于本目录。代码路径相对项目根。

---

## GATE1 复审（2026-11-26）：**PASS**

> 本轮核验 SA 是否按前轮 GATE1 REWORK 的 G1-1～G1-5 定向返工清单完成回填；下文 §R 为本轮核对，§H 为前轮 REWORK 意见存档（2026-11-26 早间，未删改）。

### R0. 结论与依据

**结论：PASS（G1-1～G1-5 全部闭合，需求可冻结）。** SA 已于 2026-11-26 10:21 回填头部自述"GATE1 定向返工已回填，待 GATE1 复审"；逐项核验五条返工要求全部落实且内容正确，改动范围与前轮驳回契约严格一致，未越限；修订条目满足 GATE1 五项标准，代码现状声明经只读抽查全部吻合（含本机锁定 Tokio/Tower 源码契约）。

本轮输入与指纹（SHA-256）：

- R（当前，回填后）：`abcf44b10fa4890c24cc053db1229ce5321bd96859bedbecaf21fa7a2b48c0e3`
- 前轮 REWORK 契约（本文件 §H 存档版本）：`d758f56c92e6ae782957d161122bd9cbce8812c2c89315b2b23545753ea92e19`（其 §9 记录的前轮 R 基线 SHA `82a13d5c…` 与本轮 before 基线完全一致，证明对照基线无误）
- before 基线 `/tmp/dev-pipeline-cli.jHGxMGJH/instance-concurrency-limit-01-requirements.before.md`：`82a13d5caee6b9b2dbe422817812d5f9f899f0c6737a839bd403d2ee48d8c9cb`
- SA 落盘 diff 证据 `instance-concurrency-limit-requirements.diff`：`489da7521a68b86e506d5028f3fe6a7e4420dbe0545bf59a13b2513e3594bbf3`

SA 未输出本轮完成标记（上游 CLI 限流中断），按工件事实以当前文件为准确认回填完成，进入复审。本轮 PASS 仅表示**需求文字冻结**，不代表实现缺口已修复、架构关口已通过或任何 AC 已通过。

### R1. 越限核对：当前文件 = before + 恰好 G1-1～5 授权范围

对 before 基线与当前文件做全文 diff（独立复核），全部差异 hunk 落点逐一映射到五条授权：

| 授权条目 | 授权的修改对象 | 实际差异落点（R 当前文件） |
| --- | --- | --- |
| G1-1 | NFR-2、AC-5、AC-7 日志观察前提 | NFR-2 :93-96；AC-5 :142-152；AC-7 :165-169 |
| G1-2 | FR-6、AC-6、§5 极大值、Q6、§8 配置校验说明 | FR-6 :75-82；AC-6 :156-161；§5 :179；§7 Q6 :208；§8 :216/:225 |
| G1-3 | FR-5、FR-6 空 env 说明；必要时 AC-5 补分支 | FR-5 :69-71；FR-4 :64（env>文件>默认与空 env 不覆盖语句）；AC-5 情况 5/6/7 :147-149 |
| G1-4 | FR-2、FR-3、AC-3、§5 healthz 边界、§7 Q1～Q5 固化 | FR-2 :34；FR-3 :40/:47；AC-3 :129-130；§5 :183；§7 :197-208 全表 |
| G1-5 | §1 历史背景、§8 影响面与 Tower 提示纠正 | §1 :8/:14-16（90268ca 之前的历史问题 + 当前基线段）；§8 :212-225 全表与实现提示 |

**未越限确认**：FR-1、NFR-1/NFR-3/NFR-4、AC-1/AC-2/AC-4、§5 其余行、§6 范围外均与 before 逐字一致；未新增 AC 编号、未改其他 feature 规格。头部状态行 :4 的更新属于文档自身状态追踪（反映 §7 已回填），非 FR/AC 语义变更。before 基线两份副本经 cmp 完全相同，当前文件与 diff 证据 after 侧逐行吻合——SA 自证与工件事实一致。

### R2. G1-1～G1-5 逐项闭合核验

**G1-1（NFR-2、AC-5、AC-7 日志观察前提）——闭合。** 前轮 RQ-1 的四点要求逐项落位：

1. 启动生效上限仍记录 INFO、显式开启 info 且过滤器允许对应 target 时核验（可隔离 `RUST_LOG=info`）→ NFR-2 :93；AC-7 :165-166；
2. AC-5 容量期望 10/10/3/3 保留，日志作证据时须显式 info，默认 warn 靠配置+受控饱和行为证明，INFO 行缺失不判配置失效 → AC-5 :151-152；
3. AC-7 区分"INFO 启动记录"与"默认 warn 下每拒一条 WARN（含上限与被拒路径）"，显式 error/off 或 target 过滤时不可见属过滤预期，事件生成义务不变 → AC-7 :167-169；NFR-2 :95；
4. 配置/初始化失败的 stderr 错误不依赖 INFO 启用 → FR-6 :80、NFR-2 :96、AC-7 :169。
另：NFR-2 :94 明确日志观察遵循 addlog 的 `RUST_LOG > LOG__LEVEL > 文件 > warn` 优先级并要求隔离残留高优先级 RUST_LOG——与 addlog 本轮冻结的 FR-4 契约一致（跨 feature 对齐核验通过；addlog 定向复审亦为 PASS）。

**G1-2（FR-6、AC-6、§5 极大值、Q6、§8 技术合法域）——闭合。** 前轮 RQ-2 要求逐项落位：

- 合法域整数 1..=U，`U = min(i64::MAX, usize::MAX, tokio::sync::Semaphore::MAX_PERMITS)`，不另设业务封顶、不隐式截断 → FR-6 :75、Q6 :208、§5 :179；
- 无损比较、不得先 `as usize` 再校验；超 i64 解析域输入带原值报错、不进入转换链 → FR-6 :76；
- 超界为普通可读配置错误，构造限流器/开始监听前终止、真实非 0 退出；错误含配置项名/env 全名 + 原始输入 + 适用上界 U → FR-6 :77；AC-6 :158；
- 不截断/取模/饱和/静默默认/不以未捕获 panic 代替 fail-fast；合法值须能安全构造容量一致的限流器 → FR-6 :78；AC-6 :160；
- 源码契约只读复核：`~/.cargo/registry/.../tokio-1.54.1/src/sync/semaphore.rs:479~485`（`MAX_PERMITS = usize::MAX >> 3`、`new` 超界 panic）与 `batch_semaphore.rs:137/149-151` 断言一致；64 位 U=2305843009213693951 < i64::MAX，32 位 U=536870911，故 R :159"当前锁定 Tokio 的 32/64 位平台均超 U，须普通配置错误"的整数判断成立；
- AC-6 覆盖 U+1、超 i64 解析域原值、U 本身合法（仅核对构造/容量，不发起 U 个请求）、空 env 不纳入非法集合 → :156-161，可判定且可执行。

**G1-3（FR-5/FR-6 空 env 与文件优先级；AC-5 补分支）——闭合。** FR-5 :67-71 改为"所有来源均未给值时为 10"，明列 文件5+未设 env=5、文件5+空 env=5、非空合法 env 覆盖（文件5+env3=3、无文件+env3=3）；FR-4 :64"非空且合法的 env > 文件显式值 > 默认 10；env 未设置或空串不覆盖文件；非空非法 env 必须报错，不得回落"；FR-6 :79"空串不是非法值，沿用 env_override 过滤语义"。AC-5 补齐情况 5/6/7（文件5+无env=5、文件5+空env=5、无文件/缺键+空env=10），并保留原四种容量期望 10/10/3/3——与前轮要求"保持现有实现及 T TC-09 的正确行为"一致（`config.rs:95-97` 的 `filter(|v| !v.is_empty())` 空值过滤语义实查吻合）。

**G1-4（固化 healthz 及 Q1～Q5 默认决议）——闭合。** FR-2 :34 healthz 豁免定案（不占名额、饱和时仍 200、不得为探针放宽上限）；FR-3 :40/:47 固定 `code="too_many_requests"` 与 `Retry-After: 1`（并注明仅为重试提示、message 不要求逐字一致），移除"若 Q3 另有决策"悬空文字；AC-3 :129-130 补响应头断言并固定 code 值；§5 :183 healthz 边界定案；§7 :197-208 六项全部改为"本轮采纳的决议与落点"，注明"未决 0 项、不是留给 AA/DA 的开放问题"。Q1～Q5 采纳值与前轮 §5 裁决逐条一致，Q6 为限定的技术容量裁决。

**G1-5（§1 历史背景、§8 影响面与 Tower 提示）——闭合。** §1 :8 注明"以下为实现提交 90268ca 之前的历史问题"；:16 新增"当前基线"段，列明 develop 已有实现（limit.rs 共享 `Arc<Semaphore>` + `try_acquire_owned`、配置字段、业务路由装配、healthz 豁免、429 错误体、启动上限字段）与真实缺口（config.rs 仅校验下界 + `as usize`、日志缺省 info 须对齐 addlog warn）；§8 :212-225 改为"已有事实及后续影响"，不再把已交付项描述为待新增；实现提示 :225 纠正为"Tower `ConcurrencyLimitLayer` 单独使用会在 `poll_ready` 等待名额，不会天然产生 429"，并注明不切换该层。只读复核：

- `velox-config/src/limit.rs:14/:20-22/:40/:44-48`：共享信号量、`try_acquire_owned` 非等待入口、RAII 归还、每拒 WARN（含 limit/path）——声明吻合；
- `velox-config/src/api.rs:16-19/:27-28/:39`：`route_layer` 仅包业务路由、healthz 注册在层外、`router(store,max)`——声明吻合；
- `velox-config/src/error.rs:22-37`：TooManyRequests → 429 + `Retry-After: 1` + JSON code/message（message 含上限）——声明吻合；
- `velox-config/src/main.rs:30-35`（缺省 info）/:46-52（INFO 含 `max_concurrent_requests` 字段）——声明吻合；
- `velox-config/src/config.rs:12/:23/:67-73/:79-87/:90-92`：默认 10 i64、serde 显式默认函数、env 覆盖（空值过滤）、validate 仅 `< 1`、访问器 `as usize`——"当前真实缺口"描述准确；
- `velox-config/Cargo.toml:21-23`：uuid/tower 为 dev-dependencies——"不要求新增运行时 tower"成立；
- `velox-config/config/velox-config.toml:5`、`velox-config/README.md:40-49`：示例 10、env 全名、429/Retry-After/行为收紧/healthz 豁免说明已在——"后续只对齐合法技术域、空 env、需重启与日志核验前提"定位准确；
- `velox-config/src/store.rs:8/:36/:152-163`：`flush_lock` 串行落盘 + `spawn_blocking`——§1 背景引用有据；
- Tower 提示源码依据复核：`~/.cargo/registry/.../tower-0.5.3/src/limit/concurrency/service.rs:56-78`（`poll_ready` 经 `poll_acquire` 等待名额、`type Error = S::Error` 不产生 429）——纠正后的表述与上游事实一致。

### R3. 修订条目 GATE1 标准复核

| 标准 | 结论 |
| --- | --- |
| FR/AC 可判定真伪 | 通过。AC-5 七种启动方式的期望上限逐一给定并保留行为验证（N+1 个请求 429/释放后再受理）；AC-6 的非法/合法集合与错误内容、退出时机、无 panic 均可观测；AC-7 三段 Given/Then 区分 INFO/WARN/过滤预期；NFR-2 各条给出可见性判定前提 |
| 边界与异常覆盖 | 通过。§5 满额/500 释放/优雅关停/竞态保守/healthz 超时豁免/极大值与技术上界/TOML 语法错误各行齐备；AC-6 覆盖 U+1、超解析域、U 本身、空 env 排除 |
| 范围外明确 | §6 未动且继续有效（分布式、速率、排队、差异配额、热更新、指标体系均排除）；NFR-2 明确"不增加诊断端点或修改 addlog 默认等级"，防止跨 feature 范围蔓延 |
| 待确认问题均附决议 | §7 六项全部由"待确认"改为"已采纳决议与落点"，无悬空选项、无"以未来决定为准"字样；未新增开放问题 |
| 影响面准确 | §8 与代码实查逐项吻合（R2 证据表）；实现提示纠正后与 tower-0.5.3 源码一致；明确"不得以规格返工为由机械重写已实现代码" |

### R4. 问题清单（本轮）

| 编号 | 描述 | 证据 | 级别 | 处置建议 |
| --- | --- | --- | --- | --- |
| RQ-B-1 | FR-4/FR-6 未显式写明"文件非法值 + 非空合法 env 并存"的组合行为（按优先级链文义应选中 env 值、文件值被覆盖不触发校验，与现有 addr 语义及实现一致），且 AC 未覆盖该组合 | R :64/:75-80；`config.rs:61-74` 先 env 覆盖再 validate 最终值 | P3（不阻断） | 文义可推导且与既有语义一致，不构成本轮驳回条件；建议 TA 在用例回填时补一条该组合用例以固化行为 |
| RQ-B-2 | §8 引用 `velox-config/README.md:40` 作锚点，实际 429/Retry-After/healthz/行为收紧说明位于 :44-49（:40-46 为配置表） | README 实查 | P3（锚点偏移，内容属实） | 后续文档同步时顺带校正行号即可 |

无其他问题；无 REWORK 修改清单。

### R5. 后续工序的硬约束（本 PASS 不解除）

1. **实现缺口仍是真实缺口**：`config.rs:79-91` 的下界校验 + `as usize` 不满足 FR-6，AA/DA 须按 §8"最小修复"补技术上界与无损转换；不得因需求 PASS 而宣称缺陷已修复。
2. TA 须重核 7 条 AC 的用例矩阵：TC-08/09 按七情况与空 env、TC-10～12 按技术上界与无 panic 期望、TC-13～15 按 NFR-2/AC-5/AC-7 的日志观察前提；前轮 §H8 指出的 `limit.rs:159/170` shared_counter 静态疑点仍须独立核验，405 行为不得当已复现缺陷。
3. AA 的 02-architecture 与本需求的新增决议（1..=U、空 env、healthz 定案、日志前提）须对齐，其页首 FR-1～FR-7 计数、§2/§9 过期表述仍待修订（前轮 §H8 已列）。
4. 最终行为对齐 addlog 默认 warn 裁决由 addlog feature 承担，本 feature 不得改动日志默认等级或诊断端点。

### R6. 本轮取证记录

只读手段：git status/log、before vs 当前全文 diff、cmp 基线一致性、六个源文件与 config/README/Cargo.toml 实查、`~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/` 下 `tokio-1.54.1/src/sync/semaphore.rs`、`sync/batch_semaphore.rs`、`tower-0.5.3/src/limit/concurrency/service.rs` 静态核对。未运行 cargo/测试/服务、未复现 panic、未计算/修改任何代码；除本文件外未改动任何文件。本轮 PASS 不生成测试通过统计、不写验收报告、不执行 GATE3 内容。

复审执行附记（2026-11-26，dispatch `task_50b46887ff63` 独立复核后维持结论）：SHA-256 指纹重算与 R0 一致（当前 R `abcf44b1…`、HEAD 基线 `82a13d5c…`）；`git diff HEAD` 全部差异 hunk 复核仍仅落 G1-1～5 授权范围（FR-1、NFR-1/3/4、AC-1/2/4、§5 其余行、§6 均未动）；`config.rs`（:12 默认 i64 10、:65-71 env 覆盖、:77-84 仅 `< 1` 校验、:88-90 `as usize`）、`limit.rs`（:14 共享信号量、:22 `Semaphore::new`、:40 `try_acquire_owned`、:44-48 每拒 WARN）、`api.rs`（:20 `limit_routes` 经 :24 `route_layer` 仅包业务路由、:28 `router(store,max)`、:40 healthz 注册层外）、`error.rs`（:22-37 429 + `Retry-After: 1` + code/message 含上限）、`main.rs`（:30-35 缺省 info、:47-52 INFO 含上限字段、load 先于限流器构造/监听）锚点实查与 R 声明一致；`tokio-1.54.1`（`MAX_PERMITS = usize::MAX >> 3`、`new` 超界 assert panic）与 `tower-0.5.3`（`poll_ready` 经 `ready!(poll_acquire)` 等待名额、`type Error = S::Error`）源码契约再读一致，Cargo.lock 锁定 tokio 1.54.1 / tower 0.5.3 核对无误；U 的 32/64 位整数判断（2305843009213693951 / 536870911 均小于 i64::MAX）复算成立。02-architecture 工作树仅 1 行措辞改动（"CLI 参数"→"启动参数"）与本需求无冲突；其页首 FR-1~FR-7 计数、§9 过期表述及空 env 措辞粗化仍属 R5.3/H8 已记录的下游 AA 对齐义务，不构成本轮冲突。03-test-cases.md v1（21TC/7AC/2RQ）未动，页首自述待 AC 冻结后重核矩阵，TC-03/08/09 与冻结决议一致。另记 P3：R :199 引"评审文件 §5～§7 裁决"，该裁决原文现存档为 §H H5～H7（内容未变），后续文档同步时顺带校正指引即可；RQ-B-1/RQ-B-2 维持 P3 备注，无新增缺陷，结论维持 PASS。

### R7. 历史脉络

- **前轮 GATE1（2026-11-26 早间）：REWORK**，G1-1～G1-5 定向返工清单 + RQ-1/RQ-2 裁决原文见 §H 存档。要点：日志观察前提与 addlog warn 冲突（G1-1）、技术容量上界欠缺（G1-2）、FR-5/6 空 env 文字矛盾（G1-3）、待确认项未固化（G1-4）、历史背景与 Tower 提示失实（G1-5）。
- **本轮（2026-11-26）：定向复审 PASS。** 五条返工全部闭合、未越限、标准复核通过；R :16"当前基线"段与本轮 R2/R5 的代码实查互为印证。需求文件可冻结进入 AA/TA 对齐与最小实现修复；R :4 的自我限定（"本轮仅修需求，不代表关口通过或代码缺口已修复"）表述准确，予以确认。

---

## H. 历史轮次存档：前轮 GATE1 评审意见（2026-11-26，结论：REWORK）

> 以下为前轮意见原文存档（对应需求文件 SHA `82a13d5c…`，即本轮 before 基线）。行号/引用以当时版本为准，其 REWORK 结论已被本轮复审取代。

### H1. 结论：REWORK（驳回）

默认 10、进程级共享、满额立即 429、拒绝不进入业务 handler 的目标清楚，现有 Semaphore/try_acquire 中间件应保留。需求尚不能冻结：**启动日志观察前提与 addlog 默认 warn 裁决冲突；"不封顶"未区分技术容量上界；默认值/空 env 的文字与文件优先级存在矛盾；现状描述及实现提示有失实。**

接受 Q1 的 healthz 豁免建议，并接受 Q2～Q5 的建议默认方案；Q6 裁定为"不设额外业务封顶，但受目标平台技术容量上界约束"。这些关口裁决须由 SA 按第 7 节写回需求；**本评审不是 SA 冻结文档的替代品**。现有 AC 的观察前提和边界必须修改，按任务规则本轮不能 PASS。

### H2. 五项检查总表

| 检查项 | 结论 | 证据/理由 |
|---|---|---|
| FR/AC 可判定性 | 主体可判定，局部须修 | FR-1～4、AC-1～4 有明确共享/拒绝/响应/副作用判据；FR-5/6 的配置域及 AC-5/7 日志前提未闭合 |
| 边界/异常 | 主体覆盖，技术上界欠缺 | R §5 已列 N 满额、500 释放、关停、竞态；超容量须在配置阶段报错，不能靠构造 panic；空 env 应保留文件值 |
| 范围外 | 清楚，需维持 | R §6 排除速率/排队/分布式/差异配额/热更新/指标；本 feature 仅 config 与其文档 |
| 待确认问题默认建议 | 六项都有建议；五项可采纳，一项需限定 | Q1～5 有可实施建议；Q6 无技术边界的"不封顶"不可原样冻结 |
| 影响面与代码现状 | 历史基线须标明，实际实现大体吻合目标 | 当前已有 limit.rs、server 字段、路由装配、429 类型、启动字段和 tower dev-dep；R §1/§8 仍称当前无限流且未依赖 tower；ConcurrencyLimitLayer"直接拒绝"说法不成立 |

### H3. FR、NFR 与 AC 逐条核对（前轮）

FR-1～FR-4 可判定；FR-5 文字矛盾（"env 未设置"不能覆盖文件=5）；FR-6 缺技术上界与空 env 语义须改。AC-1～AC-4 可测；AC-5 须改日志前提；AC-6 须冻结技术边界；AC-7 须区分 INFO 事件与过滤可见性。矩阵 7 条计划覆盖不等于 AC 已通过。

### H4. 影响面与只读代码抽查（前轮）

`config.rs:12/:23`（常量 10、i64 字段显式默认）、`:67/:74/:95`（env 优先、空串过滤）、`:79/:90`（仅下界 + as usize）、`limit.rs:14/:20/:40/:44`（共享信号量、try_acquire_owned、每拒 WARN）、`api.rs:27`（业务全局共享、healthz 层外）、`error.rs:23`（429 + Retry-After:1）、`main.rs:30/:46`（缺省 info、INFO 含上限）、`store.rs:101/:164`（flush_lock/spawn_blocking）、`Cargo.toml:22`（uuid/tower dev-dep）、config toml 与 README 现状——均与前轮判定一致，本轮 R2 复核未发现回归。前轮指出 R §1"当前对所有请求不设上限"应标为 90268ca 之前、R:208 Tower 提示失实（tower-0.5.3 `concurrency/service.rs` poll_ready 等待名额、`Error = S::Error`）——本轮已修复（R2 G1-5）。

### H5. Q1～Q6 逐项裁决（前轮）

Q1 healthz **豁免**；Q2 **[server].max_concurrent_requests**；Q3 **too_many_requests + Retry-After:1**；Q4 **fail-fast**（含技术超界）；Q5 **接受默认 10 收紧**；Q6 **不设额外业务封顶、受技术容量约束 1..=U**。六项均须 SA 写回——本轮 R2 G1-4/G1-2 核验已全部写回。

### H6. RQ-1/RQ-2 裁决原文要点（前轮）

- **RQ-1**：启动上限仍 INFO，显式 info 且允许 target 时核验（可隔离 RUST_LOG=info）；AC-5 容量期望 10/10/3/3 保留，默认 warn 靠配置+饱和行为证明；AC-7 区分 INFO 记录与每拒 WARN；配置/初始化错误不依赖 INFO 启用；不修改 addlog、不加诊断端点。
- **RQ-2**：合法域 1..=U（`min(i64::MAX, usize::MAX, Semaphore::MAX_PERMITS)`）；超界配置阶段普通错误、非 0 退出、构造/监听前终止；错误含字段名/原值/上界；不截断/饱和/静默默认/不以 panic 代替；超 i64 解析域输入带原值报错；上界验收只核对构造与容量。

### H7. 前轮定向返工清单 G1-1～G1-5

| 编号 / 级别 | 修改具体条目 | 必须写回的内容与理由 |
|---|---|---|
| G1-1，中 | **NFR-2、AC-5、AC-7** | 按 RQ-1 区分 INFO 事件与过滤可见性；日志核验显式 info，默认 warn 静默属预期，每拒 WARN 不变。原容量及错误行为不降低 |
| G1-2，高 | **FR-6、AC-6、§5 极大值、Q6、§8 配置校验** | 按 RQ-2 写合法技术域与超界普通错误退出、字段/原值/上界；不把 panic 视为正确启动失败，不设额外业务封顶 |
| G1-3，中 | **FR-5、FR-6 空 env 说明；必要时 AC-5 补对应分支** | 改为"有效 env > 文件 > 默认 10；env 未设/空串不覆盖文件"。明确文件5+无env=5、文件5+空env=5、所有来源未给值=10；空串不是非法配置。保持现有实现及 T TC-09 的正确行为 |
| G1-4，低，冻结待确认项 | **FR-2、FR-3、AC-3、§5 healthz 边界、§7 Q1～Q5** | 写入本关口采纳值：healthz 豁免、server 归属、too_many_requests/Retry-After:1、fail-fast、接受默认 10 收紧。移除互斥选项和"以未来决定为准"，不新增 AC 编号或重写其他 AC |
| G1-5，低，准确性修正 | **§1 历史背景、§8 影响面与实现提示** | 将无并发上限注明为 90268ca 之前的背景；列出现有 limit.rs、已实现字段/路由/日志/dev-dep 与单测；删除"ConcurrencyLimitLayer 直接满足超限即拒绝"的错误提示。保留现有实现与验证资产，只针对真实缺陷修复 |

### H8. 前轮对 AA/TA 的关注项（仍有效，未构成本轮结论）

AA：架构文档页首 FR-1～FR-7 计数、§2 现状过期、§9"Semaphore::new 无上限校验"失实，须对齐 RQ-1/2 与空 env 语义；"405 绕过限流"须独立核验。TA：`limit.rs:159/160` 仅注册 GET /a、PUT /b 而 :170 用 GET /b 断言 429 的静态疑点须改为命中方法；TC-03/05/18 按 Q1/Q3、TC-08/09 按配置来源、TC-10～12 补技术上界且不以 panic 为期望、TC-13～15 按日志前提重核矩阵；测试环境变量修改应恢复原值。范围：不改其他服务/Store/日志默认等级；终验遵守统一全量回归与真正 e2e 留证。

### H9. 前轮输入指纹

R：`82a13d5caee6b9b2dbe422817812d5f9f899f0c6737a839bd403d2ee48d8c9cb`；A：`b19be2b67c96ae9306a57f1f94a13c93dce99e92c24be4811e9c50a14aa09663`；T：`7c96a261966c76d1698e671574f75eab8006e5ea88d61b4179a21b73bdbddf4e`；addlog 需求：`246132785fc544a667b8cc4f465832384f572d488922d66ebb6c8ceb94572fb4`；addlog GATE2 评审：`c1db832dfffd8b832440bac166a308c52db0642ca56c95fee32320e8b2fe1e70`。

---

PIPELINE instance-concurrency-limit GATE1 PASS
