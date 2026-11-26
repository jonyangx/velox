# 测试用例（v1 定稿）：velox-registry v0.2 状态持久化 + watch 推送（registry 侧）

> 状态：v1 定稿——已对齐 01-requirements.md 冻结 AC；GATE2 第1轮返工同步稿，设计覆盖不代表测试通过。
> slug：`registry-persistence-watch`；基准分支 `develop`，HEAD `28c91140dd14a29d25dcbc270b2bb23aeb32b2f0`。本轮仅修订本文，不切分支、不提交、不运行功能测试。
> 需求输入：SA 定向修订 FR-2/AC-1/AC-2/Q2/Q6，最新 `01-requirements-review.md` 为定向 GATE1 PASS；现行 `02-architecture-review.md` 仍为 GATE2 REWORK，全文 §7/8 为返工依据。
> 架构读取版本：`02-architecture.md` SHA-256 `e51c5bb1effad7af7f90a12b91ebc4eb909c5097d7042ef7af314eecb158b5cd`（旧稿，AA 同波返工尚未落入该版本）。本文引用已定案部分；对 G2-1/2/3/5 的修订目标明确标出待 AA 对齐，不能据此宣布架构返工完成。
> 仅 registry 及文档；mesh/config/demo 代码和脚本零修改；新增测试/脚本仅在 registry 内。保留原 TC-01…41 编号，细分子场景，不撤销任何 AC。

## 1. 测试范围与分层策略（单测/集成/e2e 边界）

| 层 | 执行位置与入口 | 覆盖与边界 |
|---|---|---|
| 单测 | registry 内嵌测试，`cargo test -p velox-registry` | 配置默认/校验、固定 epoch 恢复过滤、DTO、参数默认值、版本/dirty 不变量 |
| 集成 | 拟新增 `velox-registry/tests/{persistence,watch,compat}.rs`，同一 cargo 入口 | Router+oneshot、真实临时存储、可注入 shutdown、失败/并发；当前 bin crate 私有模块不能直接从 tests/ import，AA 须明确 lib 导出或等效测试入口，DA 随实现交付；不能声称现已可编译 |
| e2e | 拟新增 `velox-registry/tests/registry-e2e.sh`；入口 `bash velox-registry/tests/registry-e2e.sh`（待实现） | registry 真进程 SIGKILL/restart、SIGINT/exit、写入中途崩溃、同 cwd 与原 config 进程隔离；脚本、夹具均限 registry 内 |
| 既有跨服务回归 | `bash velox-demo/scripts/acceptance.sh`（只运行，完全不改） | 原 mesh 拉取、consumer/provider 转发、LB、TTL 剔除等已有断言。该脚本不证明新增 registry 崩溃恢复/原子写 |
| 性能专项 | 拟新增 `velox-registry/tests/registry-perf.sh`（待实现） | 真 v0.1 对比 v0.2，TC-38 规定负载与判据；不要求每次 CI 压测，但验收不可省略 |
| 审查 | registry 代码及 README/上游文档 | 领域锁内无 IO/await、flush 锁覆盖、原子替换、边界声明；审查不能代替进程证据 |

G2 逐项落实：G2-1→TC-05b/16/17/33/34（失败后无新 mutation 重试、四路同锁、成功不清掉并发脏状态）；G2-2→TC-22/23/40（缺省与显式0分离、先校验参数、统一拒绝）；G2-3→TC-10/12/37；G2-4→TC-01/02/03/07/08/09（落盘心跳窗口、严格边界及反例）；G2-5→TC-11/14/18/37b（默认根不变、子目录与 config 隔离）；G2-6→TC-38/RQ-2（真基线、原阈值）；G2-7→本稿全部41TC、矩阵、时间/入口/档位及越界计划撤回。AA 尚未完成项由 GATE2 复审裁决，不由 TA 代改架构。

## 2. 测试环境与基建（框架、命令、夹具、e2e 手段）

### 2.1 只读侦察及执行命令

已核对根 README、`scripts/README.md`、`scripts/test.sh`、registry README/Cargo.toml、已有 demo 验收脚本及 config Store 扫描代码。现状 registry 为 bin crate，无 lib target，tower 为 workspace dev-dependency，`velox-registry/tests/` 尚无现成测试入口。临时目录可沿用 config 测试的 UUID 隔离模式，但 config 的原子写不能复用为既有事实（现状为直接 write）。

后续执行命令（本轮均未运行）：

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo build --workspace
cargo test -p velox-registry
cargo test --workspace
bash velox-demo/scripts/acceptance.sh
```

`scripts/test.sh` 完整门槛为 fmt/clippy/build/workspace test，脚本 fmt 写作等价的 `cargo fmt --all -- --check`；`--unit` 实际执行 `cargo test --workspace`，不是仅 registry。registry 运行入口为绝对构建产物或 `cargo run -p velox-registry -- <config>`；指定 cwd/配置以免默认状态互相污染。`scripts/run.sh` 日志实际为 `logs/velox-registry.log`，PID 在 `.run/`；新 runner 将 stdout/stderr 捕获为外部临时目录 `<run-dir>/registry.log`，执行报告记录绝对位置、命令和退出码，不提交运行数据、日志、状态。

已有 demo 脚本依赖 cargo/curl/jq，占用 8100/8101/8200/8300/9000/9001/9100，使用 TTL6/sweep2，默认 flush5 满足启用时跨项约束。运行时从外部设置独立 `VELOX_REGISTRY__PERSISTENCE__DATA_DIR`；原脚本会清理 `data/demo.json`，只在隔离运行环境且确认不覆盖既有数据时执行，不能新增修改 demo 脚本解决隔离问题。host 模式即可，无需 Docker。注意脚本的 provider SIGTERM 步骤容许 TTL 兜底，不把它当 registry SIGINT 优雅退出的证据。

### 2.2 统一夹具与可判定时间策略

- **F-SNAPSHOT**：按旧 A §4.2/5.1 已定 DTO，`format_version:1`、`saved_at_epoch:u64`、`services:{svc:[{id:合法UUID,address,port:u16,metadata:字符串map,last_heartbeat_epoch:u64}]}`；合法文件不省必填字段。未知版本99、缺字段、类型错、截断、空文件分别作为坏夹具。比较内容语义，不要求 HashMap JSON 字节顺序稳定。
- **F-PATH（AA 待定）**：需求默认根仍为 `./data`，本稿目标布局为 `<data_dir>/registry/registry-state.json`，tmp 同一隔离子目录 `registry-state.json.tmp`。旧 A 的顶层布局已被 G2-5 驳回；AA 最终文件布局落盘后统一替换夹具常量并复核本文，不能悄用顶层文件或改默认根。TC-37b以“专属子目录避开非递归扫描”为不变判据。
- **F-RESTORE**：固定 `now_epoch=N` 注入恢复纯逻辑；age=TTL 应保留，age=TTL+1 丢弃。相等边界在 HTTP 往返前即可再次过期，故边界不靠真实 sleep/HTTP 证明。真实恢复使用宽裕窗口（如 TTL60/flush5），杀进程前读取有效快照记录 heartbeat，重启记录时间，核验恢复时落盘 age≤TTL，再在再次过期前查询/续约；若窗口已超则本次正例不适用，不误报恢复缺陷。
- **F-TIME**：当前 tokio `full` 不包含 `test-util`，且暂停 Tokio 不推进领域 `std::time::Instant`。默认采用固定 epoch 的纯函数测试+真实短时间等待；若 DA 明确启用已有 tokio 的 dev `test-util`，只用于异步 timer，不代替领域 TTL。tick 测试需屏障同步相位，绕过 interval 首次立即 tick；epoch 秒精度要求心跳与旧值跨秒。观察窗必须完全处在下个 tick 前，不能硬写“任意0.3s内文件不变”。
- **F-FAIL**：先 open/注册成功，再让快照目标或 tmp 变为目录等确定性写失败；先断言注入确实失败，再触发操作。chmod555仅非特权环境辅助，不将目录初始化失败当运行期故障。撤销注入不产生 registry mutation，验证后续自动重试。故障事件与 ERROR 路径/原因一并捕获。
- **F-WATCH**：显式 index=n 确认挂起后再触发单次变更，禁止并行额外 mutation 干扰 n+1。以单调时钟从变更请求发起前至收到 watch 完整响应计时，保守判定 ≤1s；超限样本不能算通过，可记录环境证据后复跑或无法验证。立即分支以1s请求截止且不等待 timeout 判定；显式相等的反例同时验证挂起。
- 所有 env 修改需串行/隔离子进程并恢复环境。临时目录在系统临时根按 UUID 分配，只清理本测试拥有路径/进程；只读保留现有工作区。

### 2.3 执行档位及终验政策（本轮仅设计）

| 档位 | EXECUTE 判定与预计成本 | FINAL 要求 |
|---|---|---|
| T0 | 必跑 `cargo test -p velox-registry`（预计2–5分钟，编译另计），覆盖单测/集成；新入口未交付则记录阻塞 | 受影响套件快速回归必跑 |
| T1 | 本 feature 改启动装配/配置/刷盘及 shutdown，触发构建+起进程+P0恢复/watch冒烟，预计3–8分钟 | 按最终装配风险保留证据 |
| T2 | 配置、共享状态、API传导及兼容风险成立，应跑 `cargo test --workspace`，预计3–10分钟，并保留 fmt/clippy/build门槛证据 | 无条件全量回归，不逐包重复替代 workspace |
| T3 | 真信号/重启/写入崩溃及原mesh链路触发，registry runner+既有demo验收，预计5–10分钟；性能专项另预估5–10分钟 | 无条件执行 e2e；环境搭建超过约10分钟可降为集成等价，说明无法覆盖的真实信号/退出/跨进程证据，不能免除或称全AC已通过 |

后续逐TC记录通过/失败/阻塞/无法验证/降级，记录命令、范围、耗时、退出、原始证据路径。任何跳过/降级均写触发判定与理由；单档超过预计2倍仍无结论停止并如实记录。TAF先核常规报告与 git log：仅当实跑T2+T3已覆盖最终feature代码提交、此后无新代码才可复用；否则无条件T0/T2/T3。报告必须列feature最终代码commit与当时HEAD、脏树差异，不能拿本次设计HEAD作最终实现证据。

## 3. 用例表（TC-x：功能点/对应 AC/类型/前置/步骤/期望/优先级/可自动化）

> “单/集/e2e”即计划自动化层，审查为手工；全部为待实现/待执行案例。功能点FC-1恢复，FC-2刷盘，FC-3配置隔离，FC-4故障原子写，FC-5 watch，FC-6兼容性能观测。已知AA待改处采用G2修订目标，不冒充旧A已有一致决定。

| TC | 功能点 / 对应AC或FR | 类型 / 优先级 / 可自动化 | 前置条件 | 操作步骤 | 可观察期望 |
|---|---|---|---|---|---|
| TC-01 | FC-1 / AC-1 | e2e / P0 / e2e | F-RESTORE；enabled=true，TTL60/flush5，独立目录；有效快照已含A，恢复时落盘age≤本次TTL | POST A含metadata；读文件确认落盘字段；记录id及heartbeat；kill -9并wait；同配置重启，及时GET | 记录SIGKILL退出及重启命令；启动日志恢复svc/A；再次过期前id/address/port/metadata完全一致。成功前提不能用“实际心跳未超TTL”替代 |
| TC-02 | FC-1 / AC-2 | e2e / P0 / e2e | TC-01成功恢复且尚未再次剔除；核验同一落盘窗口 | 原id PUT heartbeat，再查healthy列表 | 200 `{"ok":true}`、无404且含A；落盘已超TTL的样本归TC-09，不适用成功断言 |
| TC-03 | FC-1 / AC-1/2 | 集 / P1 / 集成 | F-SNAPSHOT与固定N，A heartbeat=N−3，TTL30/flush5 | load/restored后查询并原id心跳；另做真实drop/open，记录前后epoch及耗时 | 字段保留，恢复年龄来自落盘epoch而非归零（允许秒精度）；原id200。drop/open仅集成恢复证据，不代替TC-01信号 |
| TC-04 | FC-1 / AC-1、FR-1 | 集 / P0 / 集成 | 有效Store/Router、F-PATH | POST A成功后立即读文件；DELETE A成功后立即再读 | 响应完成即文件含A/不含A，epoch为整数，DTO完整；均不等待下个周期 |
| TC-05 | FC-2 / AC-3、FR-7 | 单+集 / P1 / 单测+集成 | 5a TTL30/flush2；相位屏障、跨epoch秒；5b F-FAIL | a 注册并记录T0，在已知tick之后、下次前心跳，读取避开tick窗口，再等下次tick；b dirty置位后注入周期写失败，恢复可写且不发新mutation，等下个周期；c快照读取后再心跳 | a tick前文件heartbeat=T0，下一脏tick后>T0且心跳响应不await IO；b失败ERROR，dirty保留/重置，下一周期文件追上内存；c成功flush不得清掉快照之后的新dirty，再一周期持久化最新值。后台每周期合并一次仅限心跳，不限制结构写穿 |
| TC-06 | FC-2 / AC-3 | e2e / P1 / e2e | TTL30/flush5，持续每5s心跳，相位与epoch记录 | 覆盖至少3个周期，采mtime/文件heartbeat及心跳耗时；保持窗口内无结构变更 | 脏周期文件推进，心跳全部成功且不等本轮IO；mtime辅助，结合文件内容/路径调用审查证明合并，不以文件系统mtime粒度独立判失败 |
| TC-07 | FC-1 / AC-4 | 单+集 / P0 / 单测+集成 | F-SNAPSHOT，固定N，TTL30，A=N、B=N−60 | 恢复、查服务/全部/健康列表及统计日志 | 仅A恢复，B在任意列表不存在，dropped_expired=1、WARN含丢弃信息；无TTL宽限 |
| TC-08 | FC-1 / AC-4 | 单+集 / P1 / 单测+集成 | 同一不可变夹具副本，固定N，A=N−3/B=N−4；flush1 | a TTL30恢复；b新副本TTL2恢复；c固定N/TTL30置C=N−30,D=N−31 | a恢复2；b丢弃2；c恢复C丢弃D，严格age>TTL；以本次TTL为准。禁止沿用旧now−60却声称TTL30恢复的夹具 |
| TC-09 | FC-1 / AC-4、B-4/5、FR-2 | 单+e2e / P1 / 单测+e2e | a TTL6/flush1，provider心跳2s、有效文件；b固定epoch反例 | a kill registry并等待>TTL后重启；b落盘t20、实际心跳t25、t29崩溃/t51恢复、TTL30 | a恢复为空，计数匹配；未改provider持续404需外部重注册是已知限制；b落盘age31被丢弃，即使真实age26；不把该情形报为恢复缺陷 |
| TC-10 | FC-3 / AC-5 | 单+集 / P0 / 单测+集成 | enabled=false、TTL1、默认flush5；TOML及env分别；已存在快照与不存在目录分别 | load；注册/心跳/查询/注销，重启；观察目录及任务 | 小TTL配置正常；不创建/不写/不恢复，不起flusher，已有文件内容不变，重启空表；跨项flush<TTL校验仅启用时适用（待AA同步） |
| TC-11 | FC-3 / AC-5 | e2e / P0 / e2e | 无配置文件/无相关env，干净临时cwd | Config::load(None)启动；查目录；POST A后读文件 | 默认true/./data/5，先创建隔离目录且healthz200；首次空启动不要求快照已存在，POST成功后F-PATH有效文件含A，根默认值不变 |
| TC-12 | FC-3 / AC-5 | 单+进程 / P0 / 单测+e2e | 独立配置/env进程 | TOML/env各测flush=0/负数/非数字、空data_dir、非法ENABLED；enabled=true测flush=TTL、>TTL；部分节`enabled=false`及只给data_dir | 语法/类型、flush0、空目录等基本非法值禁用时仍拒绝（待AA明确），错误含配置项和实际值；启用才拒绝跨项；部分节其余字段取true/./data/5默认，TOML及env覆盖优先级可判定，不用serde bool的false误代true |
| TC-13 | FC-4 / AC-6 | 集 / P0 / 集成 | F-SNAPSHOT分别改为截断、缺字段/类型错、空文件、format_version99，每次独立目录 | 各启动恢复并GET healthz/services | 每种整文件跳过、空表、healthz200；WARN含实际文件路径及原因，skipped=1；不部分恢复 |
| TC-14 | FC-3 / FR-6、AC-5 | 集 / P1 / 集成 | F-PATH根及子目录均不存在 | open、启动、注册 | 递归创建所需隔离目录，启动成功，POST后有效快照仅在专属子目录；不修改config |
| TC-15 | FC-4 / FR-6、B-3 | 集+进程 / P0 / 集成+e2e | data_dir或必需子目录位置为普通文件；非特权权限分支辅助 | 启动 | 非零退出、未接客，错误含路径与原因；此为初始化失败，独立于TC-16运行期写失败 |
| TC-16 | FC-4 / AC-7 | 集 / P0 / 集成 | 正常open后F-FAIL注入；注册及注销分别新夹具 | a POST A后查内存；b预注册B，注入失败后DELETE并查询；读取响应及日志 | a/b均500 `{"code":"persist_failed","message":"registry state persist failed"}`；A内存可查、B内存已移除，服务继续，ERROR含内部路径/原因，HTTP不泄露路径。RQ-1已核销；结构失败仍置dirty供重试 |
| TC-17 | FC-4 / AC-7、FR-7 | 集 / P1 / 集成 | 正常注册后注入故障，周期/剔除各触发 | 心跳及查询；制造sweep写失败；撤销故障，不再mutation等下周期 | 心跳200、查询正常、无panic；后台失败ERROR；无新mutation仍自动重试，最终文件反映移除后的表（含全空场景），不留旧实例 |
| TC-18 | FC-4 / AC-14 | 集+审查+e2e / P1 / 集成+e2e，审查手工 | a有效旧F-SNAPSHOT+半写tmp；b正常实例进程，受控文件写观察 | a open/load；审查temp→sync_all→rename；b独立子例TC-18b由registry runner触发写入并在tmp写入阶段kill -9、wait，解析正式文件并重启 | a open时尽力清理tmp，恢复旧正式文件；不能说首次写才清理。b记录命令/时间/信号退出/tmp阶段命中证据/正式JSON及恢复结果，正式文件只能旧完整或新完整，不把半写当有效。未命中写窗口应重试有限次数并记无法验证，tmp模拟或普通kill不可替代此项；掉电强持久性不在承诺内 |
| TC-19 | FC-2 / FR-2、B-8 | e2e / P1 / e2e | TTL60/flush10合法配置，tick同步；注册已写穿 | 跨epoch秒再心跳制造未落盘值，确认文件仍旧；下次tick前SIGINT，wait后读文件；故障分支另跑 | 成功路径exit0、落盘heartbeat等于最后心跳；仅“含A”不算最终刷盘证据。故障路径最终刷盘ERROR且尽力退出；不以SIGTERM替代当前ctrl_c |
| TC-20 | FC-5 / AC-8 | 集 / P0 / 集成 | 单次注册后n>0，无其他mutation | GET watch?index=n−1&timeout_secs=5 | 1s截止前立即200，index=n、services含既有健康实例；不挂至timeout |
| TC-21 | FC-5 / AC-8 | 集 / P0 / 集成 | F-WATCH，显式index=n挂起，timeout30 | 另一连接仅POST X一次，记录完整响应计时 | 严格≤1s，index=n+1且快照含X；环境抖动超限记录实测/复跑或无法验证，绝不按≤2s通过 |
| TC-22 | FC-5 / AC-11 | 集 / P0 / 集成 | 无并发mutation | 显式index0及缺省index两类各请求timeout0/301 | 全部400 `code=invalid_parameter`、非空message统一JSON；即使缺省index走立即分支也必须先校验参数；语法拒绝另见TC-40 |
| TC-23 | FC-5 / AC-11 | 单+集 / P0 / 单测+集成 | 默认参数解析入口；显式相等index=n | 单测不传timeout解析；集成观察3s后变更唤醒 | 默认值精确=30（必要精确证据）；3s挂起仅辅助且随后可唤醒，不据此单独声称证明30s；不假定test-util已启用 |
| TC-24 | FC-5 / FR-8/10、AC-11 | 集 / P1 / 集成 | index=n、timeout2，无mutation | 发请求并以单调时钟计时 | 不提前于2s，测试上限3s内返回200、原index及当前健康快照；超出上限记录环境/失败，无需等待默认30s |
| TC-25 | FC-5 / AC-8、B-7 | 集+e2e / P1 / 集成+e2e | 新/恢复服务版本均0；另有版本n>0 | 大于当前index请求；真实重启后用旧高index请求 | 立即200且index纠正为当前值（重启0），快照为当前恢复健康数据；重启版本不从磁盘延续 |
| TC-26 | FC-5 / AC-10 | 单+集 / P1 / 单测+集成 | TTL2/sweep1/flush1；A及B同批过期；挂起index=n | 控制一次sweep移除A/B；另空sweep对照 | 有移除批次只+1、挂起唤醒且不含A/B；无移除不增。领域单测控制同批，集成观察相同通知；禁止默认flush5导致配置拒绝 |
| TC-27 | FC-5 / AC-10、FR-9 | 集 / P1 / 集成 | 显式index=n挂起，已有A | 单次DELETE A | ≤1s唤醒、index=n+1、快照无A；与TC-16区分正常写路径 |
| TC-28 | FC-5 / AC-10 | 集 / P1 / 集成 | 无TTL到期/其他mutation；显式index=n挂起 | 3s内GET healthz/services/instances/healthy/watch旧index | 原挂起不因查询唤醒，旧index响应和领域版本始终n；查询后再一次注册可正常唤醒 |
| TC-29 | FC-5 / FR-9、QA-3 | 单+集+审查 / P1 / 单测+集成 | sweep延后，TTL2/flush1；A过期未剔除，显式index=n挂起 | 确认healthy=false但无bump；随后迟到heartbeat使A复活；再健康内heartbeat | 自然healthy→unhealthy不单独bump；复活+1、≤1s唤醒且快照含健康A；普通续约不增。事件审查表四类：注册/注销/一次sweep/复活，收回旧“三类”期望 |
| TC-30 | FC-5 / AC-9 | 集+e2e / P0 / 集成+e2e | AppState shutdown Receiver可注入（旧A已定）；真进程挂起多个watch(timeout30) | 集成send(true)；真进程SIGINT并wait | 集成全部1s内200当前快照或断开；进程分支挂起全部结束且5s内exit0、无panic，不等30s。注入集成不能替代SIGINT退出证据 |
| TC-31 | FC-5 / FR-8、B-6 | 集 / P1 / 集成 | 挂起index=n | abort请求；注册X；同原index重连，再以新index挂起 | 重连立即获得X/新index，新相等请求可挂起；无跨请求会话泄漏或异常 |
| TC-32 | FC-5 / NFR-1 | 集 / P1 / 集成 | 50个watch均确认挂起，TTL宽裕，固定环境 | 并发20次healthz/列表，每请求1s截止；注册X | 查询全部成功、无截止超时；50请求全部≤1s获得新快照，无worker被同步等待占死；代码审查长轮询无阻塞等待 |
| TC-33 | FC-5/2 / FR-8、B-11 | 集 / P1 / 集成 | F-PATH，受控任务屏障，无其他mutation | 并发50注册及watch；再与周期flush争用，注销已知集合；join后最终flush/读文件 | id唯一、无panic/死锁；单独注册阶段最终index增50，注销按成功次数增；响应version与同锁快照一致（可合并通知不要求逐事件返回）；最终文件等于最终实例集，无旧写覆盖注销 |
| TC-34 | FC-2 / FR-2、NFR-1/5 | 单+集+审查 / P1 / 单测+集成 | F-SNAPSHOT，受控时间/状态序列，暂停一次IO并保持flush锁 | 并发心跳/结构写穿/周期/关停flush争用；快照读取后再mutation；解屏障读最终文件 | 四路同一flush_lock覆盖“锁内取快照→await阻塞IO完成”；领域RwLock仅短暂克隆，无IO/await，不反向等待flush锁；成功不得清掉并发dirty，失败重置dirty；最终语义状态最新、正式文件始终合法。墙钟回拨/秒级换算本身不能判旧写回退 |
| TC-35 | FC-6 / AC-12 | 集 / P0 / 集成 | enabled=true，新AppState测试setup，原HTTP断言基准 | 回归healthz/注册/心跳/注销/排序服务/healthy过滤/未知服务及实例；模拟mesh list→healthy拉取 | 原路径/方法/JSON/状态不变，404 service_not_found/instance_not_found与ok结构保留；原断言保留，允许setup适配AppState，不以Rust构造签名变更判API不兼容 |
| TC-36 | FC-6 / AC-12 | e2e / P0 / e2e | 原demo脚本、原mesh/provider/config代码，固定端口空闲、隔离registry数据 | 仅运行`bash velox-demo/scripts/acceptance.sh`，采退出/各步骤日志并核对改动清单 | 原断言全通过，mesh周期拉取/转发/LB/TTL链路正常；消费侧零改。原脚本不能证明registry重启/原子写；不扩写demo脚本 |
| TC-37 | FC-3/6 / AC-5/12、Q6 | 单+e2e / P0 / 单测+e2e | a load(None)无env；b两个原进程同临时cwd，默认根./data | a断默认；b先registry注册落盘，再启动config查namespace，写删普通键及registry-state namespace，重启两者再读；反顺序再跑 | a server/TTL30/sweep5沿v0.1，persistence=true/./data/5已定；b config不自动装载registry快照，顶层registry-state.json即便经config创建也与registry子目录快照隔离；各自数据重启完整、互不覆盖，tmp也不在config扫描顶层；config代码不改（布局待AA最终确认） |
| TC-38 | FC-6 / AC-13 | e2e专项 / P1 / e2e性能脚本 | 真v0.1提交`28c91140dd14a29d25dcbc270b2bb23aeb32b2f0`隔离源码/构建，不切当前分支；v0.2最终代码，协议见下 | 同机release/同负载测两版，分注册/心跳/查询统计原始样本、P99和错误 | 每端点v0.2/v0.1 P99≤2才满足；enabled=false仅辅助，不能代基线；缺测/错误/样本不足不得标通过，RQ-2留待证据交付 |
| TC-39 | FC-6 / AC-14、NFR-4 | e2e / P1 / e2e | a有效快照含1存活/1过期；b独立损坏快照；c禁用/缺文件分别 | 每场景独立启动，采新runner绝对日志或logs/velox-registry.log；结合16/18/21/24读日志 | a enabled/data_dir/恢复服务实例数/丢弃1/skipped0；b WARN含文件路径原因、恢复0/skipped1；c启停状态明确；注册/注销/剔除INFO，写失败ERROR，watch唤醒/超时DEBUG。旧A缺skipped字段须AA补；单文件不可同时有效且损坏 |
| TC-40 | FC-5 / AC-8/11、FR-8/10 | 单+集 / P1 / 单测+集成 | 新端点参数入口，RQ-3待AA一致定案；空表v0/恢复有实例v0/版本>0分别 | 各场景缺省index及显式0；显式等值；POST；index/timeout分别abc、负数、u64溢出、空值，搭配缺省index重测 | G2目标：缺省index首次快照立即200；显式0在v0挂起，在v>0立即；显式等值仍挂起。先完整校验，任何语法/范围错均400 invalid_parameter+message，POST405；handler捕获QueryRejection转新端点AppError，不能影响旧路由错误。该目标旧A尚矛盾，见RQ-3 |
| TC-41 | FC-1 / FR-1、B-12 | 集 / P2 / 集成 | F-SNAPSHOT/有效窗口；中文、空格、引号、换行等metadata，服务名沿v0.1 | 注册、读JSON、重open恢复比较 | metadata字符串内容完整相等，JSON合法，id/字段不变；不要求序列化转义后字节顺序一致，不新增服务名校验 |

### TC-38 性能专项执行协议

执行前在报告冻结机器/OS、CPU、Rust与两版准确commit、release构建参数、日志级别、客户端及请求体、数据/日志路径。真v0.1源码可从指定commit导出到系统临时目录（或既有隔离checkout），使用独立target目录；不切当前工作分支、不修改当前代码。先核该commit registry确为内存v0.1。

固定100服务×每服务10实例=1000实例，200心跳/秒均匀排程使每实例5s续约；查询100请求/秒交替services及healthy实例列表，分端点记录；注册churn20次/秒，先注销后补同一槽位，保留不参与churn的稳定样本池，任意时刻不超过1000实例。客户端上限并发32，两版相同；每次请求从发起（含排队另记）到完整响应以单调时钟计时；客户端不能维持计划负载则本轮无效，禁止偷偷减负载。预热≥30s、采样≥60s且注册/心跳/各查询端点每类≥1000有效样本，不足延长。稳定心跳池不因churn产生预期404。

P99用有效成功样本排序后 nearest-rank `ceil(0.99*n)`；全部失败/超时/排队延迟另计，记录计划/实际吞吐与每端点错误率，验收有效测量要求无请求错误（存在错误则先记录缺陷/无法验证，不得靠丢失败样本改善P99）。保留原始逐请求时序/状态、摘要及分端点比值，每项≤2；脚本轻量不等于“仅供参考免验”。后台心跳刷盘每周期最多一次全量写，结构注册/注销/剔除仍逐次写穿，不能承诺全部IO每5s一次。RQ-2为性能证据待交付，不降低AC-13。

## 4. AC 覆盖矩阵（AC × TC，标注 P0/P1 缺口）

| AC | P0/P1用例 | 设计缺口 / 执行证据要求 |
|---|---|---|
| AC-1 | 01(P0)、03(P1)、04(P0) | 有映射；01必须真实kill/restart及落盘heartbeat恢复窗口 |
| AC-2 | 02(P0)、03(P1) | 有映射；恢复后再剔除前同id200 |
| AC-3 | 05(P1)、06(P1) | 有映射；相位/epoch、响应不awaitIO、失败无mutation重试 |
| AC-4 | 07(P0)、08/09(P1) | 有映射；固定N严格边界、配置TTL变更及陈旧反例 |
| AC-5 | 10/11/12/37(P0)、14(P1) | 有映射；G2-3基本/跨项校验边界与G2-5路径待AA同步 |
| AC-6 | 13(P0) | 有映射；各坏文件独立启动含未知版本 |
| AC-7 | 16(P0)、17(P1) | 无规格缺口；500 persist_failed已核销RQ-1，仍待实跑 |
| AC-8 | 20/21(P0)、25/40(P1) | 显式分支有映射且≤1s；缺省分支RQ-3待AA一致规则 |
| AC-9 | 30(P0) | 通道集成+SIGINT进程退出均保留 |
| AC-10 | 26/27/28(P1) | 批次+1/无变更不增；29补复活设计回归 |
| AC-11 | 22/23(P0)、24/40(P1) | 默认值精确单测；QueryRejection映射待AA同步 |
| AC-12 | 35/36/37(P0) | 原断言/setup适配、原mesh/已有demo只运行 |
| AC-13 | 38(P1) | 设计有映射；RQ-2真基线测量尚无执行证据，不能判满足 |
| AC-14 | 18/39(P1) | 18b真实写中kill+审查+分启动日志；没有命中证据不能以tmp模拟核销 |

14AC均至少一条P0/P1设计案例，但**不作“全部无缺口/已满足”结论**：架构待同步、RQ-3及性能/崩溃实跑证据均须关口复核。本文无任何AC实跑结果。

## 5. 边界场景覆盖对照

| 边界 | 对应TC及判据 |
|---|---|
| B-1 落盘失败 | 05b/16/17/34：运行期内存优先、500精确、ERROR、失败dirty重试、空表也能修复 |
| B-2 损坏/半写 | 13/18a/18b：整文件跳过、open清tmp、真实写中kill证据独立 |
| B-3 目录不可创建 | 15：启动非零含路径，区别运行期失败 |
| B-4 宕机>TTL | 07/08/09：落盘age严格>TTL丢弃，无宽限；真实心跳不替代落盘前提 |
| B-5 provider无重注册 | 02恢复正例、09过期404限制、README审查，零改provider |
| B-6 客户端断开 | 31：abort/重连独立请求 |
| B-7 版本回落 | 25/40：重启0纠正旧高index、缺省与显式0分离 |
| B-8 关停竞态 | 19/30/34：最后心跳落盘、SIGINT退出、flush锁竞争 |
| B-9 时钟回拨 | 34区分真丢写与epoch偏差；手工核README已知限制，不新增回拨防御保证 |
| B-10 双registry共享目录 | 手工核README单registry独占声明，不要求多registry文件锁；37b单独验证registry/config同cwd隔离，不能混同两种约束 |
| B-11 并发 | 26/29/32/33/34：批次、复活、50watch、版本快照一致、四路锁完整覆盖 |
| B-12 字符串 | 41：完整roundtrip、沿用服务名语义 |

## 6. 需求问题清单（RQ-x）

本轮未新增需求矛盾。保留历史编号及原文，区分已核销、待执行证据与架构待定；不代替QA关闭GATE2。

### RQ-1（已核销）：AC-7 的 5xx 具体状态码与错误 code 待 AA 定案，用例期望需回填

- 原描述：AC-7写“注册按Q3定案语义响应”，Q3仅给建议默认5xx，具体code待定。
- 证据/裁决：A §4.6/4.7/9与G2 §6已固定500 `persist_failed`、message `registry state persist failed`；内部IO细节仅ERROR。
- 本轮：TC-16注册及注销均精确回填，矩阵不再称待定。核销规格不确定性，不代表AC-7测试通过。

### RQ-2（验收证据待交付，保留）：AC-13 压测验收手段缺失——无压测工具、无 v0.1 基线、采集方法未定义

- 原描述：AC-13要求1000实例/5s心跳/P99≤v0.1同条件×2，但无现成压测资产和基线采集数据。
- 证据：R AC-13/NFR-3；G2 §6/G2-6允许验收期性能专项，明确不能开关v0.2代替v0.1；旧A §1/6.4仍写可选/开关对比，AA待同步。
- 处置：TC-38已冻结可执行测量协议、真正v0.1 commit及结果要求，新脚本限定registry内；执行时补原始样本和比值。当前未测，不自动豁免、不降阈值，不作为新增SA需求问题。

### RQ-3（未消解，交AA及GATE2仲裁）：watch 请求未携带 index 参数时的行为未定义

- **保留原文描述**：FR-8端点形态为`watch?index={n}&timeout_secs={t}`，AC-11仅定义`timeout_secs`缺省按30s处理；客户端不传index的行为（404/400/按index=0立即返回）需求未写。新客户端首次订阅是该场景的自然入口。
- **保留原文影响**：影响TC-40断言范围与实现分支。旧建议“缺省按index=0处理→立即返回”不足以处理服务端version=0，故不再用该建议推定已有一致契约。
- 现行证据：旧A §4.7第1步称缺省0立即返回，第3/4步又只允许不等立即、相等等待；空表与恢复后version=0反例仍成立。G2 §6 RQ-3/G2-2退AA决定：缺少index作为首次快照立即返回；保留Option区分显式0，先完整校验，显式相等仍挂起。
- 本轮TC-40按上述G2目标完整设计三种版本/数据场景、缺省/显式0及参数错误映射；该目标不能宣称旧架构已定案。读取版本尚未消解，TAD WARN留原文与反例供GATE2仲裁；不要求SA降低/重写显式相等语义。

**本轮结果**：41主TC与14AC已逐条修订；RQ-1核销，RQ-2性能专项证据待执行，RQ-3架构矛盾待AA/GATE2确认。G2-1/3/5的dirty流程、禁用基本校验、隔离路径及测试入口仍须AA落成一致版本后复核；本文已写出明确回归目标，不替AA宣告完成。
