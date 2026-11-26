# GATE2 架构评审意见：instance-concurrency-limit（velox-config 单实例最大并发请求限流）

> 工序：F-GATE2（批量两 slug 之一），Task ID：QA-1795666025105-ff31c521c7fe3348；评审人：QA；日期：2026-11-26
> 性质：**GATE2 首次评审**（架构关口，非终验、非 GATE3；不写 05-acceptance、不执行 AC 满足度核对）。只读评审，除新建本文件外未改动任何文件；未运行构建/测试/服务。
> 被审对象与指纹（动手前重算 SHA-256，全部与锁定值一致）：
> - `02-architecture.md`（AA 定稿，303 行）：`27cba700c619c0355ddc9f417a60c45f58b5047d808265056c098d98157cbf93`
> - `01-requirements.md`（冻结，228 行）：`abcf44b10fa4890c24cc053db1229ce5321bd96859bedbecaf21fa7a2b48c0e3`
> - `03-test-cases.md`（TA v1 定稿，136 行）：`e48455f6028fb87a516a67b291ad00672e30f8106f6d814c9fe93dc0f10a36e9`
> - `01-requirements-review.md`（GATE1 复审 PASS 11:08）：`cf74ad66e17b32a008ff5902cd3c825e7caf70b976e9b3f2bc12f78137f18181`
> 分支基线：develop @ af28e65（AA 自述代码基线 28c9114；本核对基于当前工作树，其中 `velox-config/src/{config.rs,limit.rs}` 有未提交修改——经 `git diff` 逐 hunk 复核为**纯 rustfmt 测试代码换行格式化，无功能改动**，按任务要求保留不评判）。

## 1. 结论：**PASS**

本 feature 已有实现 90268ca/dff2006，AA 定稿遵循"保留既有实现与验证资产，仅修真实缺口"的冻结硬约束：唯一行为级修改为 `config.rs validate()` 补 1..=U 无损上界（u128 比较），其余为 api.rs 失实注释修正、toml/README 文字对齐与测试补充。逐 FR 覆盖与可实施性通过、与代码现状逐行吻合、备选取舍成立、7 条 AC 均有实现路径可测、TA §5 差异清单 5 项全部在 AA 定稿闭合（仲裁见 §5）、AA 三条风险提示衔接核验通过。**无阻断项**；仅 3 项 P3 备注（§7），可交 DA 实施。

## 2. 逐 FR 设计覆盖与可实施性

| FR/NFR | AA 落点 | 核验结论 |
| --- | --- | --- |
| FR-1 进程级全局上限（默认 10） | §1 映射 → limit.rs 既有共享 `Arc<Semaphore>` | 通过。`limit.rs:14-30` 实查吻合（Clone 共享、max 缓存），保留原则正确 |
| FR-2 in-flight 口径、覆盖业务路由、healthz 豁免 | §1/§4.2/§4.4；`route_layer` 仅挂业务路由；permit 持有至响应构造完成；404/405 边界见 §2.3.2 定论 | 通过。`limit.rs:35-52` guard 语义、`api.rs:19-41` 装配（healthz :39 层外）实查吻合 |
| FR-3 超限立即 429+统一错误体 | §4.3 保留 error.rs | 通过。`error.rs:22-38` 429+`Retry-After: 1`+code/message 含上限实查吻合，零改动成立 |
| FR-4 `[server]` 归属、env 覆盖优先 | §4.1 保持既有字段/env 块 | 通过。`config.rs:23-24`（serde 显式默认函数+陷阱注释）、`:67-73`（env parse 报错含全名与原值）实查吻合 |
| FR-5 env>文件>默认 10；空 env 不覆盖 | §3 链路图+§4.1"保持不动"声明 | 通过。`config.rs:95-97` `env_override` 空串过滤实查吻合；§4.1 明确"文件5+空env=5，非回落 10"，与冻结 G1-3 一致 |
| FR-6 合法域 1..=U、无损比较、fail-fast | §4.1 **唯一行为修改点**：`MAX_CONCURRENT_REQUESTS_CAP` u128 常量+validate 替换代码+访问器不变式 | 通过。常量公式为冻结 Q6 直接转写；`i64::MAX`/负数路径检查次序正确（先 `v<1` 拒负数，再 u128 比较）；MAX_PERMITS 为 tokio 公开 const 可 const 求值（本机源码复核）；"不得先 as usize 再校验"的 32 位截断论证成立 |
| NFR-1~4 | §1 映射+§4.5 日志观察论证+§5 迁移 | 通过。try_acquire O(1)、RAII 不超卖、发布说明义务与 README 对齐齐备 |

可实施性：DA 拿到无须临场决策——改动面封闭（§4 开头明列：config.rs 行为修改、api.rs 仅注释、toml/README 文字、测试补充，limit/error/main/store/handlers **零改动**），validate/访问器替换代码逐行给出，"DA 不要顺手改"清单（字段类型 i64、env 块、env_empty_falls_back 原名）与理由齐备；加载顺序 文件→env→validate 对 TC-09 组合语义的推导明确。

## 3. 与代码现状匹配（只读抽查，核 90268ca 保留/修改清单精确性）

AA §2.1"已实现资产"与 §2.2"真实缺口"逐项实查（develop @ af28e65 工作树）：

| AA 声明 | 实查证据 | 结论 |
| --- | --- | --- |
| `config.rs:79-87` validate 仅查 `< 1`；`:89-92` 访问器 `as usize`；`:95-97` env_override 过滤空串；`:12-16`/`:23-24` 显式默认 10 i64；`:67-73` env parse 报错含全名原值 | `grep -n` 全部逐行命中（validate fn 恰在 :79，访问器 :90，env_override :95） | ✅ 精确 |
| `limit.rs` guard `try_acquire_owned` 非阻塞、每拒 `warn!(limit, path)`、5 个集成单测 | `limit.rs:35-52`、`:44-48`；5 个测试名逐一存在（third_rejected/shared_counter/reject_body_shape/exempt_route/unmatched_route） | ✅ |
| `api.rs:16-18` doc comment 405 半句失实（G-3） | `api.rs:16` 原文"路由未命中（404/405）不执行中间件"——405 半句确实失实（§4.4 源码证据） | ✅ 缺口真实，列入修改清单 |
| `main.rs` load(:28) 先于 router(:39)/bind(:45)；INFO 含上限字段(:46-52)；缺省 info(:30-35) | 逐行命中；默认 warn 改造归 addlog、本 feature 不动的边界声明正确 | ✅ |
| uuid/tower 为 dev-dependencies、零新增依赖 | `velox-config/Cargo.toml:21-23` 实查 | ✅ |
| tokio 1.54.1 `MAX_PERMITS = usize::MAX >> 3`（semaphore.rs:479 公开常量）、`new` 超界 assert panic（batch_semaphore.rs:137/:149-152）；64 位 U=2^61−1、32 位 U=2^29−1，均 < i64::MAX | 本机 registry 源码重读逐项吻合；`usize::MAX >> 3` 32 位=536870911 复算成立 | ✅ |
| tower 0.5.3 `ConcurrencyLimitLayer` 在 `poll_ready` 经 `poll_acquire` 等待名额、`Error=S::Error` 不产生 429 | `limit/concurrency/service.rs:56-78` 重读吻合 | ✅ |
| Cargo.lock：axum 0.8.9、tokio 1.54.1、tower 0.5.3 | lock :27-28/:1192-1193/:1271-1272 命中 | ✅ |

保留/修改清单精确性：90268ca 资产全部列为保留（limit/error/main/store/handlers/装配），修改仅 config.rs（常量+validate+访问器注释）、api.rs 注释替换文本已给出、toml:5 注释与 README :44-49 文字对齐点明确——与"不机械重写"约束一致，未发现虚构行号或失实现状声明。

## 4. 备选方案取舍

- **A（保留，自研 guard+Semaphore）**：已在库并经 dff2006 验证，`try_acquire` 精确匹配 FR-3"不排队立即拒绝"——采用理由成立。
- **B（tower ConcurrencyLimitLayer）弃选**：`poll_ready` 等待名额、`Error=S::Error` 不产生 429，技术上不可行——本机源码复核证实，与冻结需求 §8 纠正一致，弃选成立。
- **C（AtomicUsize 手写 guard）弃选**：RAII 已由 `OwnedSemaphorePermit` 提供等价并发验证实现，手写增减边界易错——成立。
- **D（usize/u32+deny negatives）弃选**：负数在 serde 层以类型错误失败、报错文案不受控，难满足 AC-6"错误含字段名与实际值"；env 侧 ParseIntError 同样缺上下文——成立。
- **U 上界实现（u128 常量 vs `usize::try_from`+分别比较）**：32 位平台 try_from 失败需拆两种报错文案；u128 常量是冻结公式 `min(...)` 的直接转写且单一常量同时服务比较与错误信息——取舍成立。

## 5. TA 差异清单仲裁（必做项，逐项）

TA 03 §5 末尾差异清单系 TA 完成时 AA 定稿未落盘、针对旧版工作树（TA 头部自证"当前该文件仅 1 行措辞改动"）的记录，明言"属 AA 对齐义务、供 GATE2 仲裁"。逐项对照 AA 定稿（27cba700）：

| # | TA 差异项 | AA 定稿现状 | 仲裁结论 |
| --- | --- | --- | --- |
| 1 | 页首 FR-1~FR-7 计数不一致 | §1 明写"冻结需求共 FR-1～FR-6…（旧稿计数错误，予以纠正）" | **以 AA 为准，已闭合** |
| 2 | Q6 行"`Semaphore::new` 无上限校验"失实 | §9 Q6 行与 §4.1 常量均为 1..=U+MAX_PERMITS 契约，§9 末尾明列该失实为旧稿遗留并记录修正 | **以 AA 为准，已闭合**（tokio 源码复核支持） |
| 3 | "空串回落默认"措辞 | §3 链路图与 §4.1：文件5+空env=5、无文件/缺键才默认 10（G1-3，非"回落 10"） | **以 AA 为准，已闭合**（与冻结 FR-5 及 config.rs:95-97 实现一致） |
| 4 | 404/405 route_layer 语义与既有测试静态分歧 | §2.3.2/§4.4 以 axum 0.8.9 源码证据定论：404 不经层；405 先过层（饱和 429/未饱和短暂占名额后 405）；既有 `shared_counter` 测试预期 429 正确、无需修改 | **以 AA 为准，已闭合**。本轮独立复核本机 axum-0.8.9 源码：`path_router.rs` route_layer 对每注册 endpoint 调 `endpoint.layer`→`mod.rs:757-775` Endpoint::layer 分派 MethodRouter::layer→`method_routing.rs:967-993` 将 layer_fn 应用到**全部方法端点及 `self.fallback`**——405 回退被层包住，"405 请求先过限流层"定论成立；饱和时 guard 先拒（429），与 `limit.rs:167-168` GET /b 断言 429 一致。TA 的"TC-17 独立记录实跑结果"是执行期留痕要求，与 AA"按定论核验未饱和 405/饱和 429 两条路径"兼容，非接口分歧 |
| 5 | 页首 Q1~Q6 若仍标待裁决应更新为冻结决议 | §9 表格化"Q1～Q6 已由 GATE1 关口裁决冻结"，逐项落实落点 | **以 AA 为准，已闭合** |

**仲裁总结**：5 项差异全部已在 AA 定稿闭合，以 AA 定稿为准，均不阻断。TA 侧的保守设计（TC-02 不得以 GET /b 作 AC-2 共享证明、405 行为归 TC-17 独立核验）与 AA §6.2"405 的 AC-2 证明不得使用未命中方法探针"完全一致——双方在冻结语义上无分歧，TA 差异清单自本轮起转为历史记录。

## 6. AA 风险提示三条衔接核验 + 可测性交叉检查

**① TC-10~12 修复前失败预期——如实记录，未掩盖。** AA §6.1 注意段明文"U+1、i64::MAX 用例在修复前会失败（现行代码会走到 Semaphore::new panic 或静默接受），按冻结 AC-6 期望书写、执行期如实记录失败，不降低期望"；§8 风险表同口径；TA §6"执行期预期记录"（TC-10～12 修复前失败、默认 warn 进程级场景 addlog 合并前记「无法验证/依赖未就绪」）与 §2.1"当前实现会让 U+1/i64::MAX 走到构造 panic——待 DA 修复的真实缺口"一致。TA TC-12 前置条件含隔离子进程防 panic 波及。✅

**② §6 合同表出入仲裁**：即 §5 差异清单 5 项，全部闭合，无遗留出入。TA 抽验 3~5 项接口签名（`Config::load(Option<&str>)`、`max_concurrent_requests()->usize`、`ConcurrencyLimit::new/max`、`limit_routes`/`router(store,max)`、`TooManyRequests{limit}`）与代码实查逐一吻合；`available_permits` 容量核对路径（限流模块内测试可触达私有信号量，或饱和 N 后第 N+1 个 429 间接证明）可行。✅

**③ api.rs:16-18 失实注释列入 DA 修改清单**：§2.2 G-3 行 + §4.4 给出替换注释全文（与 §2.3.2 定论一致）且声明"文档修正而非行为改动、无测试需改"——已列入。✅

**可测性（逐 AC 双落位）**：7 条 AC 矩阵（TA §4）主用例+优先级+前提齐备、无 P0/P1 缺口；Q1~Q6 与 G1-1/G1-3 决议均有用例与实现路径双落位——Q1 healthz（TC-05/18 ↔ api.rs:39 层外+exempt_route 测试）；Q2 七情况（TC-08/13 ↔ config.rs 默认函数+env 块）；Q3 错误契约（TC-03/15 ↔ error.rs:27-37）；Q4 fail-fast（TC-10/12 ↔ validate 链+stderr 通道论证 §4.5.5）；G1-3 空 env（TC-08 情况 6/7+TC-09 ↔ env_override:95-97）；Q6 技术域（TC-11/12 ↔ §4.1 常量+validate）。测试基建（门闩+oneshot+ENV_LOCK+uuid 临时文件）与 §2.4 现状声明吻合；夹具 G/E 的确定性手段（started/peak 计数、部分 body 占名额、abort 并 await）可判定、不依赖 sleep 猜测；TC-14"捕获层核验事件生成而非 subscriber 可见性"口径与冻结 G1-1 一致。

## 7. 问题清单（无阻断项）

| 编号 | 描述 | 证据 | 级别 | 责任方/处置 |
| --- | --- | --- | --- | --- |
| P3-B1 | AA 表格行号在 rustfmt 后有 ±3 漂移：`limit.rs:35-55`（实为 guard :35-52）、`limit.rs:149-176`（实为测试 fn :146-175）——内容指向无误（try_acquire/warn!/GET /b 断言均实查命中） | limit.rs 工作树、AA §1/§2.3.2 | P3 | DA/TA 实施时顺带校正 |
| P3-B2 | AA §2.3.2 引 axum 官方测试 `routing/tests/mod.rs:489-513` 为 GitHub 路径，本机 .crate 包未含该测试文件（仅 tests/panic_location.rs）；但 405 定论已由本机源码（method_routing.rs:967-993 fallback 亦被 layer 包裹）独立证实，结论不受影响 | 本地 registry 实查、AA §2.3.2 | P3 | ADR/文档引用随实施留痕即可 |
| P3-B3 | env parse 失败文案"（要求 ≥1 的整数）"对超 i64 解析域输入（30 位数字）语义欠精确（该输入是整数但溢出解析域）；冻结 FR-6 仅要求"带原值明确报错、不进转换链"，现文案已满足冻结文字 | config.rs:68-72、AA §4.1"保持不动"声明 | P3 | 不强制改；如实施时顺带优化文案须保持"含 env 全名+原值" |

以上不构成 GATE2 驳回条件。**无 REWORK 修改清单。**

## 8. 范围红线、冻结项与历史脉络

- **范围红线**：改动仅 config 及其文档（config.rs/api.rs 注释/toml/README/测试），零新增依赖（运行时与 dev 均不加），不动 limit/error/main/store/handlers，不动日志默认等级（归 addlog，§4.5/R5.4 边界一致），不改其他 feature 工件——未越线。
- **冻结阈值**：默认 10、Q1 豁免、Q2 归属、Q3 code/Retry-After、Q4 fail-fast、Q5 收紧声明、Q6 合法域 1..=U——AA §9 逐项落实，无偏离、无新增开放问题（§9 末自证"无新增开放问题，DA 拿到后不需要再做架构决策"，经本轮核验成立）。
- **旧稿遗留差异闭合记录**（AA §9 末 4 项）：FR 计数/Q6 失实/空 env 措辞/405 失实——本轮逐项核验均已在定稿对应节修正（§2/§3/§4.4/§9）。
- 历史脉络：首轮 GATE1 REWORK（G1-1~5，§H 存档）→ GATE1 定向复审 PASS（11:08，需求冻结，RQ-B-1/RQ-B-2 为 P3）→ AA 重写定稿（27cba700）与 TA v1（e48455f6）同波完成 → 本轮 GATE2 **PASS**。GATE1 复审 §R5 硬约束不因本 PASS 解除：实现缺口仍须 DA 按 §4.1 修复、TC-10~12 修复前如实记录失败、默认 warn 对齐归 addlog。
- 本轮取证手段：SHA-256 重算锁定 4 份输入；完整阅读 01/02/03/GATE1 评审；`git diff` 复核 config.rs/limit.rs 工作树修改为纯 rustfmt（测试代码换行，无功能改动）；只读实查 velox-config 六源文件、根与 crate Cargo.toml、Cargo.lock、config toml、README 锚点；静态重读本机 `axum-0.8.9`（path_router.rs/method_routing.rs:967-993/mod.rs:757-775）、`tokio-1.54.1`（semaphore.rs/batch_semaphore.rs）、`tower-0.5.3`（limit/concurrency/service.rs）源码契约。未运行 cargo build/test/clippy/fmt、acceptance、服务或 panic 复现；未复现 405 实跑（按任务边界，静态源码证据已足，实跑归 TC-17 执行期）。

PIPELINE instance-concurrency-limit GATE2 PASS 逐FR覆盖与可实施性通过、代码现状逐行吻合（90268ca保留清单精确）、备选取舍成立、7条AC双落位可测、TA差异清单5项全部闭合以AA源码定论为准（405语义经本机axum源码独立复核）、风险提示三条衔接通过，仅3项P3备注，可交DA按§4.1最小修复实施
