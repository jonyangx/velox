# 需求分析：子系统日志本地文件 rotate（addlog）

> 工序：F-SA · Feature：addlog · BRD：`velox-mono/docs/brd/001-addlog.md`（逐字引用见 §1.2）
> 状态：已交付（GATE3 验收通过，见 05-acceptance.md；测试终验轮 F-TA-F 通过）· 关联工序：F-GATE3（验收）

## 1. 背景与目标

### 1.1 现状（代码证据）

- **运行时子系统共 5 个可执行 crate**（根 `Cargo.toml` members）：`velox-mesh`、`velox-config`、`velox-registry`、`velox-demo/provider`（bin `demo-provider`）、`velox-demo/consumer`（bin `demo-consumer`）。`velox-mono` 为知识中枢，非运行时组件，无 crate、无日志（`velox-mono/docs/subsystems.md`）。
- **当前日志全部输出到 stdout**，由 `tracing_subscriber::fmt()` + `EnvFilter`（读 `RUST_LOG`，缺省 `"info"`）初始化，**进程内不写任何文件、无轮转**：
  - `velox-mesh/src/main.rs:38-42`
  - `velox-config/src/main.rs:30-35`
  - `velox-registry/src/main.rs:30-35`
  - `velox-demo/provider/src/main.rs:34-39`
  - `velox-demo/consumer/src/main.rs:35-40`
- **唯一的"文件日志"来自启动脚本的重定向**：`scripts/run.sh:43` 以 `nohup ... >>"$VELOX_LOG_DIR/$crate.log" 2>&1 &` 落盘，`VELOX_LOG_DIR` = 仓库根 `logs/`（`scripts/common.sh:23`）。该文件**无限增长、无轮转、无上限**，且仅覆盖经 `run.sh` 启动的场景。
- 仓库根 `.gitignore` 已忽略 `logs/`（无需新增）。
- 存量依赖：历史 feature e2e 直接断言 `logs/<crate>.log` 内容（如 `docs/specs/registry-persistence-watch/03-test-cases.md:38,105`）；`RUST_LOG` 用法见 `scripts/run.sh:17`、`scripts/README.md:66`。
- 技术栈约束：日志选型已定为 tracing / tracing-subscriber（`velox-mono/docs/architecture.md` §6）；workspace 仅引入 `tracing-subscriber`（env-filter feature，根 `Cargo.toml`），**当前依赖不具备按大小轮转能力**，需新增依赖——按 `velox-mono/docs/development.md` §1，新增/升级依赖属架构决策，须在 02-architecture 中论证并统一放入 `[workspace.dependencies]`。

### 1.2 原始需求（BRD 逐字）

1. 为 mono 纳管的子系统日志配置为本地文件 rotate，保留 3 个文件，每个 200m 上限。
2. 指定到项目 logs 目录下。
3. 输出日志的等级可以调整，默认 warn。

### 1.3 目标

- 各运行时子系统进程**自身**将日志写入本地文件，带大小触发轮转与保留上限，消除无限增长风险；
- 日志目录统一为项目 `logs` 目录，按子系统命名隔离；
- 日志等级可配置，缺省 `warn`，降低常规运行噪音；
- 全部改动通过既有 CI 门槛：`cargo fmt --all --check`、`cargo clippy --workspace --all-targets -- -D warnings`、`cargo build --workspace`、`cargo test --workspace`。

### 1.4 「mono 纳管的子系统」边界界定

依据 `velox-mono/docs/architecture.md`、`subsystems.md`、`development.md`：

- **纳入**：Cargo workspace 全部 5 个运行时二进制（mesh / config / registry / demo-provider / demo-consumer）。它们即 mono 文档体系纳管的全部运行时子系统与示例服务；行为保持一致，避免"部分子系统有文件日志、部分没有"的运维分裂。
- **不纳入**：`velox-mono` 本身（知识中枢，无运行时进程）。

> demo 两个二进制是否纳入存在解读空间，列入待确认问题 Q3（建议纳入）。

## 2. 功能需求

### FR-1 进程内文件日志落盘

**作为**运维/开发人员，**在**任意方式启动子系统（`scripts/run.sh`、裸进程、systemd、容器）**时**，期望进程自身将日志写入本地文件，不再依赖启动脚本重定向。

- 范围：全部 5 个运行时二进制（§1.4）。
- 日志文件位于项目 logs 目录（FR-3），文件名含子系统名（如 `velox-mesh.log`），避免多进程写同一文件。
- 输出格式沿用现有 tracing fmt 格式（时间戳/级别/目标/消息/结构化字段），本 feature 不改造日志内容格式。
- 实现载体（新依赖选型、同步/异步写入器）由 F-TA-C 决定；本需求只约束行为。

### FR-2 大小触发轮转与保留上限

**作为**运维人员，**在**日志持续写入时，期望单文件体积有硬上限，且磁盘上日志文件数量有上限。

- 单个日志文件达到上限（BRD：200m，单位待确认 Q2，建议 200 MiB）即触发轮转：当前文件归档（按序号后缀，命名方案见 Q9），打开新的活跃文件继续写入；
- 磁盘上该进程的日志文件总数不超过保留数（BRD：3 个；"含/不含活跃文件"待确认 Q1，建议含活跃文件共 3 个）；超出即删除最旧归档；
- 轮转与清理对调用方透明，写入不因轮转丢失或中断进程。

### FR-3 日志目录与命名

**作为**运维人员，**在**部署/启动子系统时，期望日志固定落到"项目 logs 目录"，可预测、可清理。

- 默认目录为 `logs`（相对进程工作目录解析；从仓库根经 `scripts/run.sh` 启动即落在仓库根 `logs/`，与现行脚本约定一致）；
- 目录支持配置覆盖（配置文件 `[log].dir` + 环境变量 `VELOX_<CRATE>__LOG__DIR`，遵循 development.md §5 配置规范），生产 daemon 可用绝对路径；
- 目录不存在时启动期自动创建（含缺失的父目录）；
- 5 个进程共用同一 `logs/` 目录时按文件名（子系统名）隔离，互不覆盖。

### FR-4 日志等级可调整，默认 warn

**作为**运维/开发人员，**在**排查或降噪时，期望不重新编译即可调整日志等级，重启进程生效。

- 默认等级 `warn`（未做任何配置时，仅 WARN/ERROR 输出）；
- 生效来源优先级固定为：非空 `RUST_LOG` > 已设置的 `VELOX_<CRATE>__LOG__LEVEL` > 文件显式 `[log].level` > 内置 `warn`，配置命名遵循 development.md §5；`RUST_LOG=""` 视作未设置，新 `LOG__LEVEL` 只要已设置就参与选择（含空串），文件 level 或新 `LOG__LEVEL` 空串均非法；仅空白的非空 `RUST_LOG` 非法；
- **先选择最高优先来源，只校验该生效等级字符串**；合法高优先级可覆盖较低非法等级字符串。TOML 本身语法错误或 level 类型错误仍须报错，不属于可覆盖的等级字符串校验；
- 文件 level / 新 `LOG__LEVEL` 允许小写裸等级 `trace / debug / info / warn / error / off`，或逗号分隔的裸等级与显式 `target=level` 指令。target 非空，字符仅 ASCII 字母、数字、下划线、连字符、冒号，且至少含一个字母或下划线；项两侧允许 ASCII 空白，空项/尾逗号拒绝。不接受裸 target、数字等级、未知等级或 span/field 语法；例如 `warn,velox_mesh::proxy=debug` 合法，`warnn`、`svc=warnn`、`warn,,svc=info` 非法；通过上述校验后仍须严格解析；
- `RUST_LOG` 保留 tracing `EnvFilter` 的完整兼容语法（含 target、模块、span/field 指令），严格解析；裸 `RUST_LOG=warnn` 可作为合法 target，不能当作非法简单级别反例；`RUST_LOG=svc=warnn` 则非法。两套语法不可混同；
- 生效值解析错误或选中环境值非 UTF-8 时 fail-fast，不回退；错误含实际值与准确来源（文件路径及 `log.level` 键，或具体变量名）。生效值与来源绑定记录，较低来源不冒充生效来源（关联 AC-9/AC-10）。

### FR-5 各子系统一致接入，配置有默认值

- 5 个二进制行为一致（同一初始化逻辑；实现可为共享工具模块或宏，由 F-TA-C 决定）；
- 新增配置项全部带内置默认值、配置结构容忍未知字段（`#[serde(default)]`，development.md §5），老配置文件零改动可继续使用；
- 各子系统 config 目录下的开发默认 toml 补充 `[log]` 节并注明覆盖方式；core 三份为 `velox-mesh.toml`、`velox-config.toml`、`velox-registry.toml`，demo 两份为 `demo-provider.toml`、`demo-consumer.toml`。

### FR-6 文档与脚本同步

- `velox-mono/docs/development.md` §3.4（日志规范补充文件输出/轮转/默认级别约定）；
- `velox-mono/docs/subsystems.md` 各子系统「关键配置」节补 `[log]` 示例；
- `scripts/run.sh` / `scripts/README.md` 中与日志相关的说明与行为更新（进程内已写文件后，脚本重定向去留见 Q7）；
- 各 crate README 配置示例同步。

## 3. 非功能需求

| 编号 | 需求 | 说明 |
| ------ | ------ | ------ |
| NFR-1 | 磁盘占用上界 | 单进程日志磁盘占用 ≤ 保留数 × 单文件上限（按 Q1/Q2 建议值即 3 × 200 MiB = 600 MiB）；全 workspace 5 进程最坏 3 GiB，可控且可预测 |
| NFR-2 | 异步安全与不丢关键日志 | 文件写入不得阻塞 tokio worker 线程（非阻塞写入器）；进程优雅关停时已产生的日志须尽力 flush 落盘，WARN/ERROR 不因关停丢失 |
| NFR-3 | 兼容性 | ① 老配置文件（无 `[log]` 节）可直接运行；② 现有 `RUST_LOG` 行为与依赖它的脚本/e2e 不被静默破坏（关联 Q5）；③ `logs/<crate>.log` 路径约定对存量测试保持可用（关联 Q7） |
| NFR-4 | 可观测性 | 轮转发生、最旧文件被删除、日志目录创建失败等运维事件本身有日志（或明确错误）可查 |
| NFR-5 | 性能 | 文件日志引入的额外开销不得明显影响 mesh 数据面转发吞吐/延迟（定性验收：压测或 e2e 无回归） |
| NFR-6 | 依赖约束 | 如需新增 crate：统一声明于根 `Cargo.toml` `[workspace.dependencies]`，成员用 `workspace = true` 引用；PR 附依赖理由与替代方案（development.md §1）；禁用 native-tls 类非纯 Rust 传递依赖 |
| NFR-7 | 运行环境 | 目标环境为 Linux 宿主机 daemon 与容器；开发环境 macOS。不承诺 Windows |

## 4. 验收标准（与 FR 对应，Given/When/Then）

| 编号 | 对应 FR | 验收标准 |
| ------ | --------- | ---------- |
| AC-1 | FR-1 | Given 任一子系统二进制以默认配置在空目录启动，When 服务运行并产生任意日志事件，Then 工作目录 `logs/<子系统名>.log` 存在且包含该事件内容（时间戳+级别+消息）；不再依赖 `scripts/run.sh` 重定向（以裸进程启动亦可复现） |
| AC-2 | FR-1 | Given 5 个二进制同时运行于仓库根，When 各自输出日志，Then `logs/` 下存在 5 个独立文件（`velox-mesh.log`、`velox-config.log`、`velox-registry.log`、`demo-provider.log`、`demo-consumer.log`），且各文件内容与对应进程输出一致，无串写 |
| AC-3 | FR-2 | Given 单文件上限 M（生产固定 200 MiB），每条格式化日志块为一个事件聚合后提交的记录，处理后长度不超过 R=min(M,64 KiB)，When 当前文件剩余空间不足以容纳下一块，Then 写前先归档，再整块写入新活跃文件；相等时可恰好填满，下一非空块触发轮转。初始化成功后任何受管文件逻辑长度始终 ≤ M，超限容差为 0 字节；按整块提前轮转的未用空间小于该块长度（至多 R−1 字节）。超过 R 的单事件仅保留 UTF-8 完整前缀与 R 上限内的截断标记，不另存尾部。已有超限活跃文件或归档须在启动初始化成功前收敛到 ≤ M 并告警，无法收敛则启动失败；正常未超限旧文件仍按 E5 续写/保留。AC-13 总字节上界不变；轮转参数不作为生产配置开放 |
| AC-4 | FR-2 | Given 轮转多次发生（>保留数），When 检查 logs 目录，Then 该进程日志文件总数恒 ≤ 3（按 Q1 建议值），最旧文件已被删除，其余为最近内容 |
| AC-5 | FR-2 | Given 轮转过程中进程持续高并发写日志，When 轮转瞬间，Then 进程不崩溃、后续日志正常写入（e2e 断言：轮转前后均能 grep 到新日志行） |
| AC-6 | FR-3 | Given `logs` 目录不存在（含多级父目录缺失），When 进程启动，Then 目录被自动创建且日志正常落盘 |
| AC-7 | FR-3 | Given 配置 `[log].dir = "/tmp/velox-logs-x"`（或 `VELOX_<CRATE>__LOG__DIR`），When 启动，Then 日志写入该目录而非默认 `logs` |
| AC-8 | FR-4 | Given 未配置任何日志级别，When 启动并触发 INFO/WARN/ERROR 三个级别的日志，Then 文件中仅出现 WARN 与 ERROR 行 |
| AC-9 | FR-4 | Given 分别设置文件 `[log].level=info`、`VELOX_<CRATE>__LOG__LEVEL=info` 或非空 `RUST_LOG=info`，When 启动并输出不同等级事件，Then INFO 及以上可见；三渠道同时设置不同等级时，严格按 `RUST_LOG > LOG__LEVEL > 文件 > warn` 生效，低优先级值不生效。Given 合法高优先级值与非法低优先级等级字符串并存（如文件 warnn + LOG__LEVEL=info，或文件 warnn + 空 LOG__LEVEL + RUST_LOG=debug），Then 启动成功，行为及记录来源均为高优先级。Given 生效候选为非法新 LOG__LEVEL（含空串）或文件等级，Then 按 AC-10 fail-fast；`RUST_LOG=""` 回落下一来源，所有来源缺省才用 warn。新渠道 warnn 非法；仅 RUST_LOG=warnn 作为合法 target 接受，生效 RUST_LOG=svc=warnn 则 fail-fast，不回退，错误含实际值与准确来源 |
| AC-10 | FR-4 | Given 非法级别值（如 `"warnn"`），When 启动，Then 启动失败并输出含非法值与配置来源的错误信息（fail-fast，见 §5-E7） |
| AC-11 | FR-5 | Given 仅提供旧版配置文件（无 `[log]` 节）与未知字段，When 启动，Then 正常运行且日志行为等于内置默认值（文件+轮转+warn） |
| AC-12 | FR-5/FR-6 | Given 仓库代码，When 运行 `cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings && cargo build --workspace && cargo test --workspace`，Then 全部通过；且新增配置默认值有单元测试佐证（各 config.rs tests） |
| AC-13 | FR-2/NFR-1 | Given 长时间运行（或用小上限加速模拟）后检查，Then 单进程日志总磁盘占用 ≤ 保留数 × 单文件上限 |
| AC-14 | FR-6 | Given 文档变更，Then development.md §3.4、subsystems.md 关键配置、scripts/README.md、各 README 与实际行为一致（抽查配置项名/默认值与代码一致） |

## 5. 边界与异常场景

| 编号 | 场景 | 期望行为（建议默认，最终以评审为准） |
| ------ | ------ | ------ |
| E1 | logs 目录创建失败（权限不足、路径被文件占用） | fail-fast：启动报错退出，错误含目录路径与底层原因；不静默降级丢日志（关联 Q8） |
| E2 | 日志文件打开失败 / 运行中写失败（磁盘满、句柄被删） | 运行中写失败：记录一次 ERROR（去重/限频，避免日志风暴），进程不退出；后续写恢复后自动继续 |
| E3 | 磁盘满 | 同 E2；不因写日志失败拖垮业务请求路径（mesh 数据面尤甚） |
| E4 | 同一 crate 多实例写同一 `logs/<crate>.log` | v0.x 不做文件锁/实例隔离；文档声明"同机同 crate 多实例需各自配置不同 `log.dir`"；超出范围为见 §6 |
| E5 | 进程重启后已有历史日志文件 | 续写活跃文件，不覆盖、不清空历史；已归档文件保留并计入保留数 |
| E6 | 配置文件缺失 `[log]` 节 / 含未知字段 | 全部走内置默认值，容忍未知字段（development.md §5） |
| E7 | 级别值非法（`[log].level = "warnn"` 或空串） | 启动期报错退出（fail-fast），错误信息含非法值与来源（文件/环境变量）；不用静默回退默认值 |
| E8 | `RUST_LOG` 与 `[log].level`/`VELOX_*__LOG__LEVEL` 同时存在 | 按 Q5 结论执行；建议 `RUST_LOG` 优先（临时排障直改，无需改配置） |
| E9 | 容器场景（velox-demo docker-compose）：容器内 `logs/` 为临时层 | 进程行为不变（写容器内 `logs/`）；持久化需部署侧挂载卷，属部署方案，不在本 feature 范围（§6） |
| E10 | `scripts/run.sh` 重定向与进程内文件写同一 `logs/<crate>.log` | 禁止双写同一文件（轮转后脚本句柄仍指向旧文件会产生串写）；处理方案见 Q7 |
| E11 | 轮转与并发写竞争 | 轮转在写入器内部串行完成，不要求跨进程协调；单进程内不丢已提交的日志行（尽力，见 NFR-2） |
| E12 | 大小写/单位书写：`"200m"`、`200M`、配置值书写差异 | 单位与数值解析规则在 02-architecture 固化（关联 Q2）；配置项直接用数值（MiB），不接受字符串歧义写法 |
| E13 | `off` 级别 | 合法取值：关闭全部日志输出（文件与 stdout 均无）；启动横幅信息因此静默属预期行为，文档注明 |

## 6. 范围外（Out of Scope）

- 集中式/远程日志采集与传输（ELK、Loki、push 网关等）；
- 运行期动态调级（不重启进程热更新级别，如 admin API / 配置 watch）——本 feature 调级须重启生效；
- 按时间（天/小时）触发轮转、压缩归档（.gz）、归档上传；
- 日志内容格式改造（JSON 结构化切换、字段规范调整）——沿用现有 tracing fmt 输出；
- 日志脱敏、审计日志（config 子系统 v0.3+ 规划项）；
- metrics/Prometheus（architecture.md v0.3 规划）；
- 容器部署的日志卷挂载与采集方案（docker-compose 变更仅限 E9 必要的最小适配，如无必要不动）；
- `velox-mono` 本身（无运行时）；
- 历史 feature 工件（`docs/specs/instance-concurrency-limit/`、`docs/specs/registry-persistence-watch/`）的修订（如其 e2e 断言受 Q7 影响，仅做最小兼容性确认）；
- 跨机日志聚合、时钟同步、时区策略统一（沿用本地时间戳现状）。

## 7. 待确认问题（附建议默认方案）

| 编号 | 问题 | 建议默认方案 |
| ------ | ------ | ------ |
| Q1 | "保留 3 个文件"是否含当前活跃文件？（含活跃=3 个总计 600 MiB；不含=1 活跃+3 归档共 800 MiB） | **含活跃文件共 3 个**（字面最直接、磁盘上界确定、多数轮转实现的 Keep(n) 语义一致、AC 可判定） |
| Q2 | "200m" 单位：MiB（2^20）还是 MB（10^6）？ | **200 MiB**（与常见实现语义一致，验收阈值无歧义） |
| Q3 | demo-provider / demo-consumer 是否属"mono 纳管的子系统"？ | **纳入**（workspace 全体运行时二进制行为一致；容器内写容器层 `logs/`，见 E9） |
| Q4 | "项目 logs 目录"解析基准：仓库根固定路径，还是进程 cwd 相对路径 + 可配置？ | **默认相对 cwd 的 `logs`，新增 `[log].dir` 可覆盖（含绝对路径）**；从仓库根经 run.sh 启动即等价于现行 `logs/`，生产 daemon 用绝对路径 |
| Q5 | 既有 `RUST_LOG` 机制是否保留？与新配置的优先级？ | **保留且优先级最高**（RUST_LOG > `VELOX_<CRATE>__LOG__LEVEL` > `[log].level` > 默认 warn）；理由：scripts/README.md:66、run.sh:17 与历史 e2e 已依赖，去掉即破坏存量 |
| Q6 | 文件日志落地后 stdout 是否保留输出？ | **保留双写**（文件 + stdout）：容器/systemd 场景 journal 仍可用，开发体验不变；如评审嫌冗余可加 `[log].stdout` 开关（默认 true） |
| Q7 | `scripts/run.sh` 的 stdout 重定向（run.sh:43）与进程内文件写同一文件的冲突如何处理？脚本改动是否入本 feature 范围？ | **入范围**：run.sh 去掉对 `logs/<crate>.log` 的重定向（改 `/dev/null` 或依赖 stdout 双写由进程落盘），`run.sh logs` 继续指向同一进程内文件名；否则 E10 串写必然发生 |
| Q8 | 日志目录/文件初始化失败：fail-fast 还是降级 stdout + WARN？ | **fail-fast**（E1）：日志是可观测性底线，静默丢文件日志违背 NFR-4；降级模式留待后续需求 |
| Q9 | 归档文件命名：固定序号（`.log.1/.log.2`）还是时间戳后缀？ | **固定序号**：删除"最旧"语义确定、`run.sh logs`/e2e grep 断言路径稳定、AC 易判定 |

## 8. 影响面（涉及的模块/文件）

#### 代码（全部遵循 development.md §1/§5，禁止改动本 feature 之外的代码语义）

- `Cargo.toml`（根）：`[workspace.dependencies]` 可能新增轮转实现依赖（NFR-6）；
- `velox-mesh/src/main.rs`（:38-42 日志初始化）、`velox-mesh/src/config.rs`、`velox-mesh/config/velox-mesh.toml`；
- `velox-config/src/main.rs`（:30-35）、`velox-config/src/config.rs`、`velox-config/config/velox-config.toml`；
- `velox-registry/src/main.rs`（:30-35）、`velox-registry/src/config.rs`、`velox-registry/config/velox-registry.toml`；
- `velox-demo/provider/src/main.rs`（:34-39）、`src/config.rs`、`config/demo-provider.toml`；
- `velox-demo/consumer/src/main.rs`（:35-40）、`src/config.rs`、`config/demo-consumer.toml`；
- 可能新增：workspace 级日志初始化共享模块（归属 crate 由 F-TA-C 定）；
- `scripts/run.sh`、`scripts/README.md`（Q7）。

#### 文档

- `velox-mono/docs/development.md` §3.4（日志规范）、§5（配置规范如需示例）；
- `velox-mono/docs/subsystems.md`（mesh/config/registry「关键配置」节）；
- `velox-mesh/README.md`、`velox-config/README.md`、`velox-registry/README.md`、`velox-demo/README.md`；
- `.gitignore`：已含 `logs/`，无需改动（证据：仓库根 `.gitignore`）。

#### 兼容性关注点

- `logs/<crate>.log` 路径与 `RUST_LOG` 的存量消费方：`scripts/run.sh`、`scripts/README.md`、`docs/specs/registry-persistence-watch/03-test-cases.md`（仅确认不破坏，不重写）。
