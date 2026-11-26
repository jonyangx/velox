# 测试用例（v1 定稿）：velox-config 单实例最大并发请求限流

> 状态：v1 定稿——已对齐 `01-requirements.md` 冻结 AC（GATE1 复审 **PASS**，2026-11-26 11:08，G1-1～5 全部闭合，Q1～Q6 决议固化，见 `01-requirements-review.md`）。
> Feature：`instance-concurrency-limit`。基线：develop @ 28c9114；实现 `90268ca`、单测 `dff2006` 为既有资产，本 feature 后续仅修真实缺口（FR-6 技术上界与无损转换、日志观察前提），不机械重写，不改 addlog 默认 warn。
> 底稿说明：v1 设计稿（2026-11-26 08:59，21TC/7AC/2RQ）为本轮底稿；其 RQ-1（日志默认 warn 与启动可见性）、RQ-2（技术容量上界）已由 GATE1 裁决写回需求（分别落 G1-1、G1-2），不再是需求问题。本轮以冻结 AC 逐条回填矩阵：TC 编号与功能点分组不变，无撤销 AC，无新增/改写 AC。
> 与 F-AA 同波并行：不假定 `02-architecture.md` 已同步冻结决议（当前工作树该文件仅 1 行措辞改动），其差异清单见 §5 末尾，供 GATE2 仲裁；本文档为本工序唯一输出，未改其他文档、代码或状态文件。
> 本轮只做静态设计与文档更新，只读核对代码与既有单测；未执行构建、测试、压测或服务启动，文中全部期望为验收判据，**无任何通过结论**。

## 1. 测试范围与分层策略

仅覆盖 velox-config 的单进程业务路由共享并发名额：GET 列表/单项、PUT、DELETE 共用同一上限（冻结 Q2：`[server].max_concurrent_requests`），默认 10；超限立即 429（冻结 Q3：`code=too_many_requests`、`Retry-After: 1`、message 含上限）、统一错误体、不执行 handler；配置合法域整数 **1..=U**（冻结 Q6，`U = min(i64::MAX, usize::MAX, tokio::sync::Semaphore::MAX_PERMITS)`）、非法值 fail-fast（冻结 Q4）、healthz 豁免（冻结 Q1）、空 env 不覆盖文件（G1-3）、失败/取消释放、日志观察前提（G1-1）和升级说明（冻结 Q5）。分布式协调、限速、排队、热更新、流式响应不在范围（需求 §6）。

| 档位 | 测试层及范围 | 后续执行政策 |
|---|---|---|
| T0 | config.rs/limit.rs/api.rs 内嵌单元与进程内 Router 集成，受控门闩确定边界 | 常规轮必跑受影响套件；本轮只设计 |
| T1 | 真实二进制启动、七种配置来源、非法退出码与时机、启动日志观察前提 | 本 feature 涉及配置加载与启动装配，常规轮触发 |
| T2 | workspace 全量回归与跨 feature 日志兼容；文档审查 | 结合最终变更风险判定；终验必跑全量回归 |
| T3 | TCP/HTTP 活体并发饱和与恢复、WARN 日志、SIGINT；demo 协作链路 | 终验必执行；不能用瞬间突发流量无 429 反推限流无效 |

优先级独立于档位：P0 冒烟、P1 核心、P2 健壮性。日志类用例按 G1-1 前提执行：INFO 证据须**显式开启 info 且过滤器允许对应 target**（如隔离环境 `RUST_LOG=info`，注意隔离残留高优先级 `RUST_LOG`，遵循 addlog 的 `RUST_LOG > LOG__LEVEL > 文件 > warn` 优先级）；默认 warn 下 INFO 启动行静默属预期，以配置解析结果与受控饱和/恢复行为证明上限。全部预期均为计划，既有测试存在不代表已通过。

## 2. 测试环境与基建（含底稿侦察结论）

### 2.1 现有实现与可复用夹具（只读核对 develop @ 28c9114）

- `velox-config/src/config.rs`：字段为 i64，显式默认函数返回 10；TOML → 环境变量覆盖 → `validate()` 仅验证 `< 1`，访问器 `as usize` 转换（真实缺口：FR-6 技术上界与无损比较未实现，由 DA 最小修复）。env 覆盖先于 validate 最终值（文件非法值 + 合法 env 的组合语义按优先级链推导，见 TC-09）；空 env 被 `env_override` 过滤（`config.rs:95-97`），不覆盖文件。
- `velox-config/src/limit.rs`：一个 `Arc<Semaphore>` 随 Clone 共享；`try_acquire_owned()` 非阻塞获取，permit 持有至 `next.run(req).await` 返回；取消 future 时由 Drop 归还。响应对象构造完成为结束点，不要求等待客户端读完响应 body。每拒一条 WARN（含 limit/path）。
- `velox-config/src/api.rs`：同一个 limit 包裹全部业务路由（`route_layer` 语义：404/405 不经过限流层），`/healthz` 在层外；`router(store,max)` 是生产装配入口。`error.rs` 返回 429、JSON code/message、`Retry-After: 1`。
- `main.rs` 通过 argv[1] 读取配置路径，先 `Config::load` 再构造限流器与监听；启动 INFO 含 `max_concurrent_requests` 字段。当前源码日志过滤器缺省 `info`（`main.rs:30-35`），addlog 交付后进程缺省为 **warn**；默认 warn 场景的核验依赖 addlog 合并，未合并前该分支记「无法验证/依赖未就绪」，不以显式 info 冒充默认场景。
- Tokio 锁定 1.54.1（Cargo.lock）：`Semaphore::MAX_PERMITS = usize::MAX >> 3`（64 位 2305843009213693951，32 位 536870911），`Semaphore::new` 超界 panic（`semaphore.rs:479-485`）。冻结 Q6/FR-6 据此定义合法域 1..=U 并要求超界为普通配置错误、不进入构造链；当前实现会让 U+1/i64::MAX 走到构造 panic——这是**待 DA 修复的真实缺口**，用例按冻结 AC-6 期望书写，执行期如实记录失败，未复现前不列缺陷。
- Tower 提示（需求 §8 已纠正）：`tower-0.5.3` `ConcurrencyLimitLayer` 单独使用在 `poll_ready` 等待名额、不产生 429，本 feature 不切换该层；既有 uuid/tower 为 dev-dependencies，无需引入新框架。

既有测试以 Rust 内嵌 `#[test]` / `#[tokio::test]` 组织，tower util `oneshot` 和 uuid 已为 dev-dependencies。`limit.rs` 夹具通过 mpsc 通知 handler 已进入，再等待 watch 放行；`config.rs` 以 ENV_LOCK 串行修改目标环境变量；`api.rs::smoke_router_assembly` 测真实 router 的 PUT/GET/healthz/404。临时文件/Store 使用 Uuid 路径；后续新增测试应恢复原 env 并清理临时文件，不能仅清空开发者既有 env。各 env 相关测试须共用锁，或用子进程环境隔离。

既有测试覆盖与局限：

| 既有测试 | 可复用证据 | 尚需补的断言/核验 |
|---|---|---|
| `third_rejected_while_two_in_flight` | 两个 started 确认占满，第 3 个 429，放行后恢复 | 无超时保护、无 handler/存储调用计数，无副作用不能只凭注释认定 |
| `shared_counter_across_routes_methods` | GET /a 与 PUT /b 共用 limit | 第 3 个探针是 GET /b，而 /b 仅注册 PUT，受 405 语义影响——**静态疑点（复审 §H8）**：冻结 AC-2 的共享证明必须用命中方法探针（GET /a 或 PUT /b，或按 AC-2 原例对同一 `{ns}/{k}` 路径用 GET 与 PUT）；405 行为单独核验（TC-17），当前不宣称该测试失败 |
| `reject_body_shape` | JSON/状态/Retry-After/上限字段（与冻结 Q3 一致） | 保留并复用 |
| `exempt_route_stays_available`、`unmatched_route_bypasses_limit` | 饱和时层外 healthz、404 可用 | 生产 router 饱和装配验证、405 与取消/500 释放尚待补 |
| config.rs 默认/env/非法值/合法极值 7 个 `#[test]` | 覆盖 AC-5 情况 1/2（`default_when_*`）、3/4（`env_overrides_file`）、7 的无文件分支（`env_empty_falls_back`）、0/-1/1.5/abc（`invalid_*_fail*`）、1/100000 加载（`valid_extremes`） | AC-5 情况 5/6（文件 5+无 env/空 env）、AC-6 的 1000、U 与 U+1/i64::MAX 构造边界、真实退出码与监听前终止不能由 `Config::load` 成功替代 |

### 2.2 命令、真实 e2e 调用核验

项目门槛：`cargo fmt --all --check`；`cargo clippy --workspace --all-targets -- -D warnings`；`cargo build --workspace`；`cargo test --workspace`。后续 T0 可用 `cargo test -p velox-config`；统一终验使用 workspace 一次全量结果。本轮均未运行。

已全文核对 `velox-demo/scripts/acceptance.sh` 与 `velox-demo/README.md`：调用均为 **`bash velox-demo/scripts/acceptance.sh`，无子命令或 --host 参数**。脚本内部构建 demo-provider/consumer、`run.sh start`，用 curl/jq 验证配置热更、注册、mesh 转发、扩容及剔除；退出 trap 清理 demo/core 进程并移除 `data/demo.json`，应在隔离 checkout/验收环境运行。前置工具 cargo/curl/jq，端口 8100/8101/8200/8300/9000/9001/9100 空闲。脚本为本机源码 host 模式；脚本未专门制造 config 饱和，PASS 仅证明协作回归，不能代替本 feature 的 AC-1～4/7 专项 e2e；其 SIGTERM/TTL 场景针对 provider，不能替代 config SIGINT 优雅关停。

### 2.3 确定性测试方法及真正 e2e 观察面

进程内夹具 G：生产 `limit_routes` + 可暂停测试 handler；handler 先原子递增进入/active/peak 计数并发送 started，再等 watch/oneshot 放行，退出由测试 Drop guard 减 active。测试收到 N 个 started 才发超限请求；持有者尚未释放时，被拒 future 必须在测试保护超时内（建议 1s，属于防挂预算而非产品 SLA）完成；若超时则失败并保留日志，不能先放行再称"立即拒绝"。每个等待/JoinHandle 都设超时，finally 放行、abort 并 await，防挂死。DELETE/列表/不同 ns/key 用同一个状态与 limit；不能为每条路由创建独立信号量。

真正 e2e 夹具 E（后续测试目录脚本驱动，不改产品代码）：启动**真实 `target/debug/velox-config`**，N=1、独立端口与 data/log 目录；TCP 发送合法 PUT 请求头与 JSON Content-Length，先只发部分 body 占住名额（当前限流发生在 Json 提取前）。轮询一条已命中业务 GET，首次观察到 429 即确认占满（轮询有总超时，未观察到则记录夹具建立失败，不作产品结论）；再以第 3 个请求获取 429 及 JSON/头，同时请求 healthz=200；补齐 PUT body 收到 200，随后 GET 回读成功，证明释放与真实存储链路。全过程保存原始请求、状态/响应体、时间戳、stdout/stderr/文件日志及退出码。

此手段证明真实服务限流入口与解析窗口，不等同于精确量出 handler 并发峰值；峰值 ≤N 的严格证明由 G 的 started/active 计数承担。客户端断开 TCP 不保证服务 future 立即取消，取消释放的确定性用例使用 abort 并 await 的 G，e2e 断连仅作补充。不得为了造慢请求改 src 或增加产品调试路由。

## 3. 用例表

> Q1～Q6 均为 GATE1 冻结决议：Q1 healthz 豁免；Q2 `[server].max_concurrent_requests`；Q3 `too_many_requests` + `Retry-After: 1`；Q4 fail-fast（含技术超界，非 0 退出、非 panic）；Q5 接受默认 10 收紧并声明；Q6 合法域 1..=U、不设额外业务封顶、超界可读错误。

| TC | 功能点/对应 AC | 前置条件 | 操作步骤 | 可观察期望 | 优先级 | 档位/自动化 |
|---|---|---|---|---|---|---|
| TC-01 | 边界/AC-1 | G，N=2 | 起两个 GET 并等待两个 started；未放行时请求第 3 个；之后放行并重试 | 第 3 个在保护超时前 429；前两个 200，全程 peak≤2（AC-1 And 不超卖）；释放后 200 | P0 | T0/集成 |
| TC-02 | 全局共享/AC-2 | G，N=2，多路由多方法（按 AC-2 原例：同一 `/api/v1/config/{ns}/{k}` 同时持 GET 与 PUT，另配列表/DELETE） | 按住 GET ns-a/k 与 PUT ns-b/k；依次发**命中方法**探针（命中的 DELETE、列表、GET/PUT 已注册对）；释放 | 合计 2 即全部 429；拒绝期间 handler 进入数不增加；跨方法/路由/namespace 共用同一名额。**GET /b（仅注册 PUT）不得用作共享证明**（405 语义风险，由 TC-17 独立核验） | P0 | T0/集成 |
| TC-03 | 错误契约/AC-3 | G，N=2 已饱和 | 请求命中业务路由并解析响应 | 429；Content-Type application/json；JSON `code=too_many_requests`（冻结值）、message 非空且含当前上限数值 2；`Retry-After: 1`（冻结 Q3，仅重试提示）；字段结构与 `error.rs` 的 `{code,message}` 一致 | P0 | T0/集成 |
| TC-04 | 无副作用/AC-4 | G，按住名额；测试 handler 使用调用计数和真实临时 Store | 在饱和期间依次发 GET/PUT/DELETE；记录进入计数、内存值、持久化字节；放行后重试 | 均 429，handler/读写调用计数不变（AC-4：被拒请求不读不写不删存储、不占名额）；既有值及文件内容不变、新 key 不存在；放行后可受理，拒绝不漏 permit。仅数据不变不能证明无读访问，必须附调用计数 | P0 | T0/集成 |
| TC-05 | healthz 豁免/AC-2 关联、Q1 | G，N=1 饱和，healthz 层外；生产装配另由 TC-18 核验 | 持有业务请求时多次 GET healthz，再请求业务路由 | healthz 200 含 status=ok；业务仍 429；healthz 不消耗/阻塞业务名额，且不得因探针放宽业务上限（Q1） | P1 | T0/集成 |
| TC-06 | 正常与失败释放/AC-4 关联、§5 | G，N=1，分别让 handler 返回 200、400、404、500 | 各轮等待 started 并放行到指定响应；await 完成后发下一请求 | 每轮响应结束后下一请求可进入；成功/错误分支均无 permit 泄漏（FR-2：响应完成即释放） | P1 | T0/集成 |
| TC-07 | 取消释放/AC-4 关联、FR-2 | G，N=1 | handler 进入后 abort 所属 oneshot 任务并 await 取消完成；不发放行信号；发新请求 | 被取消 future drop 后名额归还，新请求能进入；无需依赖 TCP 断开时机 | P1 | T0/集成 |
| TC-08 | 默认与 env 优先级/AC-5 | env 隔离；可控 G 容量从 Config 值构建 | 依次 7 种启动方式：①无文件；②文件缺键；③仅 env=3；④文件 5+env 3；⑤文件 5+无 env；⑥文件 5+空 env；⑦无文件/缺键+空 env；各组按生效上限 N 个 started 占满，再发第 N+1 个业务请求，释放后重试 | ①②=10，③④=3，⑤⑥=5，⑦=10（G1-3：空 env 不覆盖文件、不统一回落 10）；各组第 N+1 个业务请求在持有者释放前 429、释放后可再次受理（行为生效，不以仅字段断言替代）。日志作上限证据时须显式 info 且允许 target；默认 warn 场景靠配置+饱和行为证明，INFO 行缺失不判配置失效 | P0 | T0/单测+集成 |
| TC-09 | 空 env 与文件非法+合法 env 组合/AC-5 关联、FR-5/FR-6、QA P3 RQ-B-1 | env 隔离 | 空 env 分别配无文件与文件=5；另两组组合：文件=0（非法）+非空合法 env=3；文件=5（合法）+非空非法 env=0 | 无文件 10；文件 5（空串不覆盖文件，不能统一断言回落 10）；文件非法+合法 env：按优先级链选中 env 值 3，文件值被覆盖不触发校验，启动成功生效 3（与 addr 语义及实现"先覆盖后 validate 最终值"一致）；文件合法+非法 env：按 env 报错，不回落文件值 5 | P1 | T0/单测 |
| TC-10 | 非法配置 fail fast/AC-6 | 临时配置及隔离 env | TOML 依次 0、-1、1.5、字符串 abc，另加超出 i64 解析域的整数原值；env 依次 0、-1、1.5、abc；调用 load 并以真实进程重复 | load 报普通可读错误（含配置项名 `server.max_concurrent_requests` 或 env 全名 + 实际原始值，超 i64 解析域的输入保留原值上下文、不进入转换链）；真实进程非 0 退出且在构造限流器/开始监听前终止；不静默默认、不截断、无未捕获 panic；错误走 stderr，不依赖 INFO 开启（FR-6）；TOML 语法错误变体沿用原始启动失败，不归因于限流 | P0 | T0/单测；T1/e2e |
| TC-11 | 合法下界与大值/AC-6 | 分别 1、1000、100000（隔离 env，文件提供） | load→访问器→构造真实 limit 并核对容量（available_permits==配置值）；N=1 用 G 验证饱和行为 | 全部可加载且容量与配置一致地安全构造（AC-6 And：合法值安全构造容量一致的限流器）；大值不以只反序列化成功替代构造验证；不必发起十万个请求 | P1 | T0/单测+集成 |
| TC-12 | 技术容量边界 1..=U/AC-6、Q6（冻结） | 读取目标平台 U=min(i64::MAX, usize::MAX, Semaphore::MAX_PERMITS)（64 位 2305843009213693951；32 位 536870911） | 分别验证 U、U+1、i64::MAX 的 load 与构造结果；U 仅核对构造与容量，不发起 U 个请求；后两项以隔离子进程捕获退出码与错误输出，防构造 panic 波及测试进程 | **冻结期望**：U 合法——load 成功且构造容量恰为 U；U+1 与 i64::MAX（均在 i64 解析域内但超 U）→ 普通可读配置错误，含配置项名/env 全名、原始输入与适用上界 U，真实进程非 0 退出、在构造限流器/监听前终止，无未捕获 panic、不截断/饱和/静默默认。现状缺口：validate 仅查 <1 且 `as usize`，预期修复前这些用例失败，如实记录（复审 R5.1：需求 PASS 不代表缺口已修复）；未复现前不列缺陷 | P2 | T0/隔离单测，T1/子进程 |
| TC-13 | 装配默认与启动日志观察前提/AC-5、AC-7 | 真实二进制，独立 data/log/端口，隔离 `RUST_LOG` 残留 | AC-5 的 7 种启动方式逐个运行；每组先按显式 info（如 `RUST_LOG=info`）观察 INFO 启动行，再按默认日志配置运行并以 N 占满/第 N+1 个 429 证上限；保存启动日志/healthz/退出码 | 显式 info 且 target 允许时：INFO 启动行含生效上限（10/10/3/3/5/5/10 与 TC-08 一致）；默认 warn 场景：INFO 静默属预期、不判配置失效，以配置解析结果+饱和行为证明上限（G1-1/AC-5 And）；非法值组的退出码与时机另见 TC-10。默认 warn 进程级行为依赖 addlog 合并，未合并前该分支记「无法验证/依赖未就绪」，不以显式 info 冒充 | P1 | T1/e2e |
| TC-14 | 每拒一 WARN 与过滤预期/AC-7 | G 饱和，测试局部 tracing 捕获器（传播至 spawn 任务，不污染全局 subscriber）；覆盖 addlog 默认 warn | 发 3 个超限请求，含不同 path；释放后发成功请求；另在显式 error/off 或 target 过滤 WARN 的过滤器配置下重复 | 恰 3 条对应拒绝 WARN，每条含 limit/path，不采样省略；成功不误记拒绝；**过滤预期**：显式 error/off 或 target 过滤时 WARN 不可见属过滤预期，事件生成义务不变（在捕获层核验事件生成，不以 subscriber 可见性否定事件）；默认 warn 且未屏蔽该 target 时 WARN 可见（AC-7，依赖 addlog） | P1 | T0/集成 |
| TC-15 | 立即拒绝真实链路/AC-1、3、4、7 | E，N=1 | 部分 PUT body 占名额，观测 429 后再请求业务；保存 JSON/头与 WARN；补齐 body 后读回 | 429 发生在放行 PUT 前（不排队、立即可观测），错误契约正确，每拒有 WARN；PUT 完成后 GET 成功、存储值正确，无永久饱和 | P0 | T3/e2e |
| TC-16 | 竞态不超卖/AC-1、NFR-3 | G，N=2，多个已 spawn 请求由统一 barrier 放行 | 首批等待 2 个 started，其他请求应被拒；检查 peak 并分轮释放/重试 | peak≤2（NFR-3 不超卖，允许保守少受理）；handler 计数与受理数一致，拒绝未进入 handler；下一轮正常。barrier 只是同时起步，started 与门闩才是占满证据 | P1 | T0/集成 |
| TC-17 | 404/405 边界/FR-2 | G 已饱和，/a 仅 GET、/b 仅 PUT | GET 未知路径、GET /b（方法不匹配），并以 PUT /b 作已命中对照 | 未知路径 404 且不占名额；GET /b 的实际行为**独立记录实跑结果**：api.rs 注释称 route_layer 使 405 不经限流层，与既有 `shared_counter` 测试预期 429 存在静态分歧——实跑结果如实留痕，分歧不以静态判断定缺陷、不用 405 路径证明 AC-2；PUT /b 应 429 | P2 | T0/集成 |
| TC-18 | 生产 router 装配/AC-1、2 关联、Q1 | 真实 `api::router(store,N)`，临时 Store | 正常 PUT/GET/DELETE/列表与 healthz；以可控请求 body 流暂停 PUT 提取，确认其他业务 429 后访问 healthz；补齐 body | 各业务路由经过同一限流入口；healthz 饱和时 200 且含 status=ok/service=velox-config；放行后业务恢复 | P1 | T0/集成 |
| TC-19 | 优雅退出/§5 | E 在途 PUT 已确认占名额 | 发 SIGINT，确认未提前退出；补齐 body 并读响应；等待退出 | 在途请求正常完成，进程在保护超时内退出，限流计数随响应完成释放，不阻碍关停；生产只处理 ctrl_c，不能用 SIGTERM 等价替代 | P2 | T3/e2e |
| TC-20 | 文档/FR-4、NFR-4、Q5、QA P3 RQ-B-2 | 最终实现与发布文档 | 抽查 README/配置示例/发布说明：env 命名、默认值 10、429/Retry-After、需重启生效、调优指引、升级收紧声明；核对合法技术域 1..=U、空 env 不覆盖、日志核验前提是否补齐；**逐条核对文档锚点实际行号**（§8 引 `README.md:40`，实查 429/Retry-After/healthz/收紧说明位于 :44-49、:40-46 为配置表） | 明确升级后默认 10 行为收紧；示例与代码一致，不误称速率或集群配额；合法域/空 env/重启/日志前提与冻结需求一致；锚点行号偏移如实记录（RQ-B-2 为 P3、内容属实），供后续文档同步校正 | P1 | T2/手工 |
| TC-21 | 统一回归/AC-1～7 关联 | 所有 feature 最终落地、无 DA WIP、记录同一覆盖提交 | 先受影响套件，再四项 workspace 门槛，执行无参数 acceptance.sh 及 TC-15/19 专项 e2e | 分别记录原始结果、覆盖提交、耗时；demo 成功不等于专项限流成功；失败如实记录，不预填通过 | P1 | T2+T3/自动化 |

## 4. AC 覆盖矩阵

| 冻结 AC | 主用例 | 优先级 | 其他证据/执行前提 |
|---|---|---|---|
| AC-1（上限生效且不超卖） | TC-01、TC-16 | P0/P1 | TC-15 真实 HTTP；TC-18 真实 router |
| AC-2（全局共享，跨路由/方法） | TC-02 | P0 | 探针必须命中路由/方法，GET /b 不作共享证明（405 风险归 TC-17）；TC-05/18 涵盖 Q1 |
| AC-3（429 错误体格式） | TC-03 | P0 | TC-15 网络级响应；Q3 已冻结（`too_many_requests`/Retry-After:1/含上限） |
| AC-4（无副作用、不占名额） | TC-04 | P0 | TC-06/07 释放；调用计数为"无读副作用"必要证据 |
| AC-5（默认值与配置优先级，7 情况） | TC-08、TC-13 | P0/P1 | TC-09 空 env 与文件非法+合法 env 组合；日志证据须显式 info，默认 warn 靠配置+饱和行为（G1-1 已裁决，原 RQ-1 闭合） |
| AC-6（非法值 fail fast，合法域 1..=U） | TC-10、TC-11、TC-12 | P0/P1/P2 | TC-12 按冻结 Q6 域判定（原 RQ-2 闭合）；现行实现缺口（下界校验+`as usize`）由 DA 最小修复后才能通过 |
| AC-7（可观测性 INFO/WARN 与过滤预期） | TC-13、TC-14 | P1 | TC-15 真实 WARN；默认 warn 进程级行为依赖 addlog 合并，未就绪记「无法验证」 |

全部 7 条冻结 AC 均有 P0/P1 计划用例，**无 P0/P1 覆盖缺口**；边界场景逐条见 §5。属设计覆盖，无实跑通过结论；TC-10～12 的通过以 DA 完成 FR-6 最小修复为前提。

## 5. 边界场景覆盖对照

| 边界 | 用例/判定方法 |
|---|---|
| N-1、N、N+1；跨路由/方法/ns/key | TC-01/02/16，通过 started 与 peak 确定，不用 sleep 猜测请求已进入 |
| 被拒不排队、不访问 Store | TC-01/04/15；门闩未放行先收到 429；调用计数与内存/文件快照联合证明 |
| 返回 4xx/5xx 与取消 | TC-06/07；abort 并 await 保证 future 确已 drop，不以客户端关闭连接代替 |
| 响应体未传完（Json 提取窗口占名额） | TC-15/18；响应对象返回即释放，未来流式响应不在本需求 |
| 配置七种来源、空 env 不覆盖、文件非法+合法 env 组合 | TC-08/09/13；空 env 有文件保留文件值，无文件才默认 10（G1-3 固化） |
| 非法值与技术上界（0/-1/1.5/abc、U+1、i64::MAX、超 i64 解析域原值；合法 1/1000/100000/U） | TC-10/11/12；配置 parse 成功不代表 Semaphore 可构造；U 仅核对构造与容量；无 panic、监听前终止、错误含字段/原值/U（Q4/Q6 固化） |
| healthz 超限豁免 | TC-05/18；不占名额、饱和仍 200，不为探针放宽上限（Q1 固化） |
| 404/405 | TC-17/18；405 实际行为独立记录，架构"405 不经限流层"声明与既有测试预期 429 的静态分歧按实跑留痕，未复现前不是缺陷、不用于 AC-2 证明 |
| SIGINT 优雅关停 | TC-19；demo 脚本 provider 的 SIGTERM/TTL 不能证明 config 优雅退出 |
| 非法 TOML 语法 | TC-10 夹具变体，沿用原始启动失败，不归因于并发限流 |
| 日志过滤预期（显式 error/off、target 屏蔽；默认 warn 静默） | TC-13/14/15；区分事件生成义务与 subscriber 可见性（G1-1/NFR-2） |
| 热更新/速率/跨进程协调 | 范围外；不新增热更新 API，不把单次突发吞吐当并发上限证据 |

### 02-architecture 差异清单（与 F-AA 并行，供 GATE2 仲裁）

本轮不假定 AA 已同步冻结决议（当前工作树该文件仅 1 行"CLI 参数→启动参数"措辞改动）。以冻结需求为准，观察到以下差异，属 AA 对齐义务（复审 §H8、R5.3 已列），不据此判定冲突：

1. 页首引用"FR-1~FR-7"与冻结需求 FR-1～FR-6 计数不一致；
2. Q6 行仍写"`Semaphore::new(usize)` 无上限校验，仅 `≥1` 下限"——与 tokio-1.54.1 源码契约（`MAX_PERMITS = usize::MAX >> 3`、`new` 超界 panic）及冻结 Q6/FR-6 合法域 1..=U 冲突；
3. 配置流程"空串回落默认，保持现有语义"（:63 附近）措辞与冻结 FR-5"空 env **不覆盖文件**"不一致（无文件时才默认 10）；
4. 404/405 route_layer 语义（:234/:356，注明 axum 0.8.9 已实测）与既有 `shared_counter` 测试"GET /b=429"的静态分歧，仍须按 TC-17 实跑独立核验，不得以文档声明代替；
5. 页首 Q1～Q6 若仍标"待裁决/默认方案"，应更新为 GATE1 冻结决议。

## 6. 需求问题清单（RQ-x）

**无**（RQ 数量 0：冻结 AC-1～AC-7 经逐条核对均可判定、可观测、无自相矛盾；原 v1 设计稿 RQ-1/RQ-2 已由 GATE1 裁决写回需求并闭合——RQ-1 落 NFR-2/AC-5/AC-7 日志观察前提（G1-1），RQ-2 落 FR-6/AC-6 合法域 1..=U 与超界 fail-fast（G1-2），不再是需求问题）。

QA 复审 P3 备注处置（不属需求问题，已纳入用例设计）：RQ-B-1（文件非法值+非空合法 env 组合未显式覆盖）→ TC-09 补组合分支，按优先级链"选中 env、文件值被覆盖不触发校验"固化；RQ-B-2（§8 README 锚点行号偏移）→ TC-20 锚点逐条核对留痕，内容属实、待文档同步顺带校正。

执行期预期记录（非 RQ）：TC-10～12 中 U+1/i64::MAX 等用例在 DA 完成 FR-6 最小修复前将失败、默认 warn 进程级场景在 addlog 合并前记「无法验证」——均为已知真实缺口/依赖，按冻结 AC 如实记录，不得据此降低期望或宣布需求矛盾。

现有实现与测试保留；后续只针对缺失断言补充，不机械重写。常规轮按 T0～T3 触发条件留痕，终验全量回归与 e2e 无条件执行；所有 feature 终验引用同一最终覆盖提交。本次不创建 04 报告、不启动任何验证轮、不向其他 feature 或流水线状态文件写入。
