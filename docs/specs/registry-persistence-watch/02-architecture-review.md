# 架构关口评审：velox-registry v0.2 状态持久化与 watch

> Feature：`registry-persistence-watch`；工序：F-GATE2（AA / TA-D join，非终验）。
> 任务：`QA-1795655544755-d75197a323c52e10`。
> 取证基准：`develop @ 28c9114`。四份输入文档已提交于该提交；`git diff dff2006..HEAD -- velox-registry velox-config/src/store.rs` 为空，架构引用的这些代码仍适用。
> 本报告之外未修改任何文件；未执行构建、测试或启动服务。下列推演是设计反例，不是已实现功能的实跑测试结果。
> 引用约定：R=`01-requirements.md`，G1=`01-requirements-review.md`，A=`02-architecture.md`，T=`03-test-cases.md`，均位于本目录；代码路径相对项目根。

## 1. 结论

**驳回，GATE2 REWORK。**

总体方案可行，落盘格式、版本批次、复活心跳通知及 500 `persist_failed` 已有明确决定。但仍有阻塞交付设计的问题：失败刷盘会丢失重试标记；缺省 index 分支自相矛盾；关闭持久化仍可能被跨配置校验拒绝；默认快照会进入 velox-config 的 namespace 扫描；AC-13 基线被替换；测试用例有错误前置条件、放宽验收阈值和越界脚本计划。另需**退回 SA 定向修订 FR-2 的无误剔除保证**，不能用 `flush < ttl` 证明该保证。

本报告不推翻 GATE1 的范围与总体方向。GATE2 对确定后的具体算法与 AA/TA 交叉结果提出以下返工项；不能把 GATE1 PASS 当成这些实现细节的正确性证明。

## 2. 七项检查结论

| 检查项 | 结论 | 证据与影响 |
|---|---|---|
| 逐 FR/AC 设计覆盖、DA 无需再定架构 | 部分满足，未过关 | 见第 3 节；G2-1～G2-6 须先解决，不能让 DA 在相互矛盾的规则间自行选择 |
| 代码现状匹配 | 主体满足，存储边界不满足 | 五个 registry 文件与 config 写穿参照均已读；A §2.2 正确纠正原子写出处，但 A:169 的共目录结论与 config 扫描实现冲突（G2-5） |
| 备选方案及理由 | 主体成立，部分论证须修正 | A §7 有 WAL/多文件/嵌入式库/长连接/锁外版本等取舍；千实例选择全量快照合理。不能宣称所有 IO 都每 5s 一次，也不能将零新增依赖“预期”描述为绝对禁止任何新依赖 |
| AC×TC 可测性、B-1…12 处置 | 有映射，尚非完整有效覆盖 | 每个 AC 都有 TC 编号，但 TC-08/19/26 不可按当前条件执行，TC-23 不能证明默认 30s，TC-21 放宽门槛；见第 5、7 节 |
| RQ-1…3 仲裁 | 已逐项给出处置 | RQ-1 核销；RQ-2 定为验收期性能专项，保留原 AC；RQ-3 存在矛盾定义，退 AA 修正，见第 6 节 |
| TC 期望回填 | 必须回填 | 第 7 节逐 TC 列出现状、应改值及责任，不修改 T 文件 |
| 范围约束 | AA 主体遵守，TA 计划越界 | A 页首只改 registry 及文档；T:13 计划扩展 `velox-demo/scripts/acceptance.sh` 与 R §6.1 冲突。只读运行已有 demo 可行，新测试脚本放 registry 内 |

## 3. FR 与 AC 实现路径核对

### 3.1 FR 逐项

| FR | 架构实现路径 | 评审结论 |
|---|---|---|
| FR-1 | A §4.2/§5，`RegistrySnapshot` 全量单文件，register/deregister 响应前 flush，sweep 后 flush | 路径明确；须统一串行锁覆盖范围（G2-1），隔离 config namespace（G2-5） |
| FR-2 | A §4.3 `dirty: AtomicBool`、`take_dirty()`；§4.4 周期任务 | 不满足已宣称的失败后自动重试；无误剔除保证不成立（G2-1/4） |
| FR-3 | A §4.4 步骤 3 在 bind 前恢复，UUID 原样保存 | 路径明确；恢复成功前提须与持久化心跳陈旧度一致（G2-4） |
| FR-4 | A §4.3/§5.2，绝对时间 age、严格 `age > ttl` 丢弃，本次配置 TTL | 路径明确，可构造 epoch 单测；TC-08 必须修复夹具及配置 |
| FR-5 | A §4.5 配置、env、集中 validate；store=None 禁用 | 缺禁用时跨项校验的适用条件；部分 TOML 节的默认值注解也须写清（G2-3） |
| FR-6 | A §4.2 `Missing/Corrupted/Restored`；建目录失败 fail fast，损坏整文件跳过 | 路径明确；文件缺失与其他读取错误分类已给出，格式版本不识别也已定案 |
| FR-7 | A §4.6/§4.7，内存先变、写失败 500 `persist_failed`，日志 ERROR，心跳不 await IO | HTTP 决策明确，RQ-1 核销；后台失败重试仍欠缺（G2-1） |
| FR-8 | A §4.3 同锁 `(version, snapshot)`，先 subscribe 后读；§4.7 三路异步等待含 shutdown | 主体可行，AC-9 有主动唤醒路径；缺省 index 在版本 0 下矛盾（G2-2） |
| FR-9 | A §4.3 规则表：注册/注销/一次 sweep/复活心跳各 +1，普通心跳与查询不增 | 已定案，G1 QA-2/3 吸收；T 的 TC-29 仍只认三种 bump，需回填 |
| FR-10 | A §4.6/§4.7，默认 30、范围 1..=300，400 `invalid_parameter` | 范围错误明确；提取器解析失败如何进入统一 JSON 错误尚缺路径（G2-2） |
| FR-11 | A §4.6 保留 404 code，§4.7 既有 HTTP 端点不动，§6.2 compat 与 mesh 原拉取序列 | 接口兼容路径明确；禁用持久化的配置兼容性仍待 G2-3；所谓既有测试“原样保留”应指断言保留，router 构造须适配 AppState |

### 3.2 AC×TC 可测性交叉检查

| AC | 架构落位 / T 中映射 | 可测性判定 |
|---|---|---|
| AC-1 | A §4.2/4.4/5；TC-01/03/04 | 可测，真实 kill/restart 保留 e2e，前提是持久化心跳仍在恢复 TTL 内；不能仅以 drop/open 代替信号级验证 |
| AC-2 | A §4.3 restored；TC-02 | 可测，恢复后同 id 发心跳；与 AC-1 一并补前提（G2-4） |
| AC-3 | A §4.3 心跳无 flush await、§4.4 flusher；TC-05/06 | 可测，但须控制 tick 相位及 epoch 秒精度，补失败后下一周期重试用例 |
| AC-4 | A §5.2；TC-07/08/09 | 可测，TC-08 当前“B=now−60 却 TTL30 应恢复”错误；不是覆盖矩阵写了编号就有效 |
| AC-5 | A §4.4/4.5；TC-10/11/12 | 可测，首次空启动不保证文件已写出；补禁用持久化且 TTL 小于默认 flush 的兼容分支 |
| AC-6 | A §4.2 load；TC-13 | 可测；保持三类损坏逐场景与文件路径日志断言，补不识别 format_version |
| AC-7 | A §4.6/4.7；TC-16/17 | 可测，固定为 500 `persist_failed`；先正常 open 再注入写失败，避免把启动目录权限失败误测成运行期写失败 |
| AC-8 | A §4.7 subscribe-before-read；TC-20/21/25 | 可测，严格保留 ≤1s，不能以 ≤2s 算通过；补版本 0 缺省 index 分支（G2-2） |
| AC-9 | A §4.4/4.7 shutdown rx；TC-30 | 有集成可注入路径及 SIGINT 真进程路径；两者保留，集成验证不能替代进程退出结果 |
| AC-10 | A §4.3 sweep 批次 +1/查询不增；TC-26/27/28 | 可测，TC-26 须配置 flush=1、TTL=2；补一次 sweep 多实例整体 +1 |
| AC-11 | A §4.7 默认常量/参数解析；TC-22/23/24 | 可测，T 的 3s 挂起只能说明未在 3s 内返回，须加默认值精确为 30 的单测 |
| AC-12 | A §4.6/4.7/6.2 compat；TC-35/36/37 | 可测，保留 mesh 原拉取及已有 demo 只读回归；不修改消费侧或 demo 脚本 |
| AC-13 | A:37/431；TC-38 | 未闭合：v0.2 开/关持久化不是原 v0.1 基线；按第 6 节性能专项回填，现阶段不得判 AC 已满足 |
| AC-14 | A §4.2 temp+sync_all+rename、§4.4 日志、§6.4 kill；TC-18/39 | 可测但证据不足：残留 tmp 模拟不等于实跑写入中途 kill；补独立崩溃注入项与跳过损坏文件日志场景 |

## 4. 现状代码抽查与 GATE1 问题吸收

### 4.1 只读代码证据

| 文件 | 实际证据 | 与架构比对 |
|---|---|---|
| `velox-registry/src/registry.rs:26` / `:44` | `last_heartbeat: Instant`；嵌套 HashMap + RwLock；`:163` 严格 `elapsed > ttl` 剔除 | A §2.1 正确；内存保留 Instant 有充分兼容理由 |
| `velox-registry/src/api.rs:16` | router 接收 `Arc<Registry>`；`:18` 起五个路由路径承载六个方法端点；模块内有 oneshot 测试 | A 的“六条 API”按方法计数成立；AppState 是计划改造而非现状；测试 setup 须适配新签名 |
| `velox-registry/src/config.rs:57` / `:67` | env 解析 u64，load 直接 Ok，无集中 validate | A §2.1 正确；旧 TTL=1 配置可被接受，并非已有约束禁止 |
| `velox-registry/src/main.rs:43` / `:97` | shutdown watch 广播；信号仅 `tokio::signal::ctrl_c()` | A §2.1/4.4 符合；TC-19 的 SIGTERM 没有优雅退出接入依据 |
| `velox-registry/src/error.rs:23` / `:28` | 404 `instance_not_found`/`service_not_found` 与 JSON code/message | A §4.6 明确保留了既有映射 |
| `velox-config/src/store.rs:164` | flush_lock 持有期间读取快照，并 await spawn_blocking；`:172` 为直接 `std::fs::write` | A §2.2 正确，原子写确为 registry 新增；A §4.8 “锁外写文件”不得被实施为提前释放 flush_lock |
| `velox-config/src/store.rs:43` / `:61` | namespace 允许字母数字、`-`、`_`；open 扫描 data_dir 全部 `.json`，文件 stem 为 namespace，解析为键→JSON 值 map | 直接反证 A:169/R Q6 的“registry-state.json 与 config 共目录无冲突”假设 |
| `velox-config/src/config.rs:78` | load 末尾调用集中 validate | A “参照 config 集中校验”属实 |
| `Cargo.toml:20` / `velox-registry/Cargo.toml:22` | tokio features=[full]；已有 tower dev-dep；无 lib target/src/lib.rs | 零新增 crate 路线可行；但 T 的 start_paused 不能由 full 推出，跨文件集成测试需要明确模块可见性入口 |

`full` 不包含 `test-util`，已只读核对本机缓存 `tokio-1.54.1/Cargo.toml` 的 features 列表；即使启用暂停 Tokio 时间，也不会推进领域层的 `std::time::Instant`。测试方案须选择真实短时等待、固定 epoch/可控领域时间，或在 registry 的 dev-dependency 中显式启用已有 tokio 的 test-util；不能写成当前基建已支持。

### 4.2 QA-1…QA-6 吸收判定

| GATE1 编号 | GATE2 判定 |
|---|---|
| QA-1 原子写出处 | 已吸收，A §2.2 明确 config 直写、registry 新增原子替换 |
| QA-2 批量版本语义 | 已吸收，A §4.3 一次 sweep 整批 +1；TA 须补多实例批次断言 |
| QA-3 复活心跳 | 已吸收，A §4.3 unhealthy→healthy +1；TA TC-29 未同步 |
| QA-4 落盘串行时序 | 主方案已吸收，但 A §4.8 描述与之矛盾，须修订为串行锁持续覆盖整个写入完成（G2-1） |
| QA-5 量化 | 部分吸收；`flush < ttl` 不能证明无误剔除；“每周期至多一次全量写”只能限定后台心跳合并刷盘，不能覆盖即时写穿（G2-4） |
| QA-6 错误码 | 已吸收，A §4.6/§9 定案 500 `persist_failed`，RQ-1 核销 |

## 5. B-1…B-12 架构处置核对

| 边界 | 架构处置与测试 | 结论 |
|---|---|---|
| B-1 落盘失败 | A §4.6/5.1，内存保留、500/ERROR；TC-16/17 | 错误响应有路径，但脏刷失败重试漏失，G2-1 |
| B-2 损坏/半写 | A §4.2/5.2，Corrupted 整文件跳过、open 清 tmp；TC-13/18 | 满足设计方向；TC-18 清理时机须改为 open，真 kill 注入另留证据 |
| B-3 目录不可建 | A §4.2 open Err 含路径；TC-15 | 满足 |
| B-4 宕机超过 TTL | A §5.2 age>ttl 丢弃；TC-09 | 满足，但不能反推宕机小于 TTL 就一定可恢复，G2-4 |
| B-5 provider 无重注册 | A 保留原 UUID/丢弃超时实例、范围红线；TC-09 记录 404 | 满足；README 继续明示丢弃后需外部重注册 |
| B-6 watch 断开 | A §4.7 每请求独立，无订阅会话；TC-31 | 满足设计方向，可中止 future 再重连验证 |
| B-7 版本回落 | A §4.7 index!=version 立即快照；TC-25 | 显式过大 index 满足；缺省 index 是另一分支，不得混为一谈 |
| B-8 关停竞态 | A §4.4 步骤 8/9、§4.7 shutdown 分支；TC-19/30 | 主体满足，TC-19 信号与配置均需回填 |
| B-9 时钟回拨 | A §4.3/§8 R3，接受限制，README 声明 | 满足约定；TC-34 不应把所有墙钟时间戳变化都等同旧快照覆盖 |
| B-10 多进程目录 | A §4.2 单 registry 独占、无需分布式锁 | 两 registry 的约束明确；与 config 同目录仍不安全，G2-5 |
| B-11 并发 | A §3.2 同锁版本快照、flush_lock 内取快照、subscribe-before-read；TC-32/33/34 | 基本成立，须修正 G2-1 并补结构变更与周期刷盘竞争 |
| B-12 字符串/metadata | A DTO serde JSON、实例沿用原字段；TC-41 | 满足；不引入服务名新校验 |

## 6. RQ 仲裁

### RQ-1：核销

依据 A §4.6、§4.7、§9：注册/注销内存变更后写穿失败，固定 **HTTP 500 + `code=persist_failed`**；响应 message 为 `registry state persist failed`，内部 IO 细节仅进 ERROR 日志。TC-16 去除“建议默认/待定/5xx”并精确断言；注销同样补断言。核销的是规格不确定性，不代表 AC-7 已实跑通过。

### RQ-2：验收期性能专项，保留 AC-13，不自动豁免

允许 TA 在 `velox-registry/` 下实现轻量测量入口或脚本，复用已有依赖/客户端。作为验收期专项，不要求常规 CI 每次压测；不能将“不进 CI”改成“可不验收”。

- 固定真正的 v0.1 基线：可选本次评审 `28c9114`（registry 仍是 v0.1），报告记录准确 commit；使用隔离构建目录/检出，不改当前工作分支。v0.2 `enabled=false` 仅作辅助对照。
- 在同一机器、同一 release profile、相同日志级别/请求体/客户端并发与负载计划下比较。规模维持 1000 实例，5s 心跳；注册 churn 应伴随注销，避免超出规模承诺。
- 执行前冻结参数：服务数、每种请求速率与并发、预热和采样时长、样本量、延迟计时点、P99 算法及错误率。建议预热至少 30s、采样至少 60s，并延长至每种被比较端点有至少 1000 个有效样本；全部错误另计，不得靠丢失败样本改善 P99。
- 分别报告注册/心跳/查询的 P99、v0.2/v0.1 比值及原始记录位置；每项 ≤2 才满足 AC-13。无测量、错误过多或样本不足，标为“无法验证/遗留问题”，不得标 AC 通过。

A:431 用“开/关持久化”替换“v0.1 同条件基线”，须返 AA；T TC-38 保留真正 v0.1 基线并补测量规范。当前 RQ-2 **已裁定执行方式，证据待执行阶段交付**。无需退 SA 降低性能门槛。

### RQ-3：不能核销，退 AA 修正（架构问题）

A:331 已写“index 缺省 0，走立即返回”，但 A:333/334 又写“!= 才立即、== 等待”。全新启动或恢复后 version=0 时，缺省值恰好相等，因此会等满 timeout；有合法定义文字，但没有一致的决策规则。

建议 AA 定案为：**缺少 index 是首次快照请求，立即返回；显式 index=0 则按与当前版本比较处理**。保留 `Option<u64>` 区分两者，先校验所有参数，再处理缺省分支。必须覆盖空表 version=0、恢复有实例但 version=0、版本>0 三场景，另证明显式相等 index 仍挂起。该修订是端点参数补全，不改变 FR-8 的显式相等语义，无需退 SA。

## 7. TC 回填清单（TA 逐条处理）

“已定案”与“AA 待修正”明确分开；此表列出了对照中发现的期望、前置条件及证据方式差异，不能只回填 TC-16/40。

| TC / 位置 | 现状期望或条件 | 应回填为 / 依据 |
|---|---|---|
| TC-01/02 | kill/restart 无 TTL 窗口前提 | 补“恢复时持久化心跳未超本次 TTL”；与 SA 的 G2-4 修订一致，不把超时丢弃当恢复缺陷 |
| TC-03/04/07/08/13/18/34/41 的快照夹具 | 泛称合法快照，未绑定最终 DTO | 使用 A §4.2/5.1 的 format_version=1、saved_at_epoch、services→实例数组、last_heartbeat_epoch；追加 format_version 不识别的损坏分支；路径等待 G2-5 隔离定案 |
| TC-05 | 心跳 0.3s 后文件必未变、下一周期必 >T0 | 与已知 tick 相位同步，心跳与原值跨 epoch 秒边界；0.3s 窗口须避开下一 tick，否则合规周期刷盘也会被误报；另补故障恢复后的重试 |
| TC-08 | 沿用 A=now/B=now−60，TTL30 却期望两者恢复；TTL1 却期望 A=now 也丢弃 | 改固定 now、A=now−3/B=now−4，TTL30 恢复、TTL2 丢弃，并设置 flush=1；或等价确定性夹具。保留严格 age>ttl 边界用例 |
| TC-11 | 空首次启动即生成目录及快照文件；文件名待定 | open 创建目录；没有状态变化时不要求已有快照。POST 成功后才断言有效文件及更新。文件布局按 G2-5 最终定案 |
| TC-12 / TC-10 | 仅测 flush=0，禁用按默认 TTL | 增加 enabled=true 下 flush>=ttl、空 data_dir、ENABLED 非 true/false；增加 enabled=false、TTL1、默认 flush5 仍能运行的兼容用例。部分 TOML 节验证字段默认值；非法 0 的规则按 A 修订明示 |
| TC-16（RQ-1） | 5xx，500/code 仍称建议 | 固定 500 + persist_failed + 统一 JSON，内存可查且 ERROR；追加注销失败同语义。权限/非法文件注入发生在正常 open 之后 |
| TC-18 | 首次落盘后清 tmp，审查仅 temp+rename | 按 A §4.2 open 时尽力清理残留，恢复仍用旧有效文件；成功写入路径审查 temp→sync_all→rename；不把本项当作真实崩溃注入已执行 |
| TC-19 | flush=30、默认 TTL30；SIGTERM；注册后直接关停且文件含 A | 改合法配置（如 flush10<TTL60），发 SIGINT；注册后再心跳制造尚未落盘的新鲜度，关停后断言该时间戳已刷出。仅“文件含 A”在注册写穿后就成立，不能证明最终刷盘 |
| TC-21 / T §2 风险3 | ≤2s 可算 AC-8 通过 | 保留 ≤1s。环境抖动超限记录测值、复跑或判无法验证，不把超阈值样本算通过 |
| TC-22 | code 仅写省略号 | 精确 400 + invalid_parameter + code/message，分别测 0/301；语法错误另按 G2-2 统一提取器错误 |
| TC-23 | 观察 3s 挂起就等于默认 30s | 增加默认解析值=30 的单测（A §6.1），3s 观察仅作辅助。若用 Tokio 暂停时间，须明确 test-util，不能据此推进 std Instant |
| TC-26 | TTL2、sweep1、flush 默认5 | 增加 flush1<TTL2，否则 A §4.5 在启动期拒绝；批次移除多个实例整体 +1 的断言同步补入 |
| TC-29 | 审查确认 bump 仅注册/注销/剔除三事件 | 健康→不健康仍不单独 bump；新增未被 sweep 前迟到心跳复活，断言 +1、健康快照含实例、挂起唤醒；事件审查表改成四类（A §4.3） |
| TC-30 / T §1/2 | shutdown 能否注入待实现决定 | A §4.7 AppState 已暴露 shutdown Receiver，可确定补通道注入集成测试，同时保留 SIGINT e2e 的进程 exit 0 证据 |
| TC-33/34 | 主要注册竞争、所有文件心跳 epoch 严格不回退 | 增加结构变更/注销与周期刷盘争用同一锁、最终文件等于最终状态。A 的 epoch 秒换算与墙钟回拨限制下，不宜把任意 epoch 回退都判旧快照覆盖；用受控时间/已知状态序列区分真正丢写 |
| TC-35/37 | 既有测试“原样”、默认值称 Q1 建议 | 保留原 HTTP 断言，setup 用 A 的 AppState；默认值 true/./data/5 已定案，去建议措辞；不要因测试构造签名变化误判 API 兼容性 |
| TC-38（RQ-2） | 轻量自建、采集细节未定；参考级 | 按第 6 节性能专项冻结参数、真 v0.1 commit、分端点 P99；参考级不能自动视为 AC-13 满足；A 的开关持久化对照改成辅助项 |
| TC-39 | 只有含一个超时实例，却要求跳过损坏文件计数；日志 logs/registry.log | 使用有效文件恢复/超时丢弃与损坏文件跳过的两次独立启动，单文件不能同时有效且损坏；明确 skipped=0/1 字段要求并补入 A §4.4。现有 run.sh 日志是 logs/velox-registry.log，或使用新 runner 明确捕获路径 |
| TC-40（RQ-3） | 缺省 index 不断言；非数字参数期望统一 JSON 但 A 未规定提取器拒绝映射 | 按 G2-2 修订后补缺省/显式0×版本0/>0；405 方法断言保留；abc/负数/溢出等非法值映射 400 invalid_parameter，AA 明确 QueryRejection 转换 |
| T §1/§2 e2e 组织 | 扩展 velox-demo/scripts/acceptance.sh | 新脚本/测试放 velox-registry 内；已有 demo 脚本只运行不改动，mesh/config/demo 代码及脚本零修改 |
| AC-14 映射 / TC-18 附项 | tmp 模拟+审查即称无缺口，kill 时序尽力 | 为实际进程崩溃注入建立独立子用例/TC，记录命令、退出、最终文件合法性和恢复结果；若环境限制未跑，只能据实列受限与遗留项，不能将模拟冒充实跑 |

尚需补充的设计回归场景：失败 flush 无新 mutation 后仍重试；一次 sweep 多实例 +1；config 共 cwd 存储隔离；禁用持久化小 TTL；请求参数缺省/非法值；恢复的 TTL 严格边界；关停前最后一次心跳落盘。这些场景补入后同步 AC/B 矩阵，现有“无缺口”结论需重算。

## 8. 阻塞问题与可执行返工清单

### G2-1（高，AA）：脏刷失败不重试，且串行锁范围有矛盾

**证据**：A:220 `take_dirty` 是原子取走；A:266 失败仅 ERROR 并称“下个周期自然重试”；A:349/350 又写“锁外 spawn_blocking 写文件”。

**反例**：dirty=true→take_dirty 返回 true 并置 false→IO 失败→磁盘恢复但无新状态修改→下一 tick 读 false，不会重试。空表注销后的失败尤其可能持续保留旧磁盘文件。另若按“锁外”理解为读快照后释放 flush_lock，旧写覆盖新写风险重现。

**修改**：A 明确失败后原子置 dirty=true（或等效 revision/ack 方案）；成功不得清掉快照之后发生的新修改。明确 flush_lock 从读快照一直持有至阻塞 IO 完成，领域 RwLock 只包纯内存读取，不与 IO 共持。四路落盘均遵守该纪律；给 DA 可落实的步骤，补失败恢复与并发验证。

### G2-2（中，AA）：缺省 index 矛盾，参数解析错误无统一映射路径

**证据**：A:331 与 :333/:334 的 version0 反例；A §4.7 只有 u64 Query 与数值范围校验，T TC-40 却要求 abc 返回统一 JSON。现有 api.rs 使用 axum Query；本机 axum 0.8.9 `src/extract/query.rs` 的 FromRequestParts 拒绝类型是 QueryRejection，反序列化失败发生于业务范围校验之前。

**修改**：按第 6 节给缺省请求独立立即快照分支；显式相等仍长轮询。明确 watch handler 捕获 `QueryRejection` 并映射 AppError::InvalidParam（例如 `Result<Query<WatchQuery>, QueryRejection>`），只改新端点，保持既有 API 错误行为。回填 TC-22/23/40。

### G2-3（中，AA）：禁用持久化仍被跨项校验拒绝

**证据**：A:288 不区分 enabled，flush>=ttl 一律拒绝；旧 config.rs:57 接受 TTL1，R FR-5/AC-5 要求 disabled 时 v0.1 行为一致。enabled=false、TTL1、默认 flush5 被新规则拒绝。

**修改**：将跨项 `flush < ttl` 约束限定于 enabled=true；明确禁用时哪些语法/基本非法值仍拒绝，避免含糊的“全部跳过”。给 PersistenceConfig 容器明确 `#[serde(default)]` + 自定义 Default，使 `[persistence] enabled=false` 等部分节可取其余默认值；测试 env、TOML 两途径。

### G2-4（高，SA 定向修订 + AA 同步）：flush<ttl 不能保证恢复不误剔除

**证据**：R FR-2:36 的“健康实例恢复后不会被误判超时”；A:288/382 宣称硬保证。

**反例**：TTL30、flush5；上次落盘心跳 t20，t25 flush 先于该次心跳而仍落盘 t20，随后心跳 t25 入内存，t29 崩溃、t51 恢复。真实最后心跳 age26（未超 TTL），落盘 age31（超 TTL），按 FR-4 必须丢弃；所有配置校验都满足。

**修改**：**退回 SA 定向修订 FR-2，并给 AC-1/AC-2 补恢复窗口前提**：无 IO 故障/调度严重延迟时，丢最近一个周期的新鲜度；可恢复取决于“恢复时 now−已持久化 heartbeat≤TTL”，不是只取决于周期<TTL。保留 FR-4 的严格过期丢弃规则；若业务坚持更强保证，SA 必须明确允许的宕机预算/宽限语义后再设计。AA 删除硬保证措辞；“每周期至多一次写”和“每5s一次fsync”的论证限定于后台心跳刷盘，结构写穿仍每次都写。

### G2-5（高，AA；SA 修正 Q6 依据）：快照被 config 当 namespace 加载

**证据**：A:169、R:199 声称同目录文件名不同就互不冲突。config Store::open 扫描所有 *.json，`registry-state` 符合 valid_namespace；RegistrySnapshot 顶层 JSON object 也能被 `BTreeMap<String, Value>` 解析。config 将出现额外 namespace `registry-state`；该 namespace 的后续 put/delete 会覆盖同一路径，破坏 registry 快照。

**修改**：不能改 velox-config。AA 选定 registry 侧的隔离布局，建议保留 data_dir 默认 ./data，实际快照存其专属子目录（如 ./data/registry/registry-state.json），使 config 非递归扫描不读取；写清实际目录创建、tmp、恢复路径并同步 TC 夹具/README。若改默认 data_dir，则须 SA 同步 FR-5/AC-5 的默认值，不能 AA 静默变更。SA 定向纠正 Q6“共目录无冲突”依据。补同 cwd、两个进程各自读写后的相互隔离验证；不以修改 config 扫描器解决。

### G2-6（中，AA/TA）：AC-13 基线被替换且执行规范未闭合

**证据**：R AC-13 明确 v0.1；A:431 只比开/关持久化，A:37 标“可选”；T RQ-2/TC-38 缺测量参数。

**修改**：按第 6 节裁决，将验收期性能专项写回 A、TC-38；指定真 v0.1 commit、同条件测量与报告格式；不要求 QA 此时跑尚未实现的功能，不降低冻结阈值。

### G2-7（中，TA，AA 配合）：回填所有不一致 TC 与测试组织

**证据**：第 7 节逐项表，尤其 TC-08/19/26 无法获得预期结果，TC-21 越权放宽 AC-8，TC-29 漏复活，TC-40 定案未闭合；T:13 计划改 demo 脚本越界。

**修改**：按表完整回填 T 并更新 RQ 状态/矩阵，新增脚本限定 registry 内。A 补测试模块可访问路径（当前是 bin crate、模块在 main 中私有；可选 registry 内 lib.rs 或明确的测试模块入口）、实际时钟策略与日志 skipped 字段；保留真正 e2e 信号/崩溃证据要求。此项由 TA 回填，不让 DA 修改冻结 AC 来迎合错误用例。

## 9. 非阻塞修正与验证记录

- A §4.2 称“落盘字节稳定”，但 PersistedInstance.metadata 仍为 HashMap，key 遍历顺序不稳定。可改成有序 DTO 或删除字节稳定承诺；本需求只要求内容可恢复，不要求 byte-for-byte 输出稳定。
- `sync_all(temp)+rename` 满足本期进程崩溃原子替换方向，但不能承诺所有掉电情况下目录项已持久化；A:170 应收窄“覆盖掉电”的表述，或明确是否同步父目录。不将掉电强保证扩成本期额外 AC。
- A §4.3 恢复会过滤过期实例，因此“文件本已匹配内存”不总成立；如不在启动时重写，应描述为接受的延迟清理，不能声称文件内容完全相同。
- A §4.8 “四把锁互不嵌套”不精确：flush_lock 内确实会短暂取领域读锁。正确理由是没有相反的“领域锁→flush_lock”等待边，无环；不应为了消除文字上的嵌套而移出快照读取。

取证动作：读取四个输入全文；读取 registry 五个源文件、config store/config、workspace 与 registry Cargo.toml、scripts/test.sh/common.sh/run.sh；核对 git branch/HEAD/status 与源文件历史差异；只读核查本机 Tokio feature 与 axum Query 拒绝路径；用不落盘的 Python 语句复核 index0 分支、dirty 失败状态、namespace 字符规则和 TTL 数值反例。没有生成或运行新代码测试，未声称本 feature 已实现或任何 AC 已实跑通过。

返工后关口复审依据：AA 修订 G2-1/2/3/5/6，SA 定向修订 G2-4 与 Q6 错误依据，TA 完成 G2-7 全部回填；再核对三者一致性后才能进入实施。

PIPELINE registry-persistence-watch GATE2 REWORK G2-1补失败刷盘重试并统一锁范围；G2-2明确缺省index分支及参数错误映射；G2-3限定禁用配置校验；G2-4退回SA定向修订FR-2及AC-1/2恢复前提；G2-5隔离config存储扫描并纠正Q6依据；G2-6保留v0.1性能基线；G2-7按第7节逐TC回填并收回越界脚本计划

---

# 第 2 轮复审（2026-11-26，被审版本 SHA-256 f4f33557…）

> 工序：F-GATE2 第 2 轮复审（AA 返工稿复核，非终验）。
> 被审对象：`02-architecture.md` 第 2 轮返工定稿，586 行；本机重算 SHA-256 `f4f3355770f2cd1e…d99c4`，与任务给定前缀一致。
> 取证基准：`develop @ 28c9114`。`git diff 28c9114 -- velox-registry velox-config velox-mesh velox-demo` 为空（工作区改动仅限本目录文档），第 1 轮全部代码证据继续适用；对 G2-5 依据涉及的 `velox-config/src/store.rs` 做了只读复核。
> 输入哈希核验：A 文首声称的输入哈希经重算属实——R=`7285db59…996`（与 G1 定向 PASS 记录一致）；T（`cd145d59…`）与 G1 报告（`0b470ead…`）均为现行工作区版本。上方第 1 轮原文（含其末行 REWORK 判定）未改动。
> 引用约定沿用第 1 轮：R/G1/A/G2/T；本节「§x」未加标注者均指新 A 章节。本轮只读评审：未运行构建、测试或服务，未声称任何 AC 已实跑通过。

## 1. 结论

**通过，GATE2 PASS（设计关口层面）。**

第 1 轮 G2-1…8 逐条闭合，§9 四项非阻塞纠正全部落实；每条结论均以新稿章节号证据复核，不采信「已修复」空声明。三方一致性成立：A 新增 §10 对照表与 T 的「待 AA 对齐」十项标注逐项吻合；G2-4 的 SA 定向修订已由 G1 定向 PASS 闭合并在 A 中同步。冻结阈值（AC-8 ≤1s、AC-13 真 v0.1 比值 ≤2、FR-4 严格 `age > ttl`、FR-5/AC-5 默认 `./data`）未被破坏；范围红线（仅 velox-registry 及文档）未被越过。

本 PASS 不代表实现完成：AC-7/8/13/14 的执行证据仍按第 1 轮裁定与 T 档位政策在实施/验收期交付（TC-16/21/38/18b 实跑、RQ-2 证据待交付），「设计正确」不等于「测试通过」。

## 2. G2-1…8 逐条核验

| 返工项 | 判定 | 章节级证据与复核说明 |
|---|---|---|
| **G2-1** 失败刷盘重试 + 锁范围统一 | **闭合** | ①失败置回：§3.2 不变量 5「任何路径 flush 失败 → `mark_dirty()`，持续重试直至成功或进程退出（关停除外）」；§4.3 `mark_dirty`「失败置回（幂等）、成功路径不得调用」；§4.4 步骤 5（sweep 失败置回+ERROR）/步骤 6（flusher Err→置回，下周期重试、不退避）/步骤 9（关停尽力而为，进程退出无重试目标）；§4.7 写穿失败 `mark_dirty`+500；§5.1 时机表四行与之一致；§7 B12b 记录被驳回备选。第 1 轮反例（take 后 IO 失败、无新 mutation 则永久滞留旧文件）由此闭合。②成功不清并发脏状态：§3.2 不变量 5「成功 flush 不清除任何 dirty」+「先 `take_dirty`、再锁内取快照」纪律；§4.3 `take_dirty` 注释给出顺序颠倒的具体丢失场景；§4.8 AtomicBool 行「成功路径不清除、多余一次 flush 无害」；§6.1/6.2 补 TC-05b/05c/17（含全空表）用例。③锁范围：§4.2 flush 契约明写覆盖范围=「读快照 + 完整阻塞 IO」，「持锁 await spawn_blocking → IO 完成后才释放」；§3.2 不变量 2、§4.8 锁表四路（写穿/sweep/周期/关停）共用同一纪律；旧稿「锁外 spawn_blocking 写文件」措辞经 grep 已不存在（§4.8/§9 QA-4 仅余否定语境引用）。 |
| **G2-2** 缺省 index 分支 + 参数错误映射 | **闭合** | ①分支分离：§4.7 步骤 4 以 `Option<u64>` 区分——缺省（None）=首次快照请求立即返回，且显式枚举空表 v0/恢复有实例 v0/version>0 三场景；显式 `Some(0)` 参与比较（v0 按相等挂起、v>0 立即）；显式相等仍挂起（反例保留）。与第 1 轮 §6 RQ-3 建议定案逐字一致，T 的 RQ-3 矛盾由此消解。②先校验后分支：§4.7 步骤 1 解析失败最先映射（handler 签名 `Result<Query<WatchQuery>, QueryRejection>`），步骤 2 范围校验明示「先完整校验所有参数，再进入 index 分支——缺省 index 的立即分支同样必须先通过校验（TC-22）」。③仅新端点：§4.6 映射表 `InvalidParam` 两个来源（范围校验 + `QueryRejection`），「仅 watch 新端点捕获，既有六端点错误行为不变」；§7 B14 记录旧方案被驳回理由。测试：§6.1 api.rs（解析失败映射单测、缺省精确 ==30）、§6.2 watch.rs（三版本场景 × 缺省/显式 0、显式相等挂起、abc/负数/溢出/空值 → 400）。 |
| **G2-3** 禁用配置校验限定 | **闭合** | §4.5：容器级 `#[serde(default)]` + 手写 `Default`（部分节缺省字段取 true/./data/5，TOML 与 env 两途径）；解析期拒绝（TOML 类型错、env 非法值）**无论 enabled**；validate 规则 1（flush=0）、规则 2（空 data_dir）**无论 enabled**；规则 3 跨项 `flush >= ttl` **仅 enabled=true**；规则 4 明示 `enabled=false + TTL1 + 默认 flush5` 合法可运行，且「除规则 1/2 外禁用态无其他拒绝项」——无含糊的「全部跳过」。测试对应：§6.1 config.rs 逐条（含 enabled=false 的 flush=0 拒绝与跨项豁免）、§6.2 persistence.rs 补 enabled=false 兼容分支。第 1 轮反例（禁用被跨项校验拒绝）闭合。 |
| **G2-4** SA 定向修订的 AA 同步 | **闭合** | SA 侧已由 G1 定向 PASS 闭合（FR-2 恢复窗口、AC-1/2 前提、Q2/Q6；本机重算 R 哈希与 A 文首一致，修订内容核对无误）。AA 同步证据：§4.5 规则 3 改为「缩小窗口、不构成硬保证，可恢复以恢复时 `now − 已持久化 heartbeat ≤ 本次 TTL` 为准」；§5.1 heartbeat 行按 FR-2 前提表述（无 IO 故障/调度严重延迟时丢一个周期，故障时不承诺硬上界）；§9 Q2/QA-5、§8 R6 同改；§1 AC-1/2 行与 §6.2 persistence.rs 显式构造恢复窗口前提；§4.3 `restored` 注释/§4.4 步骤 3 补 dropped_expired 延迟清理；「每周期至多一次写」限定后台心跳刷盘、结构写穿逐次（§4.2 要点 2、§9 QA-5）。grep 全文：「硬保证/必不/必存活」仅出现于否定语境，无正面残留；FR-4 严格 `age > ttl` 保留（§4.3/§5.2，TC-08c 边界）。 |
| **G2-5** config 扫描隔离 + Q6 纠正 | **闭合** | §4.2 定案 `{data_dir}/registry/registry-state.json`、tmp 同子目录；`data_dir` 默认仍 `./data`（FR-5/AC-5 不变）；config 代码零修改、不以改扫描器解决。依据经本轮只读复核 `velox-config/src/store.rs` 属实：`Store::open` 非递归 `read_dir` 顶层 `.json`、`valid_namespace` 接受字母数字/`-`/`_`（`registry-state` 合法）、内容按 `NamespaceData`（BTreeMap）解析、put/delete 经 flush 写同路径。配套：§2.2 边界警示、§4.4 步骤 3 递归建目录+open 清 tmp、§5.1/§5.3 隔离说明、§8 R5、§9 Q6（旧「共目录无冲突」依据作废；B-10 双 registry 约束保留且明示不得再用于证明共顶层目录安全）、§7 B13、§6.4 TC-37b（含 config 侧创建顶层 `registry-state.json` 的反向隔离验证）。第 1 轮反例（快照被装载为 namespace 后被覆盖）闭合。 |
| **G2-6** 真 v0.1 性能基线 | **闭合** | §1 AC-13 行与 §6.4 专项：真 v0.1 基线 commit **`28c9114`** 隔离检出/独立 target（该提交 registry 仍为 v0.1 内存实现，与第 1 轮取证一致，本轮 git 核验代码零漂移）；同机/同 release profile/同负载计划；冻结参数（预热 ≥30s、采样 ≥60s、每端点 ≥1000 有效样本、nearest-rank P99、错误另计不得丢失败样本改善 P99）；分端点比值**每项 ≤2 才满足**；`enabled=false` 仅辅助、不得替代基线；验收期专项、非常规 CI 门槛但不可省略，无测量/样本不足标「无法验证」。阈值未降低，与 T TC-38 协议一致。 |
| **G2-7**（AA 配套部分；TA 回填属 T 本身） | **闭合** | §4.1 新建 `lib.rs` 测试入口（对齐 T §2.1 等待项：bin crate 私有模块不可 import）；§4.4 步骤 3 启动日志补 `skipped`（单文件布局 0/1，TC-39）；§6 开头时钟策略（固定 `now_epoch` 注入 + 屏障相位 + 真实短等待，不假定 `test-util`，对齐 F-TIME）；§6.1/6.2 对应测试落位。T 侧：新增脚本均限 registry 内（`tests/registry-e2e.sh`/`registry-perf.sh`），demo 脚本只运行零修改，越界计划已撤回；A 与 T 无冲突。 |

## 3. §9 四项非阻塞纠正与 §2 措辞纠正

| 第 1 轮非阻塞项 | 判定 | 证据 |
|---|---|---|
| ① 字节稳定承诺 | 落实 | §4.2 `PersistedInstance.metadata` 注释「遍历顺序不保证字节稳定，只承诺内容可恢复」；快照服务名 BTreeMap 排序、实例列表有序仅利于 diff；未改用有序 DTO 亦符合第 1 轮给出的两个可选方向 |
| ② 掉电表述 | 落实 | §4.2 要点 2：`sync_all` 保证范围收窄为「temp 内容在 rename 前落盘」，未 fsync 父目录、目录项掉电持久化不作承诺；未扩为额外 AC |
| ③「文件本已匹配内存」 | 落实 | §4.3 `restored`/§4.4 步骤 3/§5.2：dropped_expired>0 → 置 dirty 延迟清理，周期 flusher 一个周期内改写文件，启动不做额外同步写；启动即崩溃场景幂等无害已说明；该说法已删除 |
| ④「四把锁互不嵌套」 | 落实 | §4.8 死锁评估改为「存在 flush_lock→领域读锁短暂嵌套，但无领域锁→flush_lock 反向等待边，无环」，且不为消除文字嵌套而移出锁内快照读取 |
| §2 备选论证措辞 | 落实 | §2.3：「零新增依赖」为 NFR-6「预期」之下的选择性结论，确需新依赖须重新评审而非绝对禁止；§7 B4 同步；fsync/写频率按路径区分（§4.2 要点 2） |

## 4. 与 03-test-cases.md（TA v1）对齐

A §10 对照表所列十项「待 AA 对齐」逐项核对，全部落实且与 T 原文一致：

| TA 等待项（T 位置） | 对齐证据（A） |
|---|---|
| F-PATH 布局（T §2.2） | §4.2 定案与 T 目标布局逐字一致（`<data_dir>/registry/registry-state.json`、tmp 同子目录、默认根不变） |
| TC-10 跨项校验仅启用（T:79） | §4.5 规则 3/4 |
| TC-12 禁用仍拒基本非法值（T:81） | §4.5 解析期拒绝 + 规则 1/2，并明示禁用态无其他拒绝项 |
| TC-37b 布局最终确认（T:106） | §4.2/§5.1/§5.3；「专属子目录避开非递归扫描」判据成立 |
| TC-39 skipped 字段（T:108） | §4.4 步骤 3（0/1） |
| §2.1 lib 导出测试入口（T:14） | §4.1 `lib.rs` 定案 |
| TC-40/RQ-3 缺省 index 定案（T:109/§6） | §4.7 步骤 1/2/4 = T 引述的 G2 目标（三场景、先校验、QueryRejection→400 仅新端点） |
| TC-34 四路同锁 + 成功不清并发 dirty（T:103） | §4.8 + §3.2 不变量 2/5 |
| TC-23 默认值精确 =30 单测（T:92） | §6.1 api.rs「缺省精确 == 30」 |
| TC-05c 成功 flush 不清新 dirty（T:74） | §4.3 take/mark 纪律 + §3.2 不变量 5 |

**阈值与范围未被破坏**：AC-8 严格 <1s（§6.2「超限样本记录实测/复跑或判无法验证，不以 2s 通过」；§4.7 通道即时 notify）；AC-13 真 v0.1 比值每项 ≤2（§6.4）；FR-4 严格 `age > ttl`；FR-5/AC-5 默认 `./data`——A:586 自查与本文复核一致。范围红线：仅改 `velox-registry/` 及文档；grep 复核无修改 mesh/config/demo 的设计（demo 只读回归零脚本修改、TC-37b config 零修改、新增脚本限 registry 内）。

## 5. 自洽性

新稿内部无前后矛盾：§3.1 数据流图、§4.8 锁表、§5.1 时机表对 flush 纪律（锁覆盖范围、失败置回、成功不清除、四路共用）三处表述一致；§4.3 版本规则表与 FR-9/QA-2/QA-3 定案一致；§4.5 校验规则与 §6.1 测试清单逐条对应；§10 引用的全部章节号（§3.2/§4.2/§4.3/§4.4 步骤 3/5/6/9/§4.5/§4.6/§4.7/§4.8/§5.1/§5.2/§5.3/§6.1/§6.2/§6.4/§7 B12b/B13/B14/§8 R5/R6/R8/R9/§9 Q2/Q6/QA-4/QA-5）均存在且内容相符；引用的 TC 编号（TC-05b/05c/08c/11/15/16/17/18a/18b/22/23/37b/39/40 等）在 T 中均存在且语义相符。

## 6. 非阻塞观察（不阻塞本 PASS，不要求返工）

- §4.2 `LoadOutcome::Missing` 注释「首次启动**或 enabled=false**（INFO 即可）」：按 §4.4 步骤 3，enabled=false 时 `store=None`、`load()` 根本不会被调用，注释列举 enabled=false 属冗余表述。§4.4 本身无歧义，DA 按 §4.4 实施即可；仅提示 AA/DA 实施时勿据该注释在禁用路径误调 load。

## 7. 验证记录

重算被审对象 SHA-256（`f4f3355770f2cd1e…`，586 行相符）与 R/T/G1 工作区哈希（R 与 A 文首及 G1 记录一致）；全文读取 A/R/G1/G2/T 五份文档；grep 复核残留措辞（锁外/硬保证/必不/亚秒/字节稳定/顶层/共目录）均仅出现于否定、收窄或作废语境；只读抽查 `velox-config/src/store.rs`（namespace 字符规则、非递归顶层扫描、`NamespaceData` 解析、flush 锁覆盖 IO、`std::fs::write` 直写无原子替换）证实 G2-5 依据与 §2.2 出处修正属实；git 只读核验 HEAD=28c9114、代码目录零漂移、本目录四份文档为同波 SA/TA/AA 工作区修订。未运行构建、测试或服务，未新建/修改除本报告外的任何文件；本 PASS 为设计关口结论，不声称本 feature 已实现或任何 AC 已实跑通过，执行期证据按 T 档位与第 1 轮裁定继续跟踪。

PIPELINE registry-persistence-watch GATE2 PASS
