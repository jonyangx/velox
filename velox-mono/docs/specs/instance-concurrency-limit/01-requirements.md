# 需求分析：velox-config 单实例最大并发请求限流

> feature slug: `instance-concurrency-limit`
> 状态：已交付（GATE3 有条件通过，收口/放行条件以 05-acceptance.md 为准；04 报告随规格迁移入库）

## 1. 背景与目标

velox-config 是基于 axum + tokio 的配置中心 HTTP 服务（`velox-config/src/main.rs`）。**以下为实现提交 `90268ca` 之前的历史问题**：业务请求未设进程级并发上限，每个进入的请求都会被受理并处理。虽然底层存储（`velox-config/src/store.rs`）本身是并发安全的，但无限制的并发仍会带来风险：

- 突发流量下，大量写请求同时竞争落盘串行锁（`store.rs` 中 `flush_lock`），推高尾延迟；
- 文件 IO 经 `spawn_blocking` 进入阻塞线程池，过量在途请求会耗尽阻塞线程与 fd/内存资源；
- 上游误配置（如重试风暴）可拖垮单个实例，进而影响依赖它的所有服务。

**目标**：为 velox-config 提供单实例进程级的最大并发请求上限，默认 10、可配置，超限请求立即以明确错误拒绝，防止单实例过载。

**当前基线**：develop 已包含实现 `90268ca` 和单测 `dff2006`：`src/limit.rs` 已采用共享 `Arc<Semaphore>` + `try_acquire_owned`，配置字段、业务路由装配、healthz 豁免、429 错误体与启动上限字段均已存在。当前真实缺口是配置仅校验下界且以 `as usize` 转换（`config.rs:79-91`），未保护信号量技术容量上界；日志源码当前仍缺省 info（`main.rs:30-35`），最终行为须对齐 addlog 默认 warn 裁决。保留既有实现与验证资产，仅修真实缺口；本 feature 范围仅 config 及其文档。

**定位**：这是自我保护（self-protection）手段，不是业务限流或配额管理。

## 2. 功能需求

### FR-1 单实例进程级全局并发上限（默认 10，可配置）

- 作为 velox-config 的运维者，在单实例部署场景下，期望服务对"同时在处理中的请求"设有上限，防止实例被并发压垮。
- 限流粒度为**单实例进程级**：该实例上所有业务路由共享同一个全局计数器；不做分布式/集群级协调，不做按 namespace、按 key、按客户端 IP 的区分。
- 默认上限为 **10**，可通过配置修改（见 FR-4/FR-5）。

### FR-2 并发数口径：同时在处理中的请求数（in-flight）

- "并发数"定义为**同一时刻处于处理中的请求数**：
  - 计入：请求已通过限流层、开始被处理，直到响应完成（对 velox-config 而言响应均为小体积 JSON，响应生成完成即视为处理结束）；
  - 不计入：尚未到达限流层的请求（TCP 未接入/排队于内核）、已被拒绝（429）的请求、已完成响应的请求。
- 口径覆盖范围：默认覆盖全部业务路由 `/api/v1/config*`（GET/PUT/DELETE 均计入，全局共享同一名额）。
- `/healthz` **豁免**：不占用业务名额，业务池饱和时仍正常返回 200 和 `status=ok`；不得为探针放宽业务上限（Q1 已采纳）。

### FR-3 超限行为：立即拒绝，HTTP 429 + 明确错误体

- 当处理中请求数已达上限时，新到请求**立即拒绝，不排队等待**：
  - HTTP 状态码：`429 Too Many Requests`；
  - 响应体：与现有统一错误结构一致（证据：`velox-config/src/error.rs` 的 `{ "code": ..., "message": ... }`），固定 `code = "too_many_requests"`，示例：

    ```json
    { "code": "too_many_requests", "message": "并发请求已达上限 10，请稍后重试" }
    ```

  - `message` 中应包含当前上限值，便于调用方定位；
  - 固定附带 `Retry-After: 1`（Q3 已采纳）；它仅提示客户端重试，不代表服务端排队或承诺 1 秒后必有名额，中文 message 不要求逐字一致。
- 被拒绝的请求不产生任何业务副作用（不读、不写、不删存储，不占用名额）。

### FR-4 配置项命名与归属

- 归属现有 `[server]` 配置节（该节已承载 HTTP 服务行为 `addr`，证据：`velox-config/src/config.rs` 的 `ServerConfig`、`velox-config/config/velox-config.toml`），命名：

  ```toml
  [server]
  addr = "0.0.0.0:8200"
  max_concurrent_requests = 10   # 单实例最大并发处理请求数，合法域见 FR-6
  ```

  Q2 已采纳 `[server].max_concurrent_requests`，不新增配置节。
- 遵循现有配置机制（证据：`velox-config/src/config.rs`）：
  - TOML 文件配置（启动参数指定的配置文件）；
  - 环境变量覆盖，命名遵循现有 `VELOX_CONFIG__<SECTION>__<FIELD>` 模式，即 `VELOX_CONFIG__SERVER__MAX_CONCURRENT_REQUESTS`；
  - 优先级为非空且合法的环境变量 > 文件显式值 > 默认 10；env 未设置或空串不覆盖文件（与 `addr`、`data_dir` 现有空值过滤语义一致）。非空非法 env 必须报错，不得回落到文件/默认值。
- 配置在启动时加载一次，不做热更新（见第 6 节范围外）。

### FR-5 默认值：所有来源均未给值时为 10

- 未提供配置文件，或文件未含该配置项（`serde(default)`），且环境变量未设置或为空串时，使用内置默认值 **10**；
- 文件已显式配置合法值且 env 未设置或为空串时，**保留文件值**，例如文件 5 + 未设 env = 5、文件 5 + 空 env = 5；空 env 不意味着统一回落 10；
- 非空合法 env 覆盖文件，例如文件 5 + env 3 = 3、无文件 + env 3 = 3；非空非法 env 按 FR-6 失败。

### FR-6 非法配置值：启动失败（fail fast）

- 目标平台上的合法配置域为整数 **1..=U**，其中 `U = min(i64::MAX, usize::MAX, tokio::sync::Semaphore::MAX_PERMITS)`。不另设业务封顶，不因值很大而隐式截断；域内的 `1`、`1000`、`100000` 保持合法。
- `0`、负数、非整数/非数字（如 `1.5`、`"abc"`）及超过 U 的值均非法。比较须在无损可表示域中进行，**不得先以 `as usize` 等有损转换再校验**；超出 i64 解析域的输入同样须带原值明确报错，不进入信号量转换/构造链。
- 非法值须在配置加载/校验阶段返回普通、可读的配置错误，在构造限流器及开始监听前终止启动，真实进程非 0 退出。错误包含配置项名（`server.max_concurrent_requests` 或对应 env 全名）与实际原始输入；技术超界错误还须包含目标平台适用的上界 U。
- 不得截断、取模、饱和转换、静默使用默认值，**不得以未捕获 panic 代替合规 fail-fast**。合法值转换后须能安全构造容量一致的限流器。
- 环境变量空串**不是非法值**，沿用 `env_override` 过滤语义，不覆盖文件；文件有值则保留文件，无值才默认 10（FR-5）。文件 TOML 语法或类型错误仍沿用解析失败，不因空 env 消失。
- 配置/初始化失败的可读错误直接通过启动错误通道（stderr）报告，不依赖 INFO 日志启用或过滤器放行。

选择 fail-fast 的理由：与现有非法 `addr` 直接返回错误终止启动的方式一致；配置错误静默回退会使运维误以为配置生效。Q4 已采纳此策略，Q6 的技术上界限定见第 7 节。

## 3. 非功能需求

### NFR-1 低开销

- 限流判断为 O(1) 内存操作（计数器原子增减或信号量 acquire），正常路径（未超限）不得引入可感知延迟；
- 不得在 tokio worker 线程上做阻塞操作，遵循 `velox-config/src/store.rs` 顶部注释引用的开发规范 3.3 节（异步与并发）约束。

### NFR-2 可观测性

- 生效并发上限仍以 **INFO** 启动事件记录（`main.rs:46-52` 已有 `max_concurrent_requests` 字段），不提高为 WARN；**显式开启 info 且过滤器允许对应 target** 时核验该字段，可用隔离环境 `RUST_LOG=info`。
- 默认等级保持 addlog 的 **warn**；默认 warn 下 INFO 启动行静默属预期，应以配置解析结果及受控饱和/恢复行为证明上限。日志观察须遵循 addlog 的 `RUST_LOG > LOG__LEVEL > 文件 > warn` 优先级，使用文件/env 调级时应隔离残留的高优先级 `RUST_LOG`。
- 每次因超限拒绝请求时产生一条 **WARN** 事件，包含当前上限与被拒路径，不采样省略；默认 warn 且未另行屏蔽该 target 时可见。显式 error/off 或 target 过滤掉 WARN 时不可见属过滤预期，不能据此认定拒绝路径未产生日志事件。
- 配置/初始化失败的 stderr 错误不依赖 INFO/WARN 过滤器（FR-6）；不增加诊断端点或修改 addlog 默认等级。

### NFR-3 上限正确性

- 任何时刻，实际处于"处理中"的请求数**不得超过**配置的上限值（计数增减必须并发安全；宁可偶尔少受理，不可超卖名额）。

### NFR-4 兼容性

- 该功能对存量部署是**行为收紧**：升级后未配置即默认 10，原先依赖 >10 并发的调用方将开始收到 429；
- 需在发布说明/CHANGELOG 中明确标注此变更与配置方法。

## 4. 验收标准

> 说明：velox-config 现有 handler 均为快速路径，精确验证并发边界需要控制"处理中"时长（如集成测试中注入延迟、或以大并发突发压测）。具体测试手段由 TA 决定，以下 AC 只约束可判定的外部行为。

### AC-1（对应 FR-1/FR-2/FR-3/NFR-3：上限生效且不超卖）

- Given 服务以 `max_concurrent_requests = 2` 启动
- When 同时维持 2 个处理中的请求，第 3 个请求到达
- Then 前 2 个请求正常处理并成功返回；第 3 个请求收到 429
- And 全程任意时刻处于处理中的请求数 ≤ 2

### AC-2（对应 FR-1/FR-2：全局共享名额，跨路由、跨方法）

- Given 服务以 `max_concurrent_requests = 2` 启动
- When 并发混合请求不同业务路由与方法（如同时打 GET `/api/v1/config/{ns}/{k}` 与 PUT `/api/v1/config/{ns}/{k}`）
- Then 所有路由合计受同一上限约束（在途合计达 2 时，新请求被拒），而非每路由独立计数

### AC-3（对应 FR-3：429 错误体格式）

- Given 服务处于超限状态
- When 新请求被拒绝
- Then 响应状态码为 429，`Content-Type` 为 `application/json`
- And 响应体为 JSON，含字段 `code`（固定值 `too_many_requests`）与非空 `message`，且 `message` 中包含当前上限数值
- And 响应头 `Retry-After` 固定为 `1`（仅重试提示，不保证届时可受理）
- And 结构与现有错误体一致（`{"code": ..., "message": ...}`，见 `velox-config/src/error.rs`）

### AC-4（对应 FR-3：被拒请求无副作用、不占名额）

- Given 服务处于超限状态，1 个请求被 429 拒绝
- When 该被拒请求结束后，新的请求到达且此时在途数已低于上限
- Then 新请求被正常受理（证明被拒请求从未占用名额）
- And 被拒请求对应的存储数据保持不变（无读写副作用）

### AC-5（对应 FR-4/FR-5：默认值与配置优先级）

- Given 以下启动方式依次验证，除列明的 env 外其余上限 env 均未设置
  1. 不提供配置文件启动；
  2. 提供配置文件但无 `max_concurrent_requests` 键；
  3. 仅设置环境变量 `VELOX_CONFIG__SERVER__MAX_CONCURRENT_REQUESTS=3`；
  4. 配置文件写 `max_concurrent_requests = 5` 且环境变量为 3；
  5. 配置文件写 5，环境变量未设置；
  6. 配置文件写 5，环境变量为空串；
  7. 无文件/文件缺键，环境变量为空串；
- When 分别加载配置并启动，按相应 N 个受控在途请求占满，再请求命中业务路由
- Then 情况 1、2 的生效上限为 10，情况 3、4 为 3（保留原四种容量期望 10/10/3/3）；情况 5、6 为 5，情况 7 为 10；各组第 N+1 个业务请求在持有者释放前收到 429，释放后可再次受理
- And 日志作为上限证据时须显式 info 且允许对应启动 target（如隔离环境 `RUST_LOG=info`）；默认 warn 场景通过配置及上述饱和行为验证，INFO 启动行缺失不表示配置失效

### AC-6（对应 FR-6：非法值 fail fast）

- Given 文件/环境变量分别提供 `0`、`-1`、`1.5`、`"abc"`（env 用 `abc`），以及技术上界 U+1、超出 i64 解析域的整数原值
- When 分别以这些输入启动
- Then 配置加载/校验返回普通可读错误，进程均非 0 退出，在构造限流器/开始监听前终止；错误含配置项名与实际原始值，超技术上界的错误还含适用 U，不依赖 INFO 开启，无未捕获 panic、截断或静默回退
- And U 按目标平台 `min(i64::MAX, usize::MAX, tokio::sync::Semaphore::MAX_PERMITS)` 无损计算；`i64::MAX` 输入依该域判定（当前锁定 Tokio 的 32/64 位平台均超 U，须普通配置错误）
- And 合法值 `1`、`1000`、域内 `100000` 和 U 均能正常加载并安全构造容量一致的限流器；U 的验收只核对构造/容量，无需发起 U 个请求；超 i64 解析域的失败也保留原值上下文
- And 空 env 按 FR-5/AC-5 的“不覆盖”路径成功，不纳入非法值集合

### AC-7（对应 NFR-2：可观测性）

- Given 服务以任意合法上限启动，显式 info 且允许启动 target（如隔离环境 `RUST_LOG=info`）
- Then INFO 启动记录包含生效并发上限值
- Given 使用 addlog 默认 warn，且无额外 target 屏蔽，When 启动并触发 429
- Then INFO 启动行静默属预期；每次 429 仍产生一条可见 WARN，含上限与被拒路径
- And 显式 error/off 或 target 过滤 WARN 时，不要求 WARN 可见；每拒一条 WARN 的事件生成义务不变；配置/初始化错误仍由不受 INFO 过滤影响的 stderr 通道报告

## 5. 边界与异常场景

| 场景 | 预期行为 |
|---|---|
| 恰好等于上限（在途 = N） | 新请求拒绝；在途请求不受影响，正常完成 |
| 在途请求处理失败（如 500） | 无论成功失败，响应结束后即释放名额 |
| 优雅关停（Ctrl-C，`main.rs` 的 `shutdown_signal`） | 在途请求继续处理完，限流计数随之释放；不因限流层阻碍关停 |
| 被拒请求的耗时 | 立即返回，不做任何存储访问（甚至不应执行 handler，在限流层即拒绝） |
| 极大值与技术上界 | 域内值（如 100000）接受并生效，不设额外业务封顶；合法域为 1..=U（FR-6）。U+1/超解析域输入在配置阶段以含字段/原值/上界的普通错误退出，不得让构造 panic |
| 配置文件 TOML 语法错误 | 沿用现有行为：启动失败（`config.rs` 解析报错），与本需求无关 |
| 环境变量传非数字 | 视同 FR-6 非法值，启动失败 |
| 并发竞态（多个请求同时到达边界） | 计数增减原子，可能出现"恰好差 1 个名额时判满"的边界取舍，允许轻微保守（少受理），禁止超卖 |
| healthz 在超限时被访问 | 固定豁免，不占业务名额，仍返回 200 和 `{"status":"ok",...}`；业务池继续按原上限拒绝请求 |

## 6. 范围外

明确**不做**以下内容，防止范围蔓延：

- 分布式/集群级限流（多实例全局协调、基于 registry 的配额同步）——本需求仅单实例进程级；
- 基于速率的限流（如令牌桶 N req/s）——本需求只限并发数，不限速率；
- 排队等待机制（超限请求在服务内等待空位）——超限即拒绝；
- 按 namespace / key / 客户端 IP / API key 的差异化限额；
- 配置热更新（运行中修改上限需重启生效）；
- Prometheus 等指标上报体系（仅有日志，见 NFR-2）；
- 新的 `/metrics` 或管理端点。

## 7. 待确认问题（关口裁决已回填，未决 0 项）

以下六项保留问题编号以便追溯，均按 `01-requirements-review.md` §5～§7 裁决处置；不是继续留给 AA/DA 选择的开放问题，仍需 GATE1 复审核对回填。

| # | 原问题 | 本轮采纳的决议与落点 |
|---|---|---|
| Q1 | `/healthz` 是否豁免限流？ | **豁免**，不占业务名额，饱和时仍 200；FR-2、§5 |
| Q2 | 配置项归属 `[server]` 还是新设 `[limits]`？ | **`[server].max_concurrent_requests`**，保留既有 env 全名及 env > 文件 > 默认优先级；FR-4/FR-5 |
| Q3 | 429 code 与 Retry-After？ | **`too_many_requests` + `Retry-After: 1`**，message 含上限，重试提示不代表服务端排队/精确可用时间；FR-3/AC-3 |
| Q4 | 非法配置处理？ | **fail-fast**，含技术超界：配置阶段普通可读错误、真实启动非 0 退出，不以 panic 代替；空 env 是不覆盖而非非法值；FR-6/AC-6 |
| Q5 | 是否接受默认 10 的行为收紧？ | **接受**；保留 README/发布说明及调优指引义务，不新增指标体系；NFR-4 |
| Q6 | 是否封顶？ | **不设额外业务封顶，受技术容量约束**：整数 1..=U，`U = min(i64::MAX, usize::MAX, tokio::sync::Semaphore::MAX_PERMITS)`，超界错误含原值/上界，不截断或静默默认；FR-6/AC-6/§5 |

## 8. 影响面

以下为 develop 当前已存在的实现与后续最小修复范围，不再将已交付项描述为待新增；本轮未改这些代码/README/用例。

| 模块/文件 | 已有事实及后续影响 |
|---|---|
| `velox-config/src/config.rs:12`、`:23`、`:67`、`:79`、`:90` | 已有默认 10 的 i64 字段、文件后 env 覆盖与校验；当前仅查 <1，访问器 `as usize`。真实缺口：补 FR-6 技术上界及无损转换、超界可读错误；保留默认/非空 env 优先及空 env 不覆盖语义 |
| `velox-config/src/limit.rs:14`、`:20`、`:40` | 已有 Clone 共享 `Arc<Semaphore>`、try_acquire_owned 非等待入口、permit RAII 和每拒 WARN；保留实现，构造容量须由配置安全保证，不机械重写中间件 |
| `velox-config/src/api.rs:18`、`:27` | 已有 `limit_routes`、`router(store,max)`，全部业务共用同一个 limit，healthz 在层外；保留装配与 handler，不改 Store |
| `velox-config/src/error.rs:23` | 已有 TooManyRequests → 429、JSON code/message、Retry-After:1；保留错误契约 |
| `velox-config/src/main.rs:30`、`:46` | 已有启动 INFO 上限字段；当前源码缺省 info，addlog 负责默认 warn 改造，本 feature 仅按 NFR-2/AC-5/7 明确观察条件，校验错误须在监听/限流器构造前返回 |
| `velox-config/config/velox-config.toml:5`、`velox-config/README.md:40` | 已有默认10、env、429/Retry-After/healthz及升级收紧说明；后续只对齐合法技术域、空env、需重启与日志核验前提，保留发布说明义务 |
| `velox-config/Cargo.toml:21` | 已有 uuid/tower 的 workspace dev-dependencies；不要求新增运行时 tower 或替换当前方案 |
| `velox-config/src/config.rs`、`src/limit.rs`、`src/api.rs` 内嵌 tests；本目录 `03-test-cases.md` | 复用既有默认/env/非法值、门闩并发、错误体、豁免及装配测试；补容量边界与安全构造、文件5+空env、显式info/默认warn观察。共享计数测试的超限请求需命中已注册方法；405 行为待独立核验，不能把静态疑点当已复现缺陷 |

**实现约束与提示**：保留当前 `Semaphore::try_acquire_owned` 非等待入口。Tower `ConcurrencyLimitLayer` 单独使用会在 `poll_ready` 等待名额，不会天然产生 429，不能直接满足“满额立即拒绝”；本 feature 不切换为该层。依据为本机锁定依赖 `tower-0.5.3/src/limit/concurrency/service.rs:64-78`（`poll_acquire`），技术容量契约见 `tokio-1.54.1/src/sync/semaphore.rs:479-485`。后续 AA/DA 只对齐本轮决议、修真实缺口，TA 重核现有 7 条 AC 的矩阵，均不得以规格返工为由机械重写已实现代码。

---
*证据基准：2026-11-26，只读核对 develop @ 28c9114 的当前源码、配置、README、既有单测，以及本目录 GATE1 评审的 G1-1～5/RQ-1～2；历史无并发上限背景仅指 90268ca 之前。本轮未运行 cargo/e2e、未执行构造 panic 复现，不宣称实现或验收通过。*
