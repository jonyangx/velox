# 需求分析：velox-config 单实例最大并发请求限流

> feature slug: `instance-concurrency-limit`
> 状态：待评审（含 6 个待确认问题，见第 7 节）

## 1. 背景与目标

velox-config 是基于 axum + tokio 的配置中心 HTTP 服务（`velox-config/src/main.rs`），当前对所有请求**不设任何并发上限**：每个进入的请求都会被受理并处理。虽然底层存储（`velox-config/src/store.rs`）本身是并发安全的，但无限制的并发仍会带来风险：

- 突发流量下，大量写请求同时竞争落盘串行锁（`store.rs` 中 `flush_lock`），推高尾延迟；
- 文件 IO 经 `spawn_blocking` 进入阻塞线程池，过量在途请求会耗尽阻塞线程与 fd/内存资源；
- 上游误配置（如重试风暴）可拖垮单个实例，进而影响依赖它的所有服务。

**目标**：为 velox-config 增加单实例进程级的最大并发请求上限，默认 10、可配置，超限请求立即以明确错误拒绝，防止单实例过载。

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
- `/healthz` 是否计入见待确认问题 Q1（当前倾向：豁免）。

### FR-3 超限行为：立即拒绝，HTTP 429 + 明确错误体

- 当处理中请求数已达上限时，新到请求**立即拒绝，不排队等待**：
  - HTTP 状态码：`429 Too Many Requests`；
  - 响应体：与现有统一错误结构一致（证据：`velox-config/src/error.rs` 的 `{ "code": ..., "message": ... }`），建议：

    ```json
    { "code": "too_many_requests", "message": "并发请求已达上限 10，请稍后重试" }
    ```

  - `message` 中应包含当前上限值，便于调用方定位；
  - 是否附带 `Retry-After` 响应头见待确认问题 Q3。
- 被拒绝的请求不产生任何业务副作用（不读、不写、不删存储，不占用名额）。

### FR-4 配置项命名与归属

- 建议归属现有 `[server]` 配置节（该节已承载 HTTP 服务行为 `addr`，证据：`velox-config/src/config.rs` 的 `ServerConfig`、`velox-config/config/velox-config.toml`），命名：

  ```toml
  [server]
  addr = "0.0.0.0:8200"
  max_concurrent_requests = 10   # 单实例最大并发处理请求数，≥1
  ```

  归属 `[server]` 还是新设 `[limits]` 节见待确认问题 Q2。
- 遵循现有配置机制（证据：`velox-config/src/config.rs`）：
  - TOML 文件配置（启动参数指定的配置文件）；
  - 环境变量覆盖，命名遵循现有 `VELOX_CONFIG__<SECTION>__<FIELD>` 模式，即 `VELOX_CONFIG__SERVER__MAX_CONCURRENT_REQUESTS`；
  - 同名配置同时存在时，环境变量优先（与 `addr`、`data_dir` 现有覆盖语义一致）。
- 配置在启动时加载一次，不做热更新（见第 6 节范围外）。

### FR-5 默认值：未配置时为 10

以下三种情况均取默认值 10：

1. 未提供配置文件（现有 `Config::load(None)` 走全默认路径，证据：`config.rs`）；
2. 提供了配置文件但文件中无该配置项（`serde(default)` 模式）；
3. 环境变量未设置。

### FR-6 非法配置值：启动失败（fail fast）

以下取值视为非法，进程**启动失败**并以非 0 退出码退出，错误信息需包含配置项名称与实际读到的值：

- `0`：语义上等于"拒绝一切请求"，几乎必然是误配置，不允许；
- 负数（如 `-1`）：无意义；
- 非整数 / 非数字（如 `1.5`、`"abc"`）：解析失败；
- 空字符串经环境变量传入：现有 `env_override` 会过滤空值回落默认值（证据：`config.rs` 的 `filter(|v| !v.is_empty())`），保持该语义。

选择 fail fast 而非"静默回落默认值 + 告警"的理由：与现有非法 `addr` 的处理方式一致（证据：`main.rs` 中 `"非法监听地址 '{}'"` 直接返回错误终止启动）；限流值错误属于配置错误，静默降级会让运维误以为配置生效。

## 3. 非功能需求

### NFR-1 低开销

- 限流判断为 O(1) 内存操作（计数器原子增减或信号量 acquire），正常路径（未超限）不得引入可感知延迟；
- 不得在 tokio worker 线程上做阻塞操作，遵循 `velox-config/src/store.rs` 顶部注释引用的开发规范 3.3 节（异步与并发）约束。

### NFR-2 可观测性

- 启动时在日志中输出生效的并发上限值（现有启动日志已有 `version/addr/data_dir` 字段，证据：`main.rs`，新增上限字段即可）；
- 每次因超限拒绝请求时输出 `warn` 级别日志，包含当前上限与被拒路径；日志频率与字段细节由 DA 定。

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
- And 响应体为 JSON，含字段 `code`（值为 `too_many_requests`，若 Q3 另有决策则以决策为准）与非空 `message`，且 `message` 中包含当前上限数值
- And 结构与现有错误体一致（`{"code": ..., "message": ...}`，见 `velox-config/src/error.rs`）

### AC-4（对应 FR-3：被拒请求无副作用、不占名额）

- Given 服务处于超限状态，1 个请求被 429 拒绝
- When 该被拒请求结束后，新的请求到达且此时在途数已低于上限
- Then 新请求被正常受理（证明被拒请求从未占用名额）
- And 被拒请求对应的存储数据保持不变（无读写副作用）

### AC-5（对应 FR-4/FR-5：默认值与配置优先级）

- Given 以下四种启动方式依次验证
  1. 不提供配置文件启动；
  2. 提供配置文件但无 `max_concurrent_requests` 键；
  3. 仅设置环境变量 `VELOX_CONFIG__SERVER__MAX_CONCURRENT_REQUESTS=3`；
  4. 配置文件写 `max_concurrent_requests = 5` 且环境变量为 3；
- Then 情况 1、2 的生效上限为 10（启动日志可证实）；情况 3、4 的生效上限为 3（环境变量优先）

### AC-6（对应 FR-6：非法值 fail fast）

- Given 配置中 `max_concurrent_requests` 分别取 `0`、`-1`、`1.5`、`"abc"`
- When 分别以这些配置启动
- Then 每次进程均以非 0 退出码退出，错误信息包含配置项名（`max_concurrent_requests`）与实际读到的值
- And 合法值 `1`、`1000` 可正常启动并生效

### AC-7（对应 NFR-2：可观测性）

- Given 服务以任意合法上限启动
- Then 启动日志包含生效的并发上限值
- And 每次触发 429 时产生一条 `warn` 级别日志

## 5. 边界与异常场景

| 场景 | 预期行为 |
|---|---|
| 恰好等于上限（在途 = N） | 新请求拒绝；在途请求不受影响，正常完成 |
| 在途请求处理失败（如 500） | 无论成功失败，响应结束后即释放名额 |
| 优雅关停（Ctrl-C，`main.rs` 的 `shutdown_signal`） | 在途请求继续处理完，限流计数随之释放；不因限流层阻碍关停 |
| 被拒请求的耗时 | 立即返回，不做任何存储访问（甚至不应执行 handler，在限流层即拒绝） |
| 极大合法值（如 100000） | 接受并生效；不设上限封顶（见待确认 Q6 中是否需要封顶的讨论） |
| 配置文件 TOML 语法错误 | 沿用现有行为：启动失败（`config.rs` 解析报错），与本需求无关 |
| 环境变量传非数字 | 视同 FR-6 非法值，启动失败 |
| 并发竞态（多个请求同时到达边界） | 计数增减原子，可能出现"恰好差 1 个名额时判满"的边界取舍，允许轻微保守（少受理），禁止超卖 |
| healthz 在超限时被访问 | 取决于 Q1 决策：豁免则正常返回 `{"status":"ok",...}`；不豁免则可能收到 429 |

## 6. 范围外

明确**不做**以下内容，防止范围蔓延：

- 分布式/集群级限流（多实例全局协调、基于 registry 的配额同步）——本需求仅单实例进程级；
- 基于速率的限流（如令牌桶 N req/s）——本需求只限并发数，不限速率；
- 排队等待机制（超限请求在服务内等待空位）——超限即拒绝；
- 按 namespace / key / 客户端 IP / API key 的差异化限额；
- 配置热更新（运行中修改上限需重启生效）；
- Prometheus 等指标上报体系（仅有日志，见 NFR-2）；
- 新的 `/metrics` 或管理端点。

## 7. 待确认问题

| # | 问题 | 建议默认方案（供上游决策） |
|---|---|---|
| Q1 | `/healthz` 是否豁免限流？ | **豁免**。理由：healthz 是存活探针，过载时恰恰最需要它可用；豁免可使编排系统在过载时正确观测实例状态。代价是极端情况下 healthz 仍会占用少量处理能力，可接受 |
| Q2 | 配置项归属 `[server]` 还是新设 `[limits]` 节？ | **`[server].max_concurrent_requests`**。理由：这是单实例 HTTP 服务器自身行为，`[server]` 节已存在且语义契合；避免为单个配置项新建节。若后续规划更多限流类配置（速率、连接数），再考虑 `[limits]` |
| Q3 | 429 错误体 `code` 取值？是否附带 `Retry-After` 头？ | `code = "too_many_requests"`（与现有错误码命名风格一致，如 `config_not_found`、`invalid_namespace`，见 `error.rs`）；**附带 `Retry-After: 1`**，提示调用方 1 秒后可重试（并发限流下无精确等待时间，取短固定值即可） |
| Q4 | 非法配置值处理：启动失败还是静默回落默认 + 告警？ | **启动失败（fail fast）**，与现有非法 `addr` 处理一致（`main.rs`）。限流值配置错误若静默降级，运维会误以为配置已生效 |
| Q5 | 默认 10 对存量部署是行为收紧，是否接受？ | **接受**（需求明确要求默认 10），在发布说明中标注变更，并给出调优指引（如何评估与调大） |
| Q6 | 是否需要为上限设置合理最大值封顶（防误配超大值失去保护意义）？ | **不封顶**。保持配置语义简单；误配超大值等效于"不限流"，属于运维责任，不做隐式截断（隐式截断会让实际行为与配置不符，更难排查） |

## 8. 影响面

| 模块/文件 | 影响 |
|---|---|
| `velox-config/src/config.rs` | `ServerConfig` 新增 `max_concurrent_requests` 字段（`serde(default)` 默认 10）、对应环境变量覆盖、启动校验（≥1） |
| `velox-config/src/api.rs` | `router()` 组装限流层（tower middleware / 自定义 layer），拒绝路径生成 429 响应；函数签名可能需传入上限值 |
| `velox-config/src/error.rs` | 若复用统一错误映射，需补充限流错误到 429 的映射（限流发生在 handler 之前，也可能由中间件直接构造响应，由 DA 定） |
| `velox-config/src/main.rs` | 启动日志增加生效上限字段；校验失败的启动报错 |
| `velox-config/config/velox-config.toml` | 增加示例配置项（默认 10） |
| `velox-config/Cargo.toml` | 可能新增 `tower.workspace = true`（workspace 已统一管理 tower 0.5 + util feature，证据：根 `Cargo.toml` `[workspace.dependencies]`；velox-config 当前未直接依赖 tower） |
| 测试 | `config.rs` 校验逻辑单测；api 层并发超限集成测试（AC-1～AC-4） |

**实现提示（非约束）**：workspace 中 `velox-registry` 等已使用 tower；tower 0.5 自带 `ConcurrencyLimitLayer` 可直接满足"超限即拒绝"语义，但需注意其错误类型到 429 响应的转换。实现选型由 DA 决定，本需求只约束第 2～4 节的外部行为。

---
*证据基准：本文所有"现有行为"描述均基于 2026-02 调研时 `velox-config` 源码（main.rs / api.rs / config.rs / error.rs / store.rs）与根 `Cargo.toml` 实际内容。*
