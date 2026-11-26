# GATE2 架构评审意见：addlog（子系统日志本地文件 rotate）

> 工序：F-GATE2，F-AA/F-TA-D join 评审；Task ID：QA-1795654240579-7885a11ecff50ad2
> 基线：develop；评审对象 `02-architecture.md`（383 行）、`03-test-cases.md`（v1，178 行）；参照冻结需求与 GATE1 意见。
> 本次只读审查与静态推演，未运行测试、构建或服务；仅新增本评审文件。以下“闭合”表示设计议题已可定案，不表示实现已经通过测试。

## 1. 结论：REWORK（驳回）

共享 `velox-log`、5 个二进制统一接入、200 MiB/含活跃共 3 份、默认 cwd/logs、等级优先级等主方案明确。但 DA 仍需补做关键行为决策：脚本另存的完整 stdout 日志无限增长；超大事件允许突破 AC-13 硬上界；等级校验与空串处理不能兑现 AC-10；轮转告警进入自身队列存在持续轮转反例；故障后的轮转状态恢复未定案。此外，架构测试策略与 v1 用例存在执行路径与编号错位，RQ-3 的需求验收约束仍未冻结。

**退回 AA 修订设计、TA 回填用例；需求问题退回 SA 定向修订，严格限定 FR-4/AC-9、AC-3、FR-5，不改其余 FR/AC。** RQ-1、RQ-2、RQ-5 的技术定案予以保留，不需要重做全部需求或引入生产轮转参数配置。

## 2. 检查项 1：逐 FR 设计覆盖与可实施性

| FR | 设计覆盖 | 结论及证据 |
| --- | --- | --- |
| FR-1 进程自身落盘 | D1、§4.1/4.3/4.4：共享库、统一 init、文件 fmt 禁 ANSI、文件名取包名 | 主路径明确。5 个接入点、格式及保活变量已指定；错误处理和告警反馈链仍需修订，见 G2-3~5。 |
| FR-2 轮转与保留 | D2/D3、§4.2：写前检查、删 .2、.1→.2、活跃→.1；200 MiB、含活跃共 3 | **不通过**。正常文件链可实施，但超大事件无界、告警反馈及中途失败恢复未闭合，见 G2-2/4/5。 |
| FR-3 目录与命名 | D4/D5、§4.3：cwd/logs、dir 文件/env 覆盖、create_dir_all、启动 open 失败返回错误 | 通过设计审查。绝对路径、多级目录和初始化失败契约明确；多实例分目录的脚本适配也已指定。 |
| FR-4 等级配置 | D6、§4.3/4.5：RUST_LOG > env > 文件 > warn，off，非法值 fail-fast | **不通过**。空 LOG__LEVEL 被忽略；EnvFilter 接受 warnn 为 target；来源报文使用“文件或 env”，未确定真实来源，见 G2-3。优先级还需 SA 扩充 AC-9。 |
| FR-5 一致性与默认值 | LogConfig 默认值、5 份 Config 新字段、默认 toml、统一初始化 | 设计主体通过。老配置与未知字段兼容可沿用现有 serde 结构；需求 FR-5 文件名措辞需 SA 定向修正，TC-19 需按代码常量更新。 |
| FR-6 文档与脚本 | D7/D8、§4.7/9：run.sh、demo acceptance、README、development/subsystems、ADR-0002 | **不通过**。启动规范修订方向可接受，但合流重定向制造不轮转的完整副本；测试/文档责任与引用未对齐，见 G2-1/6/7。 |

依赖名及版本范围 `tracing-appender = "0.2"` 已给出，不属于完全未选型。初始化顺序 Config::load → 等级解析 → mkdir/open → worker → subscriber → guard 也已给出。上述缺陷不是一般编码细节，不能交由 DA 临场选择后直接实现。

## 3. 检查项 2：与代码库现状逐项核对

| 抽查项 | 只读证据 | 结论 |
| --- | --- | --- |
| workspace 与集中依赖 | 根 Cargo.toml:3 members 为原有 5 crate；:19 起集中依赖；:26 tracing-subscriber env-filter；Cargo.lock:1361 为 0.3.23 | 架构现状吻合。velox-log 尚不存在，属于拟新增库；不得计成第 6 个运行时二进制。 |
| 5 个 main.rs | mesh:38、config:30、registry:30、provider:34、consumer:35 均 fmt()+try_from_default_env()，失败回退 info | 日志初始化位于 Config::load 之后，接入点及现状等级说明吻合。 |
| run.sh | scripts/run.sh:43 合并 stdout/stderr 追加 `$crate.log`；:50 失败时 tail | 现状引用准确；改名 `.stderr.log` 不能改变 stdout 同样进入该文件的事实。 |
| acceptance.sh 与多实例 | velox-demo/scripts/acceptance.sh:80~88 start_demo 用 `nohup env "$@" ... >>"$log" 2>&1`；:224 启第二 provider | env 透传与第二实例风险属实；给第二实例传 `VELOX_PROVIDER__LOG__DIR` 可隔离进程内文件。仓库无根 scripts/acceptance.sh，实际影响文件是 velox-demo/scripts/acceptance.sh，02 已使用正确路径。 |
| cwd 与 run.sh 范围 | scripts/common.sh:12~13 解析根并 cd；:16~18 清单只有 core 三个服务 | cwd/logs 方案吻合；但 02:328“run.sh start → 5 文件出现”错误，demo 两个还需另外启动。 |
| 5 份 Config serde | mesh/src/config.rs:5~11；config:5~10；registry:7~12；provider:8~14；consumer:8~14 均顶层 Default/Deserialize、serde(default) | 加默认 LogConfig 的结构可行；serde(default) 填补缺省值，未知字段容忍还依赖不设置 deny_unknown_fields。 |
| 空 env 的现状 | env_override：mesh:98、config:95、registry:71、provider:127、consumer:122，均 filter 非空 | 现状描述正确，但不能据此让新日志等级空串绕过冻结 E7/TC-15 的拒绝要求。 |
| development §1/§3.4/§7 | development.md:17~18 集中依赖与新增依赖论证；:58~66 日志规范与启动字段；§7 要求架构决策附 ADR | QA-2 张力真实，修订 §3.4 的方向合理。ADR 流程引用成立。 |
| ADR 目录 | velox-mono/docs/adr/ 当前仅 0001-mono-repo-layout.md | 0002 编号可用。02 已承诺实施时新增 0002；此关口不把“尚未实现的 ADR 文件缺失”单独判成交付缺陷。 |
| 包名与 gitignore | 5 份成员 Cargo.toml 的 name 与 AC-2 一致；.gitignore:31 为 logs/ | 文件命名与忽略规则吻合。 |

## 4. 检查项 3：依赖及备选方案取舍

1. **共享 crate vs 复制五份：成立。** 统一配置和轮转算法能降低接入遗漏，新增库不改变运行时子系统边界。实施清单需明确根 `[workspace.dependencies]` 的 `velox-log = { path = "velox-log" }` 与五成员 `velox-log.workspace = true`，02 §4.6 当前示例未完整列出。
2. **tracing-appender 只用于非阻塞层：方向成立。** 只需要新增一个直接外部依赖，官方时间轮转不能实现大小阈值，保留 tracing fmt 的选择合理。但“只新增一个直接依赖”不等于没有新增传递依赖。
3. **自研 writer vs file-rotate：论证不足以认定自研风险低。** §7-B 只有“语义未必一致”的推测，没有选一个具体版本核对计数、命名及大小轮转 API。既然自研方案已有 G2-2/4/5 的实质漏洞，须补一组具体 API/版本对比或引用，然后定案继续自研及其故障责任；不要求一定换库。flexi_logger 的整栈桥接取舍针对文中所述方案可以成立，不应推导为所有第三方 writer 都会丢 tracing 字段。
4. **版本与 MSRV：仅部分证实。** 只读读取上游 tag `tracing-appender-0.2.4` 的 Cargo.toml，确有 rust-version=1.63.0；依赖还包括 `time = "0.3.2"`（范围），parking_lot 为 optional。02 §4.6 漏列 time，不能仅凭顶层 MSRV 推导最终锁定依赖图适配 Rust 1.80。要求在 ADR/实施锁定版本中说明实际传递依赖兼容边界，本次未解析/构建依赖，不能宣称已通过 MSRV 验证。
5. **队列与 flush：披露仍需准确。** 上游 non_blocking.rs 默认 lossy=true、容量 128000；WorkerGuard::drop 使用有时限的发送/等待，并非无条件排空保证。worker.rs 写失败后继续循环、错误并不自动修复底层 writer。02 §8-R1 已披露队列丢事件取舍，但 §4.4“run 返回即全部日志写完”及 §4.2.3 对失败恢复的保证仍需限定条件，见 G2-5。

上游只读取证地址：

- https://github.com/tokio-rs/tracing/blob/tracing-appender-0.2.4/tracing-appender/Cargo.toml
- https://github.com/tokio-rs/tracing/blob/tracing-appender-0.2.4/tracing-appender/src/non_blocking.rs
- https://github.com/tokio-rs/tracing/blob/tracing-appender-0.2.4/tracing-appender/src/worker.rs

这些是对选型声明的静态核对，不是对尚未写入 Cargo.lock 的实际解析版本的运行验证。

## 5. 检查项 4：AC-1~14 的实现路径与可测性

| AC | 架构路径／v1 用例 | 结论 |
| --- | --- | --- |
| AC-1 | §4.3/4.4 init；TC-01 裸进程 | 可测。默认 warn 下需触发实际 WARN/ERROR；TC-01 的普通启动/注册 INFO 不能单独作为文件内容探针。 |
| AC-2 | D5 五个包名；TC-02 五进程 | 可测。run.sh 仅 core 三个，另启两个 demo；TC-02 已写明，02 §6.2 必须同步。 |
| AC-3 | §4.2 写前检查；TC-04 | 待修 G2-2，当前事件容差无界，不能视为量化完成。 |
| AC-4 | keep=3 归档链；TC-05/03 | 正常路径可测；失败中途重试和额外诊断文件需按 G2-1/5 收敛。 |
| AC-5 | 串行 writer + 后台队列；TC-06 活体并发 | 执行路径待补。私有 writer 构造参数仅服务单测，不能直接给真实二进制降低阈值；必须保留真实阈值活体路径或定案测试专用进程夹具，不能以纯 writer 单测替代 AC-5 的 e2e。 |
| AC-6 | init create_dir_all；TC-08 | 可测。直接 writer::new 明确要求目录已存在，不能用它代替 init 的 mkdir 验证；02 §6.1 用例落点需区分。 |
| AC-7 | Config 文件/env 合并 + init dir；TC-09 | 可测，须定义隔离的 subscriber/子进程测试路径，避免反复安装全局 subscriber。 |
| AC-8 | 共用过滤器；TC-11/12 | 可测但双轨预期有冲突。TC-12 e2e 备用分支把写文件失败的 stderr 诊断当作“文件中 ERROR 行”，不成立；应通过能写文件的正常 error! 探针验证。 |
| AC-9 | 配置合并、D6；TC-13，优先级扩展为 TC-14 | 可测；RQ-3 未冻结，须 SA 定向扩充原 AC-9 后 TA 映射 TC-14。 |
| AC-10 | validate_level/config 校验；TC-15 | 不通过设计审查，见 G2-3：空串过滤、warnn 作为合法 target、真实来源未记录。 |
| AC-11 | serde 默认 + LogConfig；TC-18 | 可测，需分别验证无 log 节及包含未知字段，恢复/重启归档另走 TC-03。 |
| AC-12 | CI 四命令、各 Config 默认测试；TC-19 | 实施后可测。TC-19 仍要求生产轮转数值/单位解析，与 D3“不开放配置”不符；应测共享常量及 writer 注入，Config 只测 dir/level。 |
| AC-13 | 小阈值 writer 构造 + 常量检查；TC-07 | 注入议题闭合，但硬上界仍不成立（G2-1/2）。计量须区分文件逻辑字节与 du 分配块；小阈值下 du -sk 可因块对齐大于 keep×max，不能直接拿它作字节阈值等价断言。 |
| AC-14 | §4.7/9 文档清单；TC-23/20 | 方向可测。TC-20② 按静默回填，①仍须检查 info 下版本/地址/配置路径/关键值，不能仅一条缺字段的“日志有效配置”记录代替全部启动规范。 |

**矩阵有映射，不代表所有用例已能执行。** 尤其 AC-3/5/8/10/13 目前有边界或执行口径冲突。02 §6.2 仍用旧 TC 编号（如 T3 标 TC-18，v1 实为 TC-22；优先级标 TC-13/16，v1 实为 TC-14），必须整体按 v1 引用纠正，不改测试需求本身。

## 6. 检查项 5：RQ-1~RQ-6 逐条仲裁

| RQ | 裁决 | 依据、责任与时点 |
| --- | --- | --- |
| RQ-1 = QA-1/OP-4 | **闭合（注入能力这一议题）** | 接受 §4.2.1/§9：生产常量 200 MiB/3，不新增生产配置；同一 writer 构造函数接受测试小阈值。原 AC-13 已允许小上限模拟，不必为内部测试接口新增用户 FR。TA 在实施前回填 TC-04/05/07 的模块内测试路径并检查生产常量；TC-06 活体注入不能据此宣称闭合，归 G2-6。 |
| RQ-2 = QA-2 | **闭合（规范取舍）** | 接受 §9-QA-2：默认 warn 下 INFO 启动行静默，info 排障时仍完整记录，初始化错误始终 stderr 可见。DA 随 FR-6 实施修改 development.md §3.4/ADR/README；TA 在执行前回填 TC-20②。无需对原默认 warn 需求反向提级启动行。 |
| RQ-3 = QA-4 | **需改需求——退回 SA 定向修订** | 技术链采纳 D6，但 TA 补用例不能替代 SA 冻结验收约束。只修 **FR-4 与 AC-9**：把 RUST_LOG > VELOX_*__LOG__LEVEL > 文件 > warn 写成已定案契约，AC-9 增加并存场景（含高优先级生效、低优先级无效）。AA 对齐校验语法和空串；TA 将 TC-14 映射 AC-9。须在重新 GATE2/交 DA 前完成，不延期至实施后。 |
| RQ-4 | **需改架构，并退回 SA 定向修订 AC-3** | “≤上限+单条日志行”只有在记录大小有明确上界且与 AC-13 一致时才可判定；02:147 允许任意超大事件写穿，当前不可接受。AA 先确定不突破单文件/总量硬上界的写入及超长事件策略、字节定义；SA **只修 AC-3** 的“日志块/容差”定义，与该策略一致。不得顺便放宽 AC-13 或重写其余 AC。TA 更新 TC-04/07。 |
| RQ-5 = QA-3 | **闭合** | 采纳 Q3 纳入全部五二进制、D5 活跃名=CARGO_PKG_NAME、Q9 固定 .1/.2，与现有 AC-2 的五个确定文件名一致。不需要再把 AC-2 改成新的悬而未决表述；TA 移除相应用例的待定标注即可。 |
| RQ-6 = QA-5 | **需改需求——退回 SA 定向修订，仅 FR-5** | §6 原文的 RQ-6 是 demo 开发配置文件名措辞，非 stdout 重定向。FR-5 改成“各子系统 config 目录的开发默认 toml，demo 两份为 demo-provider.toml/demo-consumer.toml”，其他 FR/AC 不动；用例侧已按正确路径。属于低严重度修正，可并入本次 SA 定向修订。 |

脚本重定向和其余遗留按下一节 OP-6、G2-1 等单列，不借 RQ-6 概括而漏裁决其原文。

## 7. OP-1~OP-10 逐条处置与挂起去向

| OP | 仲裁结果 | 落点／遗留责任 |
| --- | --- | --- |
| OP-1 demo 范围 | 闭合：纳入五二进制 | D1/D5、Q3；TA 回填 TC-02/17。 |
| OP-2 保留计数 | 闭合：含活跃总计 3 | D3、Q1；TC-05；诊断副本不得绕过上界，G2-1 仍挂 AA。 |
| OP-3 等级渠道与优先级 | 链定案，验收约束挂起 | D6、Q5；SA 修 FR-4/AC-9；AA 修 G2-3；TA 回填 TC-13/14/15。 |
| OP-4 参数配置/注入 | 内部注入定案；生产无参数配置 | D3、RQ-1；模块内 TC-04/05/07 解除能力挂起；活体路径按 G2-6 处理。 |
| OP-5 单位/归档/目录 | 闭合：200 MiB、.1/.2、cwd/logs+覆盖 | D3/D4/D5、Q2/Q4/Q9；大小超长处理另挂 RQ-4，不用“单条事件”模糊上限。 |
| OP-6 脚本双写 | **需改架构，未闭合** | 不再同文件双写的方向正确，但完整 stdout 另存不轮转仍违反核心目标；AA 修 D7/§4.7/§7，TA 扩充 TC-21/22。 |
| OP-7 初始化失败 | 闭合：fail-fast | Q8、§4.3；TC-10。运行中恢复另挂 G2-5，不混同启动策略。 |
| OP-8 非法配置 | **需改架构/用例，未闭合** | 新 LOG__LEVEL 空串与 warnn 应按 AC-10/E7 拒绝；RUST_LOG 语法边界明确；轮转参数为常量，不存在生产单位字符串解析；AA/TA 修 G2-3/6。 |
| OP-9 轮转选型与依赖 | 主选型明确，细节挂起 | 继续自研并非被禁止，但须修故障状态和告警反馈，补 file-rotate 具体比较、依赖兼容说明及测试接口，AA 负责 G2-4/5/6/7。 |
| OP-10 记录日志有效配置 | 闭合：info 级记录、默认静默 | §4.3 第5步/§9-QA-2；TC-20 回填。DA 明确记录最终生效等级（含 RUST_LOG）、目录、阈值和份数，启动规范其他必含字段仍保留。 |

## 8. 检查项 6：两项行为变更披露

- **info→warn：披露充分，处置方向通过。** 02 §5、§6.2、§8-R5、§9-QA-2 已说明 INFO 启动日志静默，排障使用 RUST_LOG=info，历史 INFO grep 需设等级。实施时必须同步 README/development/PR 说明；TA 回填 TC-01 探针及 TC-11/20/21/22，不能继续用默认 INFO 启动行作默认落盘证据。
- **非法等级回退→fail-fast：已披露，但机制不完整。** 02 §4.3/§5 已说明行为变更；不过“非法”必须按可执行语法界定。EnvFilter 中 warnn 是合法 target，空串也不是自动解析失败。需要补校验和配置来源记录，明确新 LOG__LEVEL 空串及 RUST_LOG 空串/模块指令的区别，再覆盖 TC-15。不能仅把 try_new/try_from_default_env 的返回值换成 `?` 就宣称完成。

## 9. 问题清单与定向修改要求

### G2-1（高，阻断）：脚本完整日志副本无限增长

**证据：** 02:194~205 保留 stdout fmt；:283/:286 两脚本 `>>*.stderr.log 2>&1`；:347 论证诊断副本。01 FR-2/AC-4/AC-13 要求数量与大小上界。

**静态反例：** 每输出一个 WARN/ERROR，同一正文同时进入轮转文件和 `.stderr.log`；info/debug 排障时同理。轮转清理 `.log.2` 不会删除 `.stderr.log`，故它可以任意增长，且额外增加文件数。文件名中的 stderr 不会让 shell 只捕获 stderr。

**AA 修改：** 改 D7/§4.7/§7，明确 stdout 与 pre-init/panic stderr 分流及诊断文件的生命周期/上限/计数归属；确保持续常规日志不会追加到不受管控的本地副本，并保留初始化失败可查。不能以“默认 warn 量小”或“诊断文件不算”替代核心容量约束。TA 将持续输出后检查全部相关文件加入 TC-21/22/07。

### G2-2（高，阻断）：超大事件突破硬上界，RQ-4 未闭合

**证据：** 02:147 允许单条超 max_bytes 仍整条写入；:318 又断言 Σsizes≤keep×max；01 FR-2、AC-3/13。

**静态反例：** 注入 max=M，连续写入多条长度 2M 的缓冲，每次先轮转仍将 2M 写入单文件；三个保留文件可达 6M。按“单条事件”定义容差并未给它数值上界。

**AA 修改：** 定案超长记录和正常轮转的字节边界策略，保持 AC-13；同时说明已有超限旧活跃文件的首次写入处置及验证时点。SA 仅按 RQ-4 修 AC-3。TA 覆盖等于阈值、跨阈值、单事件超过阈值及重启旧文件。

### G2-3（高，阻断）：等级校验不满足 AC-10/E7

**证据：** 02:187~189 使用 EnvFilter 并忽略 RUST_LOG 空串；:239~248 LOG__LEVEL 沿用过滤空串的 env_override，来源报文为“文件 或 env”；03:87 TC-15② 明确要求空 LOG__LEVEL 失败。已读本地 tracing-subscriber-0.3.23 `src/filter/env/directive.rs:215~225`：无法按级别解析的裸词作为 target、级别 TRACE；`env/builder.rs:168~172`：空串可正常构造过滤器。

**AA 修改：** 写出可操作的 level 校验规则，区分裸级别、明确模块指令与 RUST_LOG 的兼容语法，保证新配置的 warnn/空串按 AC-10/E7 拒绝；新日志等级 env 读取须保留“已设置为空”的事实，不能影响其他存量 env 的空值惯例。记录最终生效值的准确来源，错误应含实际值及具体文件路径或变量名，不用“或”代替来源。若更高优先级覆盖非法低级配置，应把该契约写入 FR-4/AC-9 的定向修订和 TC-14。

### G2-4（高，阻断）：轮转告警进入自身队列形成持续轮转

**证据：** 02:153 与 §8-R2 用“每轮转恰发一条事件”推导不可能风暴；构造函数允许测试小阈值，§6.1 还要求测试“轮转中再轮转”。

**静态反例：** 当 max 小于一条轮转告警的格式化长度时，业务记录触发轮转 → 产生告警 A → 写 A 触发轮转 → 产生告警 B → 写 B 再触发轮转。即使业务已停止，反馈链也可持续；每轮恰好新增一条不能证明终止。仅直接构造 writer 且不装 subscriber 的单测无法覆盖这条反馈路径。

**AA 修改：** 选择明确的无反馈运维事件报告路径或自事件抑制机制，保持 NFR-4 可观测；同步修 §4.2.3/§8-R2 的无风暴论证。TA 增加真实 subscriber+后台队列下“小阈值、业务停止后能静止”的验证场景，不能只测直接 write。

### G2-5（高，阻断）：错误后的状态恢复与关停保证未定义完整

**证据：** 02:121~126 以 `?` 退出写入，未定义失效句柄/计数处理；:137~142 是破坏性 rename 链；:154 承诺后续自动重开；:219 将 guard drop 等同所有日志完成。上游 worker.rs 在 writer Err 后继续取下一条，不替项目恢复文件状态。

**待定决策：** 例如已成功“活跃→.1”但新建活跃失败，下一条若仅 ensure_open 重新打开并用旧 written 计数，可能再次推进归档或计数错误；普通写失败后 current 未必变成 None；外部 unlink 后原 fd 仍可写而目录看不到日志，不能等同 open 失败。已有 fd 的 chmod 也不能保证触发写失败。

**AA 修改：** 定义每个删除/改名/open/短写/flush 失败后的 current、written、归档状态和下次动作，限频报告覆盖所有失败出口并标明 ERROR。明确删除/改名中途失败的恢复及允许丢失范围，不能交 DA 决定。注明 guard 有界等待及后台业务任务结束与 guard drop 的先后契约，避免无条件不丢日志承诺。TA 对 TC-24 使用可确定触发的 IO 故障注入，不能只依赖 chmod/删除句柄。

### G2-6（中，阻断可测性定稿）：测试路径、测量口径与 v1 用例不同步

**证据：** 02 §6.1 禁止单测 init，但 03 §1/TC-12 等需要完整过滤/初始化；writer 为 pub(crate)，生产 init 固定阈值；02:331 将 200 MiB 真值验证整体列为可选，而 TC-06 是活体并发轮转；02:328 run.sh 五文件断言及旧 TC 引用错误；TC-19 仍测不存在的轮转生产解析项。

**AA/TA 修改：** 定案集成测试隔离方式（子进程或非全局 subscriber 构造接口）与 worker 测试手段；TC-06 明确真实阈值活体执行或测试专用进程夹具，不让 DA 另选架构。TC-12 使用正常 error! 探针，写失败 stderr 不作为文件 ERROR 的等价验证；TC-19 改为常量及注入测试；TC-07 明确逻辑字节求和，物理块占用另报。按 v1 重填所有 TC 号与 OP/RQ 状态，core+demo 才是五进程。

### G2-7（中，随返工修正）：选型与依赖约束论证仍需补实证

**证据：** 02 §4.6/§7-B：file-rotate 语义只有推测、传递依赖漏 time、顶层 MSRV 被当作完整兼容论据；workspace 示例遗漏 velox-log path dep。§6.1 称沿用 Uuid 不新增 dev-dep，但新库依赖表没有 uuid。

**AA 修改：** 补具体版本/API 比较；在 ADR-0002 计划内容中列清最终直接/关键传递依赖与锁定策略，不声称本关口已验证未解析依赖的 MSRV；补齐 path dep 和测试 Uuid 的 workspace 引用或换成无需该依赖的隔离命名方案。ADR 文件可以随实施落地，不要求本关口提前制造代码产物。

### G2-8（中，需求冻结阻断）：RQ-3 未补验收链，RQ-6 措辞未修

**证据：** 01 FR-4/AC-9 仅 env>文件，Q5 仍是建议；03:158 RQ-3 明确要求 GATE2 后补 AC；01 FR-5 的 demo 文件名仍不准确。

**处置：退回 SA 定向修订。** 只改 FR-4/AC-9（优先级及高优先级覆盖行为）、AC-3（配合 RQ-4 的边界定义）、FR-5（真实开发 toml 名）；本次不修改其余 FR/AC、不改业务范围、不增加生产轮转配置。SA 冻结后 AA/TA 仅同步对应设计和矩阵。

## 10. 遗留挂起项与再次关口的条件

| 项目 | 责任与关闭条件 |
| --- | --- |
| G2-1~5 | AA 更新相应 D/§4/§7/§8，并给出可直接实施的 IO/过滤/反馈/容量契约；TA 同步对应反例用例。 |
| RQ-3、RQ-4、RQ-6 | SA 按 G2-8 限定范围修订，随后 AA/TA 回填映射；不重开已闭合 RQ-1/2/5。 |
| G2-6/7、OP-3/6/8/9 | AA/TA 完成测试接口、实际活体路径、依赖论证与编号纠正，再提 GATE2。 |
| implementation/ADR/文档更新、最终 CI 与 T3 | 属下游 DA/TA 实施与执行工序，不是要求本次评审运行测试；ADR-0002 和 §3.4 修订仍是明确交付任务。 |

没有理由将上述确定的设计冲突以“实现时再定”留给 DA。本报告不要求当前运行任何测试，也不判定尚未实现的功能测试失败。

## 11. 取证与审查边界

已完整阅读 01 需求、GATE1 意见、02 架构、03 v1 用例；只读抽查代码、脚本、依赖清单、开发规范和 ADR 目录，并静态读取上游 appender 源码。未执行 cargo test/build/clippy/fmt、acceptance、服务启动、轮转写入或故障注入。网络 crates.io 元数据读取未成功，不据此作版本兼容结论；上述 GitHub tag 源码读取成功。

评审时 git 分支为 develop；已有未跟踪 `docs/specs/_pipeline-state.md`、`docs/specs/addlog/`、`velox-mono/docs/brd/`，本关口不因流水线在途文档未提交而驳回。本次唯一新增文件是 `docs/specs/addlog/02-architecture-review.md`。

输入文档 SHA-256（防止把后续修订与本次结论混同）：

- 01-requirements.md：`246132785fc544a667b8cc4f465832384f572d488922d66ebb6c8ceb94572fb4`
- 01-requirements-review.md：`f54a5d128cd2ee13bad8974714cceb0260d49719ccce4c41555e6db41c085a31`
- 02-architecture.md：`7350f4450a21ce603e96118fc1b9976610a3084eac4c2db91ab0f3c7f4083dc7`
- 03-test-cases.md：`2f9250d91b884f5d6854d7d7cb272e647690bde443a89ce0b56830bee8ceb2bd`

PIPELINE addlog GATE2 REWORK AA修复无界日志副本、超大事件上界、等级校验、告警反馈及IO恢复并补齐测试接口/依赖论证；TA回填v1用例与矩阵；退回SA定向修订FR-4/AC-9、AC-3、FR-5，其余FR/AC不动

---

# GATE2 架构复审（第 2 轮）：addlog（子系统日志本地文件 rotate）

> 工序：F-GATE2（批量两 slug 之一），Task ID：QA-1795666025105-ff31c521c7fe3348；评审人：QA；日期：2026-11-26
> 性质：**GATE2 返工复审第 2 轮**（非终验、非 GATE3；不写 05-acceptance、不执行 AC 满足度核对）。第 1 轮 REWORK 原文（§1~11）全文保留于上方存档，未删改；本轮对照其 §9 G2-1~8 返工契约逐条复审。
> 被审对象与指纹（动手前重算 SHA-256，全部与锁定值一致）：
> - `02-architecture.md`（AA 返工定稿+映射同步，555 行）：`a154e26483c766eb590b67618dcdd1ef68c95fabb9b1290cd1781cb20d16808d`
> - `01-requirements.md`（冻结，194 行）：`ce54e32520d21ca953ffc46d74bba564cb9bc0cb537cd9e0e3452d42643554df`
> - `03-test-cases.md`（TA v1 定稿，178 行）：`1aaa6f0f66d3a2d531b2a1748c1186380681e86f18ac406b18e19116ca9a30b8`
> - `01-requirements-review.md`（GATE1 定向复审 PASS 11:06）：`4a8af5836d902dabc0e5963ac5b50fb22415abe665d88b0a01243708b60309da`
> - 第 1 轮本评审文件：`c1db832dfffd8b832440bac166a308c52db0642ca56c95fee32320e8b2fe1e70`
> 分支基线：develop @ af28e65。git status 在途修改（icl 规格、velox-config 纯 rustfmt 格式化）与本 slug 无关，不影响本轮。本轮只读评审，除追加本章节外未改动任何文件；未运行构建/测试/服务。

## R2-0. 结论：**PASS（返工复审通过）**

第 1 轮 REWORK 契约 G2-1~8 逐条闭合：无界脚本副本（G2-1）、超大事件硬上界（G2-2）、等级校验与来源绑定（G2-3）、告警反馈自激（G2-4）、IO 故障状态机与关停保证（G2-5）、测试接口与 v1 用例同步（G2-6）、选型/依赖实证（G2-7）均已在 AA 返工定稿给出章节级可实施定案，TA v1 按 §6.3 合同回填且抽验无接口分歧；G2-8 经 SA 四处限域修订并 GATE1 定向复审 PASS，需求已冻结且 AA/TA 映射同步。**无阻断项**；仅余 3 项 P3 备注（§R2-6），不构成驳回条件。可交 DA 实施。

## R2-1. G2-1~8 逐条闭合核验（章节级证据）

**G2-1（脚本无界副本）——闭合。** AA：D7（02 §3 决策表"stdout=/dev/null，stderr 用管道回显；不创建 stderr.log/nohup.out/诊断归档"）；§4.7 给出两启动点的具体进程替换脚本（run.sh/acceptance.sh 均 `</dev/null >/dev/null 2> >(trap '' HUP; exec cat >&2)`），论证 nohup 在 stdout=/dev/null、stderr=管道下不会另造 nohup.out、管道为内核有界缓冲非磁盘文件、cat 随 EOF 退出；§4.2.1 不变量 4 明确"stdout/pre-init stderr 不在本地另存，新增诊断文件不能以'不属于日志'规避计数"；§4.2.3 精确清理遗留 `<stem>.stderr.log` 与 `.log.N`（N≥3）并 WARN，不误删他包；§4.7 初始化前失败"可查渠道是本次启动 stderr，而不是持久化第四文件"；§7.3 否决 stdout 再存/tee/第四文件分配。TA：TC-07②（全目录检查无 stderr.log/nohup.out/第二运行名副本）、TC-21①（持续大量输出后全部相关文件仍受管）、TC-22（第二 provider 分目录、旧精确副本清理）均已按第 1 轮要求落位。bash 语义复核：nohup.out 仅在 stdout 为终端时产生，/dev/null+管道组合成立。
**G2-2（超大事件突破硬上界）——闭合。** AA §4.2.2：`BoundedMakeWriter` 每事件独立缓冲容量 R=min(M,64 KiB)，drop 时只提交一次，超长保留 UTF-8 完整前缀+R 内 ASCII 标记 ` [truncated]\n`，"不会创建第二个无限缓冲或第二条队列记录"；writer 侧对单次输入 >R 同规则二次防御；§4.2.1 不变量 2/3（任一写入不越过 M、总和 ≤3M，短写/失败/重试保持）；"触发允许越界的容差为 0 字节"，提前轮转未用空间 ≤R−1——数值自洽（R<M 时触发轮转必要求 written>M−R>0，故未用空间 <R）。已有超限文件处置与验证时点：§4.2.3 init 在装 subscriber/接客前完成规范化，>M 文件 `set_len(M)`+WARN，活跃 len=M 不在构造期滚动、首条非空记录写前轮转；规范化失败即初始化失败不接客。SA：冻结 AC-3（01:114）R 定义、0 容差、R−1 提前量、截断、启动前收敛逐项一致，AC-13 未动。TA：TC-04 覆盖 M−1/恰好填满/不足下一块/单事件 >R/连续 2M 反例/逐次量长六序列，TC-03 覆盖旧活跃/归档超限与半途重启。
**G2-3（等级校验）——闭合。** AA §4.4.1：`var_os`/可注入 EnvInputs 区分 unset/已设空/非 UTF-8；先选最高来源、只校验生效字符串；RUST_LOG="" 视为未设置、空 LOG__LEVEL 与空文件 level 非法、仅空白非空 RUST_LOG 非法、非 UTF-8 报错标变量名；RawLogConfig `Option<String>` 记录 file_level_present，空文件 level 保持 Some("") 不降级为 Default。§4.4.2 新渠道严格白名单语法（裸六等级或显式 `target=level`，target 字符集规则；`warnn`/`""`/`svc=warnn`/`warn,,svc=info` 拒绝）；§4.4.3 RUST_LOG 保留 EnvFilter 完整语法、裸 warnn 为合法 target 接受、`svc=warnn` fail-fast。来源绑定与错误上下文（值+具体变量名或绝对路径+log.level 键，不写"或"）及 §4.4.3 七行合法性例表齐备。上游语义复核（本轮重读本机锁定源码）：tracing-subscriber-0.3.23 `directive.rs:215~225` 裸词无法按级别解析转 target+TRACE、`builder.rs:168~172` 空串按默认指令构造成功——与 FR-4/AA 取舍一致。SA：冻结 FR-4（01:77-81）/AC-9（01:120）与 §4.4 逐项一致。TA：TC-13（三渠道+严格优先级+来源绑定）、TC-14（例表全行）、TC-15（两套语法分流+准确来源断言）落位。
**G2-4（告警反馈自激）——闭合。** AA §4.3.4：`ReportSink::record` 类型化事件+固定原子计数；明文"任何 writer/截断/队列内部报告都不得调用 tracing，不得发回 NonBlocking，不得写本地诊断文件"；因果图仅"业务事件→writer→stderr"，无 stderr→subscriber 回边，故业务停止后小阈值也不会自激轮转——第 1 轮反例（max 小于告警格式化长度形成轮转链）在该结构下不成立；worker 直接 `writeln!(stderr.lock())` 并忽略 stderr 写失败（不用会 panic 的 eprintln!）；限频 60s 按类别、首次立即、被抑制次数累加计数；只有启动线程与文件 worker 执行 stderr IO。§8 风险表同步更新。TA：TC-25 真实 subscriber+NonBlocking+队列、M 小于报告文本长度、业务停止后两个观测窗计数静止、无自激断言，且明文"不能只用无 subscriber 的 writer 单测证明无反馈"。
**G2-5（故障恢复与关停保证）——闭合。** AA §4.3.1 状态机（Phase 枚举；`written: Option<u64>` None=未知、严禁旧计数透过 reopen；FileOps 含 len/dev/ino/nlink 元数据接口；FaultFs 每操作第 N 次失败/短写/WriteZero/Interrupted/flush 失败，cfg(test)）；§4.3.2 逐步失败表覆盖 remove .2/.1→.2/活跃→.1/open 失败/metadata 失败/短写/Interrupted（≤3 次）/write 0 或错误/flush 失败/身份检查（unlink 后 dev/ino 不符、nlink=0 → Reopen 不写不可见 inode）/Reopen 时 >M——每行给出 current/written/归档状态与下一条动作；NotFound 容忍空洞、phase 仅成功后推进、不做递归；§4.3.3 短写按实际 n 更新 written、flush 分 phase 语义、保存 Result 不以 `?` 跳过报告；§4.6 LogGuard 有界等待（0.2.4 guard 100ms/1000ms 握手，如实引用）、五入口顺序契约（保存 JoinHandle→cleanup→producer 先结束→guard 后释放→不 process::exit）、"尽力排空"与允许丢失条件列明、不再承诺"run 返回即全部写完"。TA：TC-24 逐故障点+guard 2s 预算分支+队列满专项，FaultFs 确定注入，不用 chmod/unlink 错误假设。
**G2-6（测试接口/口径/编号同步）——闭合。** AA §6.1 五条分层接口（私有 writer::new 注入；build_dispatch 局部 Dispatch 不装全局 subscriber；全局 init 仅子进程与真实二进制；TempDir 无 uuid；FaultFs/可注入时间源）定案，消除双轨；§6.2 TC-06 活体夹具定案（velox-log 测试可执行文件自举子进程、cfg(test) 内部 `VELOX_LOG_FIXTURE=1`、真实 subscriber+队列+同一 writer、TCP 夹具 8 客户端×≥64 请求、≥4 次真实轮转、marker 前后窗可 grep、默认随 `cargo test --workspace` 执行、父端超时+kill+wait）；§6.3 逐 TC 回填合同（TC-01~25 全表）。TA v1 已按合同回填：TC-06 取消双轨并引用 §6.2；TC-12 正常 `error!` 探针（stderr IO 报告不作文件 ERROR 等价）；TC-19 常量断言（209715200/3/R=65536/队列 1024）+注入构造、删除生产轮转解析项；TC-07 `metadata.len()` 逻辑字节求和、du 仅另报；§1 分层表与 §6.1 一致；旧 TC 编号错位全部纠正（AC-9 映射 TC-13/TC-14；core+demo 五进程口径）。
**G2-7（选型与依赖实证）——闭合。** AA §7.2 file-rotate 0.7.6 具体构造与 API/源码行为表（AppendCount(2) 不含活跃得 .log/.1/.2；Bytes 变体精确拆写 vs BytesSurpassed 整 buffer 写穿不选；new() expect/ok 语义；rotate 先移位后清理；运行中 file=None 送 void；manifest 连 Compression::None 仍带 chrono/flate2），并据实证给出继续自研的真实理由（错误/诊断/历史收敛契约），不再以"150 行简单"论证；取证来源列出 static.crates.io 抓取的 Cargo.toml/src。§4.8：workspace members 补 velox-log、五成员 `velox-log.workspace = true`、新库依赖仅 tracing/tracing-subscriber/tracing-appender/serde/anyhow；传递依赖如实披露（crossbeam-channel ^0.5.6、time ^0.3.2、thiserror 2、parking_lot optional 不启用），`cargo update -p time --precise 0.3.36` 锁基线步骤、ADR-0002 必载内容清单；明确"只证明这些包的声明，不证明整个 workspace 在 Rust 1.80 已构建成功"，1.80 check 列为 DA 实施验证项而非本轮结论——无 MSRV 越权宣称。Uuid 依赖以删除方案消解（§2/§4.8/§6.1.4：temp_dir+pid+原子计数，无 uuid/tempfile dev-dep，不声称"用了 Uuid 却未声明"）。本轮复核 Cargo.lock：无 tracing-appender/file-rotate/time（与新库拟新增一致，lock 未提前污染）。
**G2-8（需求冻结链）——闭合。** SA 四处限域修订（FR-4/AC-9、AC-3、FR-5）经 GATE1 定向复审 PASS（01-requirements-review.md §R0~R6，11:06，含越限核对：其余 FR/AC 逐字未动、AC-13 未动）；AA §9.2 存档建议文本并注明冻结事实；AA §1 映射表与冻结文本逐项一致（本轮抽验 FR-4 优先级链/AC-3 R 与 0 容差/FR-5 实名五份 toml 均与 01 及仓库实际文件名吻合）；TA §6 RQ 清单 0 项、§7 OP-1~10 全部闭合留痕。RQ-1/RQ-2/RQ-5 未被重开，生产轮转参数未新增。

## R2-2. 三方一致性（A/T/R 交叉核对，抽验）

TA 03 头部声明"按 A §6.3 合同逐条对齐、无 AA/TA 接口分歧"，且其读取版本 SHA `a154e264…` 即本次被审定稿——声明与事实相符。抽验 6 项均一致：

| 抽验项 | A（02） | T（03） | R（01 冻结） | 结论 |
| --- | --- | --- | --- | --- |
| TC-06 夹具参数 | §6.2：M=1024/K=3/队列 1024，8 客户端×≥64，≥4 次轮转，默认随 workspace test | TC-06 同参数同口径，双轨取消 | AC-5 轮转瞬间不崩溃（01:116） | 一致 |
| 常量与注入 | §4.2.1/§6.3 TC-19：M=209715200、K=3、max≥32 注入、无生产解析 | TC-19①~⑤ 同值同口径 | AC-3/AC-13、Q1/Q2 冻结 | 一致 |
| AC-9 映射 | §1/§6.3：TC-13+TC-14 一并映射，例表全行 | §4 矩阵 TC-13(P0)+TC-14(P1)，TC-14 覆盖例表①~⑥ | 冻结 AC-9（01:120） | 一致 |
| 等级 env 前缀 | §4.1 表 VELOX_MESH/CONFIG/REGISTRY/PROVIDER/CONSUMER | §2 配置注入同一前缀清单 | FR-3/FR-4 命名规范 | 一致 |
| 脚本双轨消除 | §4.7 stdout=/dev/null+stderr 管道、第二 provider `VELOX_PROVIDER__LOG__DIR=…/demo-provider-2`、tail -F | TC-21/TC-22 同断言（含旧副本精确清理） | FR-6/Q7/E10 | 一致 |
| 行为变更披露 | §5（info→warn、fail-fast、截断、超限收敛、lossy/有界） | §2 风险 1~7 同口径 | NFR-2/AC-7/E13 | 一致 |

R 侧核对：AA §1 映射表引用的冻结文本（FR-4 链、AC-3 R/0 容差/R−1、FR-5 实名五份、AC-13 600 MiB）与 01 逐项一致；01 §7 Q5 行"P3 编辑备注（RQ-A-1）"按 GATE1 处置由 TC-23 承接，不依赖 Q5 措辞——A/T/R 三方口径统一。

## R2-3. 可实施性（DA 拿到设计无须再做架构决策）

依赖选型（自研 writer+appender =0.2.4、path dep、time 锁定步骤）、模块边界（velox-log 五文件布局与公共/私有接口草案）、故障路径（§4.3.2 逐行失败表）、等级语法（§4.4 白名单+例表）、脚本改法（§4.7 逐条含第二 provider 目录）、测试夹具（§6.1/6.2 定案、无双轨）、文档清单（§4.7 DA 文档列表）均已定案，并多处明文"DA 按此实施，不另选机制/不得在实现时另选上述语义"（§4.1/§7.3/§8 步骤）。本轮只读复核实施前提：根 Cargo.toml members 仍为五 crate（:3-9）、`[workspace.dependencies]` 集中依赖含 tracing-subscriber env-filter、**velox-log 尚不存在**（`ls` 证实，属拟新增共享库而非第 6 个运行时二进制，第 1 轮已认定方向成立）、rust-toolchain.toml channel=stable 注释 1.100.0（02:50 的描述逐字吻合）、ADR 目录仅 0001（0002 随实施落地，第 1 轮已认定不判缺陷）。无遗留"待 DA 临场选择"事项。

## R2-4. 可测性交叉检查

03 §4 矩阵 AC-1~14 逐条有 TC 且每条 AC 至少 1 条 P0/P1（缺口栏全为"无"）；实现路径可测性复核：AC-3/4/13 走 writer 注入+逻辑字节求和（§6.1.1）；AC-5 走 §6.2 活体子进程（真实 subscriber+队列+真实 OS 文件，非纯单测）；AC-6/7/9/10/11 走完整 init/局部 Dispatch/EnvInputs 注入（§6.1.2/6.1.3，不重复装全局 subscriber）；AC-8 用正常 error! 探针；AC-12 常量+注入构造；AC-14 两分支判定。TC-06 活体夹具（§6.2）与 TC-24/25 故障用例的测试接口论证成立：夹具经 build_dispatch→全局 subscriber→LogGuard 完整生产路径、FaultFs 可确定注入每类失败、ReportSink/时间源可注入使限频与静止断言无需等待 60s。队列专项与静止专项分设，不以"60 秒内告警不多"掩盖反馈。CI 门槛命令与 scripts/test.sh 现状一致（fmt+clippy+build+test），`--locked`/`cargo test -p velox-log` 为实施后补充，非本轮已执行项（文档如实声明）。

## R2-5. 范围红线与冻结项

范围仍为运行时五 bin 日志接入 + 共享库 + 两启动脚本 + mono 文档/ADR，未越界：无生产轮转配置（D3）、AC-13 未动、RQ-1/2/5 未重开、不改 compose/历史 feature 工件（§4.7 末）、第二 provider 以目录隔离不新增运行名副本。冻结项（FR-4/AC-9、AC-3、FR-5、AC-13、Q1~Q9 决议）在 A/T 双侧均未偏离。TA 03 明确"本轮只做静态设计，无任何通过结论"，与本关口性质一致。

## R2-6. 问题清单（本轮无阻断项）

| 编号 | 描述 | 证据 | 级别 | 责任方/处置 |
| --- | --- | --- | --- | --- |
| P3-A1 | §4.3.4/§4.6 中"worker 卡死/被 kill 时报告不可达"等披露为尽力语义，TC-24/25 的对应分支依赖时间预算（2s/观测窗），实跑时须如实记录而非以预算通过替代语义验证 | 02 §4.6 末段、03 TC-24/25 | P3 | TA 执行期留痕即可 |
| P3-A2 | §6.2 夹具以测试可执行文件自举子进程并使用 `--test-threads=1`，与并行测试套件的资源竞争（临时目录、端口）依赖 TempDir 唯一化与 127.0.0.1:0 绑定，属实施期常规工程注意点，设计已给唯一化方案 | 02 §6.2/§6.1.4 | P3 | DA/TA 实施时按既定方案 |
| P3-A3 | §4.8 第 1 步 `cargo update -p time --precise 0.3.36` 依赖新库入 lock 后执行，步骤文本已含此前置（"生成 Cargo.lock 后"），实施时须按序执行避免在无 time 图上跑该命令 | 02 §4.8 | P3 | DA 按序执行 |

以上均为实施期注意点，不改变设计定案，不构成 GATE2 驳回条件。**无 REWORK 修改清单。**

## R2-7. 历史脉络与取证记录

- 第 1 轮 GATE2（本文件 §1~11 存档）：REWORK，G2-1~8 + OP/RQ 仲裁；需求侧退回 SA 限域修订。
- GATE1 定向复审（01-requirements-review.md §R0~R6，2026-11-26 11:06）：PASS，四处修订落实且未越限，需求冻结。
- 本轮（2026-11-26）：GATE2 返工复审 **PASS**。AA 返工定稿+映射同步、TA v1 回填均按契约完成；下游硬约束（AA §8 实施步骤、§6.4 命令、ADR-0002 与 MSRV 实证归 DA 实施轮）不因本 PASS 解除。
- 本轮取证手段：SHA-256 重算锁定 5 份输入；完整阅读 01/02/03/两份评审文件；只读抽查根 Cargo.toml、Cargo.lock（无 tracing-appender/file-rotate/time，tracing-subscriber 0.3.23）、rust-toolchain.toml、velox-log 不存在性、scripts/run.sh:43、scripts/common.sh:12-23、velox-demo/scripts/acceptance.sh:80-88/:224、velox-registry/tests/ 存在性、五个 main.rs 日志初始化；静态重读本机 `tracing-subscriber-0.3.23`（directive.rs:215~225、builder.rs:168~172）核实 EnvFilter 裸词/空串语义。未运行 cargo build/test/clippy/fmt、acceptance、服务或轮转写入；未执行网络取证（第 1 轮已完成的上游源码读取结论按存档采信，本轮本地锁定版本语义复核一致）。
- git 状态：develop @ af28e65；工作树在途修改为 icl 规格与 velox-config 纯 rustfmt 格式化（与本 slug 无关，全部保留不评判）；本轮唯一写入为本文件追加章节。

PIPELINE addlog GATE2 PASS G2-1~8返工契约逐条闭合（无界副本/超大事件硬上界/等级校验/无反馈报告/IO故障状态机/测试接口与v1同步/依赖实证/需求冻结链），三方A-T-R抽验一致、DA无须临场决策、范围红线与冻结项未动，仅3项P3实施期备注，可交DA实施
