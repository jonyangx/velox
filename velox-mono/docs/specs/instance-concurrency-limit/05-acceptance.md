# 验收报告：instance-concurrency-limit（GATE3 终验）

> Task ID：QA-1795673097097-36a9012671f506c1；验收日期：2026-11-26（Asia/Shanghai）。
> 独立只读终验，CLI_EXECUTION=INNER；未派发代理、未运行构建/测试/格式化检查/服务，未执行 git 写操作。本次仅新建本文件。运行结果唯一取自 `04-test-report.md`，以下“实测”均指该报告已有记录，不表示 QA 重新执行。
> 验收基线：`develop`，HEAD=`4b24a20131e88c3bfc7a368cdbf1b2e5eb4ac086`；存在已披露的纯格式在途差异，详见 §7。

## 1. 验收结论

**CONDITIONAL（有条件通过）。** 冻结 AC 的产品行为证据 7/7 满足，最终代码的 T2 全量与 T3 活体实跑具备复用基础；但报告尚未明确完成 F-TA-F 复用判定/终验标记，且本 feature 的 04 报告仍未跟踪入库，两项交付收口条件未完成，不能给出无条件 PASS。

未发现本 feature 产品缺陷或需要 DA 修改产品代码的事项。放行须逐项闭合 §8 的 C1、C2；本报告不代替 TA 补终验标记，不把流水线任务中的完成主张直接当作工件事实。

## 2. 五件套核验记录

动手前先读 `git status --short --branch`，随后以 `shasum -a 256` 独立重算；01/02/03 全部与任务锁值逐字一致，未触发 SHA 不符停止条件。文件行数经 `wc -l` 复核。

| 交付项 | 非空/规模 | 重算 SHA-256 / 基线 | 核验结果 |
|---|---|---|---|
| `01-requirements.md` | 有，228 行 | `abcf44b10fa4890c24cc053db1229ce5321bd96859bedbecaf21fa7a2b48c0e3` | 与冻结锁值一致；已入库 |
| `02-architecture.md` | 有，303 行 | `27cba700c619c0355ddc9f417a60c45f58b5047d808265056c098d98157cbf93` | 与定稿锁值一致；已入库 |
| `03-test-cases.md` | 有，136 行，21TC/7AC，§4 覆盖矩阵 | `e48455f6028fb87a516a67b291ad00672e30f8106f6d814c9fe93dc0f10a36e9` | 与 v1 锁值一致；已入库 |
| 代码+单测 | 有，既有实现/单测+最终修复；config 套件 23 例 | `90268ca → dff2006 → 46e928a → 4b24a20`（非相邻提交，详见 §7） | 代码/测试与设计和报告抽查相符 |
| `04-test-report.md` | 有，105 行，21 条 TC 结果 | `a16f50066e420230fda2e8749a019f5ca5e9cec0784e0ec8ff963c2449e32c4a` | 任务未给锁值，此为本次取证指纹；未跟踪，记 QA-02 |

五件套存在性通过，归档完整性有条件。两个前置关口工件亦非空：`01-requirements-review.md` 重算 `cf74ad66e17b32a008ff5902cd3c825e7caf70b976e9b3f2bc12f78137f18181`，其复审结论 PASS；`02-architecture-review.md` 重算 `f68b4fc621af2217392aac6c88657789b35a423f84c974f6008d1c6570bf8533`，结论 PASS。

证据简称：R=`01-requirements.md`，A=`02-architecture.md`，TC=`03-test-cases.md`，T=`04-test-report.md`；下列行号均为本次指纹对应的版本。

## 3. 逐 AC 核对表

| 冻结 AC / 预期 | 判定 | 报告实跑证据出处 | 交叉核验与边界 |
|---|---|---|---|
| AC-1：N=2，两在途成功，第 3 个 429，不超卖 | 满足 | T §3 TC-01（第37行）：`limit::tests::third_rejected_while_two_in_flight`；TC-16（第52行）：C6 真进程 N=2、10线程、3轮结果 `[(2,8),(2,8),(2,8)]`，每轮仅2个 key 写入、释放后恢复；TC-15：持有者未放行前收到429 | 既有测试等待两个 started 后才发第3个请求、前两个放行后200；`limit.rs` 共享 Semaphore、非阻塞取 permit 与 RAII 给出上限不变式。有限轮实测不表述为穷尽所有调度 |
| AC-2：跨路由/方法共享全局名额；healthz 豁免 | 满足 | T §3 TC-02/05/18（第38/41/54行）：C3 两个半包 PUT 占满N=2，GET/PUT/DELETE/两种列表共5条命中业务探针均429、释放后200；连续5次healthz均200，含status=ok/service=velox-config，同刻业务仍429 | `api.rs:33-46` 创建一个 limit、全部业务同层、healthz层外；未把既有 GET /b 的405路径作为共享证明 |
| AC-3：429、JSON结构/code/message含上限、Retry-After:1 | 满足 | T §3 TC-03/15（第39/51行）：`reject_body_shape`（N=3）及C1（N=1），429、application/json、`too_many_requests`、含上限的message、`retry-after: 1` | `error.rs:26-36` 与冻结R AC-3、A §4.3一致 |
| AC-4：被拒请求零读写删副作用、不占名额，释放后可受理 | 满足 | T §3 TC-04/06/07/15（第40/42/43/51行）：C1 GET/PUT/DELETE均429、文件字节不变、keep原值/blocked不存在、放行后恢复；C5a六路200/400/404/500后均可再受理；C5b断连后恢复200且cancel无写入 | 无读访问并非由快照单独证明：T TC-04明确结合guard失败分支不调用next的白盒证据，本次只读确认 `limit.rs:40-50`。此为对计划调用计数的替代证据，限制详见§5 |
| AC-5：默认10、env>文件>默认、空env不覆盖，七种来源与日志观察前提 | 满足 | T §3 TC-08/09/13（第44/45/49行）：Batch B七情况上限10/10/3/3/5/5/10，真实饱和429/释放恢复；9组来源双模式，显式info含精确上限、默认warn无INFO但饱和/WARN有效；文件0+env3成功、文件5+env0失败 | `config.rs:75-98,138-139` 文件→非空env→validate；INFO证据仅在显式info且target放行下成立，默认warn静默不判失败 |
| AC-6：合法域1..=U无损校验、非法值普通错误fail-fast、无panic、构造/监听前终止 | 满足 | T §2 T1及§3 TC-10/11/12（第25/46-48行）：16项非法输入rc=1，无panic、监听前终止、stderr字段/原值，技术超界含U；合法1/1000/100000/U健康检查200、SIGINT rc=0；`valid_capacity_boundary_u`核对Semaphore容量；`over_technical_cap_fails_with_bound`、`parse_domain_overflow_keeps_raw_value`通过 | U=2305843009213693951为报告64位实测；32位仅做代码/依赖契约静态核对，不虚称实跑。u128先校验后usize转换，详见§6；空env不归非法值 |
| AC-7：显式info记录上限，默认warn每拒一条且含limit/path，过滤预期正确 | 满足 | T §3 TC-13/14/15（第49-51行）：9组显式info上限精确；默认warn INFO静默；C1三次拒绝恰3条WARN含limit/path，成功不误报；C2 error过滤下3次429、0条可见WARN | `main.rs:49-55` INFO字段及`limit.rs:43-48`拒绝分支WARN事件存在；默认warn来自addlog，未被本feature改写。error不可见不等于事件生成义务被取消 |

AC 判定是运行报告与代码结构联合支持的产品行为判断，不等同于 TC 中每种计划夹具/仪表均照原样执行；终验工序标记仍待 C1。

## 4. 终验级充分性核验（F-TA-F 复用路径）

| 必核项 | 核验结果 | 证据与裁决 |
|---|---|---|
| ① T2/T3针对最终提交4b24a20 | 成立 | T第4行明确覆盖HEAD=4b24a20，第29行重申全档位针对最终代码；第6行披露limit.rs纯格式基线，本次diff吻合。当前HEAD完整SHA一致，`git log 4b24a20..HEAD`为空；不存在报告之后本feature新代码提交 |
| ② T2为workspace全部crate、含--no-fail-fast | 成立，带已知flaky例外 | T §2第26行、§4第79-85行明确 `cargo test --workspace --no-fail-fast`，六crate均列出，共130例、129通过、1已知demo flaky，隔离11/11通过；静态门槛与demo acceptance均有结果/耗时。已知失败如实保留，不能称原始workspace命令130/130全绿 |
| ③ T3为真进程/真端口活体端到端 | 成立 | T §2第27行与§3 C1～C7：真实target/debug/velox-config、TCP半包夹具、隔离端口/目录、HTTP响应、文件快照、日志、SIGINT与退出码。不是纯oneshot或demo脚本替代；无T3集成等价降级遗留 |
| ④ 报告明确完成复用判定与终验标记依据 | **不成立，QA-01** | T第3行仍标常规执行轮；TC-21第57行写“统一终验待F-TA-F确认”“若4b24a20为最终提交…可…复用”；末节仍为未执行用例说明，无已完成的终验判定/标记。任务/索引宣布复用不能补齐报告本身的留痕 |

**裁决：前三项满足，现有实跑结果具备终验复用的实质充分性；四项记录要求尚未全部闭合，终验记录完整性有条件。** 按 `dev-pipeline.md:167` 的“已含最终提交T2+T3则仅补终验标记、不重复执行”规则，C1只要求TA补明确记录，无需独立重跑。不能将缺标记说成未跑T2/T3，也不能由QA替TA宣称终验工序已完成。

## 5. 预期检查与报告内部一致性抽验结果

### 5.1 数字与行为

| 抽验项 | 复核结果 |
|---|---|
| T0 config 23例、新增7例 | 静态计数为config.rs 14 + limit.rs 5 + api.rs 1 + store.rs 3 = 23；最终提交新增7个测试函数，与T第24/80行吻合。23例×3轮是重复稳定性验证，不并入独立套件总数 |
| T1非法16项 | TC-10列文件6项（0/-1/1.5/abc/23位溢出/非法TOML）+env5项（0/-1/1.5/abc/溢出）=11；TC-12文件/env各U+1、i64::MAX共4项；文件5+env0组合1项；合计16，rc=1/无panic/监听前终止表述一致 |
| T1合法4项、来源9组 | 合法1/1000/100000/U，共4项；七种来源的第7项拆无文件/缺键两个空env变体，再加文件0+env3，合计9；info上限序列10/10/3/3/5/5/10/10/3自洽 |
| T2总量/通过数 | 根Cargo.toml列六成员。只读统计测试属性：config23+log28+registry59（lib38+2+8+11）+mesh4+provider5+consumer11=130；通过23+28+59+4+5+10=129、失败1。隔离consumer11/11是另一次结果，不能相加成140/130。报告“除flaky外129/129”可还原，并非130例全绿；建议C1将汇总直接写129/130通过、1已知失败 |
| 立即429/零副作用/恢复 | §2总延迟范围1.3～2.6ms涵盖TC-01的1.6～2.6ms及TC-15的1.3～2.0ms；均在持有者放行前收到429，不把放行后响应冒充立即拒绝；错误体/头与源码一致 |
| healthz与404/405 | 5次healthz=200且业务仍429；未知路径始终404，方法不匹配未饱和405/饱和429。与A §2.3/4.4、api注释及本机axum源码吻合。TC旧稿静态分歧已由GATE2仲裁和本轮TC-17实跑闭合 |
| 竞态与退出 | 10线程每轮2成功+8拒绝=10；3轮合计6成功/24拒绝，每轮写入2个key，与N=2无冲突。C7用SIGINT，先确认0.5s仍活着、再补body获得200、随后rc=0；未用provider的SIGTERM替代 |
| 缺陷0、21TC结论 | 报告列齐TC-01～21，业务结果支持7AC，无新增产品缺陷记录；缺陷栏“无”与另列既有flaky范围一致。TC-21“常规通过”不等于已完成终验，QA-01正是该区别；本次交付记录问题不冒充TA发现的产品缺陷 |

统计未发现超过套件规模等需复跑的异常；按本次只读约束未执行任何抽跑。

### 5.2 测试计划覆盖及替代证据的限制

TC §4矩阵覆盖7AC，无遗漏AC；但执行手法并非逐字照搬TC §2.3/用例表：TC-04没有新增handler/Store调用计数、TC-16没有记录G夹具active/peak仪表、TC-07采用真实TCP断连而非G的abort+await、TC-14采用活体日志而非局部捕获器。T TC-04与§5已披露前两项既有测试不足的一部分，不能把“21/21场景有结果”扩大成“计划仪表全部执行”。

本次接受以下联合证据用于产品AC判断：拒绝分支不执行next故不进入handler/Store；唯一Semaphore的permit结构限制上界；真实断连后业务恢复证实名额已释放；WARN调用在拒绝分支且与过滤后的可见性分开判断。上述静态证明与T的活体结果互补，未发现产品反例。TCP测试不构成对所有取消时机的穷尽证明，3轮写入计数也不声称直接测得handler瞬时peak。建议后续保留自动化夹具与仪表，作为非阻断的测试资产增强待办；不要求本次越权补代码或重跑。

### 5.3 需求冻结链落位

GATE1复审PASS（11:08，评审§R1/R2/R5）→Q1～Q6/G1-1～G1-5冻结→A定稿/GATE2首次PASS（上游记录12:20）→4b24a20最小修复（提交时间13:52:56+0800）→T覆盖该提交（文件mtime14:03:07+0800）。两个规格页首“待评审”是各阶段落盘的历史状态，以对应评审PASS工件及SHA链为准，不视作新开放问题。

| 冻结项 | 最终落点与报告证据 |
|---|---|
| Q1/Q2/Q3、G1-4 | healthz层外；server字段/env命名；`too_many_requests`/Retry-After:1；T TC-02/03/05/18 |
| Q4/Q6、G1-2 | u128的1..=U；普通可读错误；T TC-10/11/12真进程与边界构造 |
| G1-3 | 空env过滤、文件5保留5、无来源默认10；T TC-08/09/13；RQ-B-1组合场景已补测闭合 |
| G1-1 | addlog默认warn不变、info观察须放行target；T TC-13/14/15；velox-log默认实现相符 |
| Q5、G1-5 | 保留既有架构，仅修真实上界缺口；README:51-53保留默认10收紧/调优与日志前提；T TC-20留痕；未切换tower等待式限流器 |

## 6. 修复验证（4b24a20 代码/测试与报告断言只读比对）

- `config.rs:25-36` 使用const fn `min_u128`计算三项最小值，等价于A §4.1的min公式，避免依赖const方法可用性；输入均可无损提升到u128。
- `config.rs:115-125` 先拒绝v<1，再将正整数提升到u128与CAP比较；超界错误包含`server.max_concurrent_requests`、原始v与`U=<数值>`。不存在先转usize再查上界的路径。
- `config.rs:98` 校验后才返回；`main.rs:28/30-48` 配置加载先于日志初始化、Store打开、router构造和监听。错误经main的Result到stderr；与T非法输入未创建data/logs、rc=1的记录相容。
- `config.rs:131-134` 的usize转换由已校验1..=U且U≤usize::MAX不变式保证；两个debug_assert是辅助，生产安全性不依赖其启用。
- 七个新增测试在`config.rs:281/292/304/316/353/391/417`：文件无/空env、缺键空env、合法/非法组合、技术超界、解析域溢出、U容量。超界测试检查字段/原值/U；U测试同时构造真实limit及Semaphore并断言available_permits==cap，与T TC-08～12一致。
- `valid_extremes`既有测试只检查1/100000加载，不能单独证明大容量构造；T T1四项真实启动补足1/1000/100000/U构造链，U另有容量断言。
- 本机Cargo.lock锁定axum0.8.9、tokio1.54.1、tower0.5.3。只读本机registry：tokio `batch_semaphore.rs:137` 上界为usize::MAX>>3，构造处assert；axum `path_router.rs:311-341`→`routing/mod.rs:757-775`→`method_routing.rs:967-993`，fallback同样被layer包裹，证实405定论。64位U=2^61−1、32位U=2^29−1，均小于i64::MAX；未宣称32位执行验证。

## 7. 分支与提交检查记录

### 7.1 分支、提交链与内容

- `git status --short --branch`：`develop...origin/develop [ahead 4]`，HEAD完整SHA见页首。遵守指定develop分支；ahead本身不是交付缺陷，未要求推送。
- 实际历史：`90268ca → dff2006 → 46e928a → 28c9114 → af28e65 → d78061d → a5b75e5 → 4b24a20`。中间registry/addlog提交归其他feature，不能误写成四个相邻提交。四个目标提交分别经`git merge-base --is-ancestor <rev> HEAD`核验返回0。
- 目标提交标题分别为`feat(config)`、`test(config)`、`docs(specs)`、`fix(config)`，符合Conventional Commits，无wip标题。git show核验：既有实现、既有单测、初始规格、最终修复分别对应说明；4b24a20修改config行为/7个测试、api注释、README/TOML、五份规格工件，与正文一致；未改main/limit/error/Store逻辑或增加依赖。
- `git diff --quiet 4b24a20 --` 对最终核心文件、README/TOML及01/02/03返回0；`git log 4b24a20..HEAD`及feature路径过滤结果为空，报告后无新的feature代码提交。
- 敏感信息抽查：检查目标提交新增行中的私钥标记、常见云/API/GitHub密钥格式与显式凭据赋值，匹配0；结合diff人工抽查，未发现密钥/token、临时调试入口、大段注释废代码。此为本feature差异检查，不声称完成全仓秘密扫描。

### 7.2 在途改动归属

| 进入终验时的git状态 | 归属/处置 |
|---|---|
| `M velox-config/src/limit.rs` | 独立逐hunk核对，仅TooManyRequests结构体表达式合行、healthz oneshot调用换行两处rustfmt；无逻辑差异。与4b24a20正文、T第6行披露一致，为既有在途格式改动；保留原样，不计本feature代码遗留 |
| `?? docs/specs/instance-concurrency-limit/04-test-report.md` | **本feature交付物**，不能归入其他feature豁免；五件套存在但尚未归档，QA-02/C2 |
| `?? docs/specs/_pipeline-state.md` | 承载层流水线状态索引，不是本feature产品代码；不修改 |
| `?? docs/specs/registry-persistence-watch/{04-test-report,05-acceptance}.md` | registry feature在途报告，归其自身流程；不修改 |
| `?? velox-mono/docs/brd/`及可能继续出现的F1 addlog TAE工件 | mono/其他feature在途材料；初始status未列addlog独立04文件。不得虚称已逐一验收未出现的文件；归属说明后不纳入本feature清理条件 |
| 本次新建`05-acceptance.md` | 当前QA授权输出，进入终验前不存在；与04一起待后续文档收口，QA不自行提交 |

因此“本feature无未提交/未跟踪遗留”本项目前**不满足**，原因是04报告而非其他feature的在途改动。

## 8. 遗留问题盘点与建议

| 编号 | 状态/级别 | 证据、影响与处置 |
|---|---|---|
| QA-01：F-TA-F复用终验标记未落报告 | 已记录待办；P2交付记录，放行前必须闭合 | T第3/57/103-105行；前三项实跑充分，第四项缺明确已完成判定。责任TA，执行C1；不要求重跑已有最终提交结果 |
| QA-02：本feature测试报告未跟踪入库 | 已记录待办；P2交付归档，放行前必须闭合 | 初始/复核git status均为`?? .../04-test-report.md`，git ls-files仅01/01-review/02/02-review/03。责任DA/文档收口负责人，执行C2 |
| demo-consumer defaults_match_contract既有flaky | 已记录待办，demo范围，终态备注交上游；不阻塞本feature | T §4第85行、§5第98行：workspace下10/11、串行11/11。复现方式已列`cargo test --workspace --no-fail-fast`，隔离命令`cargo test -p demo-consumer -- --test-threads=1`；源码config.rs:151未取ENV_LOCK，而:198等会改进程env，与竞态解释相容。本次未重现/修复，不称已修复 |
| 原上界缺口G-1/G-2、405注释G-3 | 已修复 | 最终代码/测试及T TC-10/12/17证据见§6；不存在未闭合产品修复项 |
| RQ-B-1 / RQ-B-2、GATE2 P3备注 | 组合场景已验证；锚点等已记录，非阻断 | TC-09闭合组合行为；TC-20记录README实际锚点39/44/51-53。limit行号漂移已在本报告使用当前行号；axum本机fallback源码替代缺失的打包测试引用；env解析错误旧文案仍含字段/原值，满足冻结约束 |
| 计划仪表/夹具与执行方法差异 | 已记录增强待办，非阻断 | 详见§5.2。产品AC有互补证据，但后续建议补可复用调用计数、active/peak与确定性取消自动化，避免仅依赖一次性活体验证 |

无其他已识别的本feature产品遗留；无T3仅集成等价降级事项。上述已知flaky不被“0新增缺陷”掩盖，交上游处理。

**逐条可执行放行条件：**

1. **C1（TA）**：在04报告补F-TA-F复用终验记录/标记，明确最终代码完整SHA为`4b24a20131e88c3bfc7a368cdbf1b2e5eb4ac086`，复用既有§2/§3/§4的T2全量及T3活体记录、无需独立重跑、无后续feature代码变化；将TC-21的“待确认”更新为已确认复用。汇总如实写130例、129通过、1项既有flaky及隔离11/11，保留例外与原始结果。验收方式：只读重算报告SHA，检查该记录/标记与git历史一致；仅文档变更不触发重新构建/测试。
2. **C2（DA/文档收口负责人）**：C1完成后，以单独Conventional Commit归档本feature的04与05报告，保留其他feature在途改动；通过`git ls-files`确认两份报告受版本管理，feature目录无未提交/未跟踪遗留，再做只读QA闭合记录。不要把既有limit.rs格式改动混入该文档提交。若期间出现本feature新的行为代码提交，则本次复用基线失效，须对新最终提交补足T2/T3后再验收，不得仅改覆盖号。

当前结论保持CONDITIONAL；条件完成前不得当作无条件PASS。没有DA产品代码返工清单。

## 9. 取证记录

- 首次及写入前复核`git status --short --branch`；读取01/02/03/04、两个关口评审、QA角色正文，以及流水线F-TA-F规则。运行证据只采信04；索引和任务摘要仅用于定位，不作为实跑替代。
- `shasum -a 256`锁定输入，`wc -l`核验规模；`git rev-parse HEAD`、`git log/show/diff/ls-files`、祖先检查核验提交与工作树；只读源码/本机registry核对上界与405契约；按测试属性统计六crate测试数量，未调用测试发现/执行命令。
- 受检最终代码`config.rs/api.rs/main.rs/error.rs/README/TOML`及01/02/03相对HEAD无差异；limit.rs两处格式差异与报告一致。T文件修改时间晚于最终代码提交，内容明确声明该基线；未把mtime单独作为执行真实性证明。
- 审查范围为本feature及必要的共享启动/依赖契约；已知demo flaky按报告与源码交叉核验，不扩展到其他feature终验。未运行cargo build/test/fmt/clippy、acceptance脚本、curl或任何服务；未写临时文件、未修改流水线状态、未提交/切分支/恢复文件。
- 本文件生成后再次检查输入指纹、HEAD和状态，并以仓库非忽略文件内容快照核对本次唯一新增文件为05报告；放行条件留给有写入授权的后续工序执行。

PIPELINE instance-concurrency-limit GATE3 CONDITIONAL 放行条件：C1由TA补04报告的4b24a20复用终验记录与标记并如实列130例129通过1既有flaky；C2由DA归档本feature04/05报告并只读复核目录无遗留、代码基线未变化
