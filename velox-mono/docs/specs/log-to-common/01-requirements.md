# 需求分析：log-to-common（velox-log 重构为 velox-common，log 作为其中一个组件）

> 工序：F-SA，v1 首轮（无返工反馈）。
> 调研基线：develop 分支 @ `cf75e33`；工作树含 `velox-config/src/limit.rs` 既有 rustfmt 改动（非本 feature 范围，原样保留、不得混入本 feature 提交）。
> 说明：`docs/specs/log-to-common/03-test-cases.md` 为并行工序 F-TA-C 的产物，与本文并行消费同一原始诉求；本文「待确认问题」编号（OP-1~OP-6）与该文已使用的 OP-1~OP-4 保持兼容并扩展，供后续 F-TA-D 合并仲裁。

## 1. 背景与目标

### 1.1 背景

- 共享日志 crate `velox-log` 由前一 feature addlog 建立（提交 `a5b75e5`「feat(log): 子系统日志本地 rotate（velox-log 共享库 + 五包接入）」），为五个运行时二进制统一提供：等级链解析（`RUST_LOG` > `VELOX_*__LOG__LEVEL` > 文件 `[log].level` > warn）、本地文件 rotate（固定三文件、单文件 200 MiB 硬上界、超长事件 64 KiB 截断）、异步 lossy 队列落盘（1024 条）、无反馈 stderr 运维报告。crate 本体现状：`velox-log/src/` 六文件共 3346 行（`lib.rs` 295 / `rotate.rs` 672 / `filter.rs` 242 / `report.rs` 370 / `bounded.rs` 113 / `tests.rs` 1654，28 例 `#[test]` 全部集中在 `tests.rs`，已 grep 核实）。
- 需求原文：「重构 velox-log，修改为 velox-common，log 只是其中一个组件。」（用户原文写作 velex-log/velex-common，系 velox 拼写笔误，统一按 velox 处理。）
- workspace 现状：根 `Cargo.toml:9` members 含 `"velox-log"`；`Cargo.toml:26` `[workspace.dependencies]` 有 `velox-log = { path = "velox-log" }`；五包（velox-config、velox-registry、velox-mesh、velox-demo/provider、velox-demo/consumer）各自 `Cargo.toml` 声明 `velox-log.workspace = true`（均为 `Cargo.toml:11`，已逐一核实）。

### 1.2 目标

- **G1**：新建 workspace 通用基础库 crate `velox-common`（目录 `velox-common/`），日志能力作为其中一个组件（模块）完整迁入。
- **G2**：`velox-log` 不再作为独立 crate 存在——目录移除、workspace member 与 path 依赖移除（旧名残留按分层口径清理，见 FR-5/OP-2/OP-3）。
- **G3**：五包接入点改为 velox-common（或其 log 组件再导出），日志现有能力与行为零回退（rotate/filter/report/bounded 及测试语义全保留）。
- **G4**：velox-common 的目录与导出结构为后续公共组件预留扩展位（本期不迁移、不新增 log 以外的组件）。
- **G5**：velox-mono 活文档中涉及 velox-log 的引用同步更新。

## 2. 功能需求

### FR-1 workspace 结构重构本体

- **作为**仓库维护者，**在**workspace 结构治理场景，**期望**：`velox-log/` 目录与独立 crate 身份完全退场；新目录 `velox-common/` 入驻 workspace 成为共享基础库 crate；日志能力作为其一个组件（模块）迁入，不是第六个运行时子系统，亦不是第二个日志 crate。
- 结构要求：`velox-common` 以 workspace path member 身份存在，依赖统一走 `[workspace.dependencies]`（沿用现有纪律：根 `Cargo.toml:19` 注释「所有 crate 必须从这里引用」）。

### FR-2 log 组件能力无损迁移

- **作为**五个运行时二进制的维护者，**在**crate 结构重构场景，**期望**：日志全部能力原样迁入 `velox-common` 的 log 组件，公共 API 对外可见形态等价：
  - 对外 API 面（现状核实，消费方实际使用的全集）：`LogConfig`、`ResolvedLevel`（含 `filter()`/`source_display()`/`LevelSource`）、`resolve_level`、`absolutize`、`init`、`LogGuard`，常量 `MAX_FILE_BYTES`/`KEEP_FILES`/`MAX_EVENT_BYTES`/`QUEUE_EVENTS`；
  - 内部模块 `bounded`/`filter`/`report`/`rotate` 现状为 `pub mod` + `pub(crate)` 条目（外部本就不可见），迁移后保持同等可见性，不因重构意外扩大或收窄；
  - crate 内 28 例单测（`velox-log/src/tests.rs`）全部随迁，数量与语义一一对应，测试注入式设计（`EnvInputs` map 注入、`FaultFs`、`TestReportSink`、`RotationLimits::injected`）不破坏；
  - 文档注释（`//!`）随迁移更新为新结构口径（crate 名、BRD/ADR 指向），不保留指向已消失 crate 的自述。

### FR-3 五包接入点切换（含测试资产）

- **作为**五包（velox-config、velox-registry、velox-mesh、velox-demo/provider、velox-demo/consumer）的维护者，**在**依赖重组场景，**期望**：
  - 各包 `Cargo.toml` 依赖声明由 `velox-log.workspace = true` 改为指向 `velox-common`（继续走 workspace 统一引用，不出现散落 path/版本）；
  - 各包源码 `use velox_log::…` / `velox_log::…` 全部切换为新路径（接入形态 OP-1 仲裁：`velox_common::…` 顶层再导出，或 `velox_common::log::…` 组件路径）；
  - **测试资产必须同改**：`velox-registry/tests/common/mod.rs:39-40` 直接构造 `velox_log::LogConfig::default()` 与 `velox_log::ResolvedLevel::default()`（registry 是唯一在测试代码中直接引用 velox_log 的包）——只改 src 不改 tests 会导致编译失败或漏改。

### FR-4 workspace 依赖图与 Cargo.lock 一致性

- **作为**构建系统，**在**crate 改名场景，**期望**：
  - 根 `Cargo.toml`：members 列表 `"velox-log"` → `"velox-common"`（行 9）；`[workspace.dependencies]` `velox-log = { path = "velox-log" }` → `velox-common = { path = "velox-common" }`（行 26）；
  - `Cargo.lock` 同一变更内重新生成并提交：现存 velox-log 条目（`Cargo.lock` 中 6 处出现：包定义 1 处 + 五包依赖列表各 1 处，行 181/200/1537/1541/1567/1585）全部更新为 velox-common 对应条目；
  - `cargo build --workspace --locked` 通过（lock 与 manifests 失同步时 `--locked` 会失败，故 lock 必须随同提交）。

### FR-5 旧名残留清理（分层口径）

- **作为**代码评审者，**在**改名收尾场景，**期望**旧名 `velox-log`/`velox_log` 按分层口径清零：
  - **层① 源码与构建清单（零残留）**：`velox-common/`、五包 `src`+`tests`、根与各包 `Cargo.toml`；
  - **层② 活文档（零残留）**：`velox-mono/docs/adr/`、`architecture.md`、`development.md`、`subsystems.md`、根 `README.md` 中 velox-log 字面引用全部按新口径更新（ADR 处置方式见 OP-4）；
  - **层③ 历史规格工件（默认豁免）**：`velox-mono/docs/specs/addlog/01~05`（约 62 处）、`velox-mono/docs/specs/instance-concurrency-limit/04-test-report.md:81`（「velox-log：28/28」）、`05-acceptance.md:86`——历史交付记录，处置见 OP-3（建议默认原样保留，不计残留）；
  - **层④ 行为字符串（默认豁免）**：`report.rs:291` stderr 报告行前缀 `component=velox-log` 与 `lib.rs:187` worker 线程名 `"velox-log"`——处置见 OP-5（建议默认保留原值，不计残留）。

### FR-6 velox-mono 活文档同步

- **作为**文档读者，**在**查阅架构/子系统文档场景，**期望**活文档描述与真实 crate 结构一致：
  - `velox-mono/docs/adr/0002-log-rotation.md`：现存 6 处 velox-log 字面（标题、D1 决策行 18、依赖配置示例行 41、`cargo tree -p velox-log` 证据行 46/57、边界说明行 58），按 OP-4 处置（建议默认：新增 ADR 记录本次结构重构决策 + 0002 顶部加 superseded/更名注记，0002 正文不改写）；
  - `velox-mono/docs/subsystems.md:61,111,171`（三处 `[log]` 配置段注释「子系统日志（addlog）」）与 `velox-mono/docs/development.md:67`（「文件输出与轮转（addlog，ADR-0002）」）：经核实**不含 velox-log 字面**，仅使用 addlog 历史术语——是否顺带统一见 OP-6（建议默认不动，仅核对描述行为仍准确）；
  - `architecture.md`、根 `README.md`：已 grep 核实无 velox-log/velox_log/addlog 字面引用，本项无改动义务；
  - `velox-mono/docs/specs/_pipeline-state.md`：含 velox-log 引用（2 处）但属流水线在途状态文件，按范围约束**不触碰**。

### FR-7 velox-common 扩展位预留

- **作为**后续公共组件的贡献者，**在**向 velox-common 新增组件场景，**期望**：log 组件的目录与导出结构不排除平级新增组件——log 以独立组件模块存在，顶层 `lib.rs` 具备按组件再导出的组织位（顶层再导出或组件路径，随 OP-1 定），新增组件时不需要改动 log 组件内部代码或既有五包接入；
- 预留是结构性的而非占位性的：不创建空壳占位组件/空目录仪式性文件，不做 log 以外的任何实现。

## 3. 非功能需求

### NFR-1 日志行为零回退

重构前后日志可观测行为等价，以下不变量全部保持（均由随迁的 28 例单测与 acceptance.sh 实证）：

| 不变量 | 现状锚点 |
|---|---|
| 等级链 `RUST_LOG(非空) > VELOX_*__LOG__LEVEL(已设置) > 文件 [log].level(显式) > warn`；先选来源、只校验生效字符串 | `velox-log/src/filter.rs`（模块纪律注释）、`lib.rs:149 resolve_level` |
| rotate 硬不变量：仅 `stem.log/.log.1/.log.2`，单文件 ≤ M=200 MiB，0 超限容差，启动规范化收敛 | `velox-log/src/rotate.rs` 头注释、`lib.rs:39 MAX_FILE_BYTES` |
| 超长单事件截断 R=min(M,64 KiB)，UTF-8 完整前缀 + ` [truncated]\n` | `velox-log/src/bounded.rs` 头注释、`lib.rs:43` |
| lossy 队列 1024 条、丢弃计数外显、独立 worker 线程 | `lib.rs:45 QUEUE_EVENTS`、`lib.rs:182-189` |
| 无反馈报告：不回流 tracing/队列，stderr 行 `component=velox-log level=… stage=…` ≤1024 字节，60s 限频 | `velox-log/src/report.rs:7,291` |
| LogGuard drop：末次队列采样 → closing 标记 → 有界等待排空 | `lib.rs:245-261` |
| 生产无轮转参数配置（`RotationLimits::injected` 仅库内部测试） | `lib.rs:14` |

### NFR-2 构建/静态/验收门槛全绿

四门槛 + 验收脚本零修改通过：`cargo build --workspace`；`cargo test --workspace`；`cargo fmt --all --check`；`cargo clippy --workspace --all-targets -- -D warnings`；`bash velox-demo/scripts/acceptance.sh`。acceptance.sh 经核实**不含 velox-log/velox_log 字面引用**（其中 `VELOX_LOG_DIR` 为脚本自设环境变量名，与 crate 无关），应零修改运行并 PASS。

### NFR-3 工作树纪律

- `velox-config/src/limit.rs` 既有 rustfmt 改动原样保留，不混入本 feature 提交（提交内容须可按文件剥离）；
- `velox-mono/docs/specs/_pipeline-state.md`、运行数据（`logs/`、`data/`）、`velox-mono/docs/brd/`（未跟踪）不入库不触碰。

### NFR-4 依赖与元数据最小扰动

- 不新增任何外部依赖（迁移是纯结构重构）；`tracing`/`tracing-subscriber`/`tracing-appender`/`serde`/`anyhow` 依赖面随迁不变；
- `rust-toolchain.toml`（stable）、workspace 统一 version/edition/rust-version 元数据不变；velox-common 沿用 workspace 包元数据，`description` 更新为通用基础库口径。

### NFR-5 测试资产完整性

原 `velox-log` 28 例单测迁移后逐例对应（数量不减、断言语义不变）；workspace 全量测试除既有已知 flaky（`demo_consumer::config::tests::defaults_match_contract`，见 instance-concurrency-limit/04-test-report.md:26 记录）外全绿。

## 4. 验收标准

### AC-1（对应 FR-1）velox-log 退场、velox-common 入驻

- Given 重构完成后的工作树；
- When ① 查看根 `Cargo.toml` members；② `ls velox-log/`；③ 查看依赖图（`cargo metadata` 或构建输出）；
- Then ① members 不含 `"velox-log"`、含 `"velox-common"`；② `velox-log/` 目录不复存在；③ 依赖图中无 velox-log 节点、velox-common 为 path 成员。

### AC-2（对应 FR-2）log 组件 API 与测试无损迁移

- Given `velox-common` 中的 log 组件；
- When ① 对照本文件 §FR-2 API 清单检查对外可见项；② 运行原 28 例单测；
- Then ① API 面（6 项 API + 4 常量）对外可用性与迁移前等价（再导出透传或等价路径均可，OP-1 口径），内部模块可见性未扩大/收窄；② 28/28 通过，注入式测试设计完好，断言无一例删除或改弱。

### AC-3（对应 FR-3）五包接入点切换完整（含测试资产）

- Given 五包工作区；
- When ① `grep -rn "velox_log" --include="*.rs" velox-config/src velox-registry/src velox-registry/tests velox-mesh/src velox-demo`；② 逐包检查 Cargo.toml 依赖声明；③ `cargo build --workspace`；
- Then ① 源码与测试代码零 `velox_log` 残留（含 `velox-registry/tests/common/mod.rs:39-40`）；② 五包依赖声明统一指向 velox-common 且走 workspace 引用；③ 编译零错误零警告。

### AC-4（对应 FR-4）依赖图与 lock 一致

- Given 根 `Cargo.toml` 与 `Cargo.lock`；
- When ① `grep -n "velox-log" Cargo.lock`；② `cargo build --workspace --locked`；
- Then ① lock 中 velox-log 条目（原 6 处出现点）清零、velox-common 条目就位且五包依赖列表同步更新；② `--locked` 构建通过（lock 未失同步的直接证明）。

### AC-5（对应 FR-5）旧名残留分层清零

- Given 分层残留口径（§FR-5）；
- When `grep -rn "velox-log\|velox_log"` 分层扫描 ①源码+Cargo.toml ②活文档（adr/architecture/development/subsystems/README）③历史规格 ④行为字符串；
- Then ①② 零残留；③ 按 OP-3 默认原样保留、不计残留（若上游改判则按改判口径全改）；④ 按 OP-5 默认豁免（若改判为改名，则 `report.rs:291`、`lib.rs:187` 及断言/文档锚点一并同步且行为测试随之更新）。

### AC-6（对应 FR-6）活文档同步

- Given 活文档集合；
- When 检查 `docs/adr/0002-log-rotation.md`、`docs/subsystems.md`、`docs/development.md`、`docs/architecture.md`、根 `README.md`；
- Then ADR 0002 按 OP-4 默认方案处置（新增 ADR + 0002 注记后，活文档不再以 velox-log 为**现行** crate 名描述日志机制）；architecture/README 维持零旧名；subsystems.md 三处注释与 development.md:67 的 addlog 术语按 OP-6 默认保留且其描述的行为经核对仍然准确。

### AC-7（对应 FR-7）扩展位结构性预留

- Given `velox-common/src` 布局与 `lib.rs` 导出组织；
- When 审视「新增一个平级组件（假想）」所需改动面；
- Then log 为独立组件模块，新增组件只需新增模块与顶层导出项，无需改动 log 组件内部或五包既有接入；仓库中不存在空壳占位组件。

### AC-8（对应 NFR-1/2/3/5）整体门槛与不回退实证

- Given 完整重构提交；
- When 依次运行 §NFR-2 五条基线命令；
- Then 五条全部通过（acceptance.sh 零修改 PASS）；测试统计中 log 组件 28/28；提交 diff 不含 `velox-config/src/limit.rs`、`_pipeline-state.md`、`logs/`、`data/`、`velox-mono/docs/brd/`。

## 5. 边界与异常场景

1. **Cargo.lock 失同步**：crate 改名后旧 lock 必然失配，`--locked` 构建与 CI 失败。处置：Cargo.lock 在本 feature 同一提交内重生成并提交（FR-4），禁止「lock 后补」。
2. **只改 src 漏 tests**：registry 是唯一测试代码直接引用 velox_log 的包（`tests/common/mod.rs:39-40`）。漏改则 `cargo test --workspace` 编译失败。处置：AC-3 检查面显式包含 `velox-registry/tests`。
3. **行为字符串误伤**：`component=velox-log`（`report.rs:291`）是文档化 stderr 行格式契约；worker 线程名 `"velox-log"`（`lib.rs:187`）可在线程列表观测。二者是否随 crate 改名属行为判定问题，不在 FR-5 层①机械清零范围内（OP-5 仲裁）。
4. **不受影响点（防误改）**：五包调用 `velox_log::init(env!("CARGO_PKG_NAME"), …)` 中的 `CARGO_PKG_NAME` 取值来自**消费方**包名——日志文件 stem（`logs/velox-config.log` 等）与 `VELOX_*__LOG__DIR`/`__LOG__LEVEL` 环境变量契约均不受 crate 改名影响，不得顺带改动。
5. **并行单测环境纪律**：`filter.rs` 经 `EnvInputs` 注入环境以避免并行测试改进程环境；迁移时目录搬移/模块路径调整不得迫使测试退回直接读真实环境。
6. **doc 注释与文档锚点漂移**：`lib.rs:1` 模块头自述「velox-log：velox 共享日志库（BRD 001-addlog / 架构 02 定稿）」等 crate 自指文本需随迁移更新；引用外部文档行号时遵循既有锚点纪律（参照 instance-concurrency-limit/04-test-report.md:56 的行号留痕先例，注明核对时点）。
7. **目录删除后的构建缓存**：`velox-log/` 删除后 target 中旧工件残留不影响正确性（cargo 按 lock/manifest 求解），验收以干净命令结果为准，不依赖增量缓存结论。
8. **crates.io 同名**：`velox-common` 在 crates.io 上是否已被占用不影响本期（纯 workspace path 依赖，不发布）；若未来发布需另行查名（范围外）。

## 6. 范围外

1. 不新增、不迁移 log 以外的任何公共组件（扩展位只是结构预留，AC-7 禁止空壳占位）。
2. 不改动日志能力本身：不调 rotate 参数、不改等级链语义、不改队列容量/限频、不改 stderr 报告格式（OP-5 默认保留时）。
3. 不回溯修订历史规格工件（`specs/addlog/01~05`、`specs/instance-concurrency-limit/04/05`），除非 OP-3 上游改判。
4. 不引入 velox-log 兼容壳 crate / deprecation shim（OP-2 默认完全移除；意图原文明确「velox-log 不再作为独立 crate 存在」）。
5. 不触碰 `_pipeline-state.md`、`logs/`、`data/`、`velox-mono/docs/brd/`、`velox-config/src/limit.rs`（既有 rustfmt 改动原样保留）。
6. 不处理 crates.io 发布、版本号变更、rust-toolchain 升级。
7. 不重构五包各自的 config.rs 结构（仅改 import/调用路径与依赖声明，最小 diff）。

## 7. 待确认问题

| # | 问题 | 建议默认方案 |
|---|---|---|
| OP-1 | 五包接入形态：顶层再导出（`use velox_common::LogConfig`）还是组件路径（`use velox_common::log::LogConfig`）？ | **顶层再导出**：五包 diff 最小（仅 use 行 crate 名替换），「log 是组件」由 velox-common 内部模块结构表达，不与扩展位冲突（FR-7）；组件路径可作为未来多组件时的演进方向 |
| OP-2 | 「velox-log 不再作为独立 crate 存在」的字面口径：是否保留薄再导出壳 crate（deprecation shim）？ | **完全移除**（目录+member+依赖清零）：workspace 内部库无外部消费者，无兼容包袱；意图原文口径支持完全移除 |
| OP-3 | 历史规格工件（`specs/addlog/01~05`、`specs/instance-concurrency-limit/04/05`）中 velox-log 引用是否回溯修订？ | **原样保留不改写**：改则破坏历史交付证据链（测试统计 28/28、提交锚点等均为当时事实）；残留检查分层豁免（AC-5 层③） |
| OP-4 | ADR 0002 处置方式：改写正文，还是新增 ADR + 0002 加注记？ | **新增 ADR 记录本次结构重构决策 + 0002 顶部加 superseded/更名注记，0002 正文不改写**：rotate 设计决策本身未被推翻，仅 crate 归属变化；符合 ADR 不可变惯例，且满足「引用同步更新」（活文档口径经注记指向新结构） |
| OP-5 | 行为字符串锚点：stderr 报告行 `component=velox-log`（`report.rs:291`）与 worker 线程名 `"velox-log"`（`lib.rs:187`）是否随 crate 改名？ | **保留原值不变**：二者标识「日志组件」自身而非 crate 包名，属于 NFR-1 行为零回退范围（行格式是文档化契约）；改动将连带测试/文档锚点漂移且无收益 |
| OP-6 | subsystems.md 三处注释与 development.md:67 的「addlog」历史术语（无 velox-log 字面）是否顺带统一为新口径？ | **不动**：与 OP-3 同口径（历史 feature 代号保留）；仅核对三处注释描述的行为仍准确。若上游要求术语统一，一并改为「日志组件（velox-common log 组件）」并同步 ADR 指向 |

## 8. 影响面

### 8.1 代码与构建清单（须改动）

| 文件 | 触点 |
|---|---|
| `velox-log/`（整目录） | `Cargo.toml`、`src/lib.rs`(295)、`src/rotate.rs`(672)、`src/filter.rs`(242)、`src/report.rs`(370)、`src/bounded.rs`(113)、`src/tests.rs`(1654，28 例) → 迁入 `velox-common/` log 组件 |
| `Cargo.toml`（根） | `:9` members、`:26` `[workspace.dependencies]` path 依赖 |
| `Cargo.lock` | velox-log 条目与五包依赖列表共 6 处（行 181/200/1537/1541/1567/1585）随重生成更新 |
| `velox-config/Cargo.toml:11`、`velox-registry/Cargo.toml:11`、`velox-mesh/Cargo.toml:11`、`velox-demo/provider/Cargo.toml:11`、`velox-demo/consumer/Cargo.toml:11` | `velox-log.workspace = true` → velox-common |
| `velox-config/src/config.rs:4,106-107`；`velox-config/src/main.rs:35` | `use velox_log::{LogConfig, ResolvedLevel}`、`velox_log::absolutize`、`velox_log::resolve_level`、`velox_log::init` |
| `velox-registry/src/config.rs:10,134-135`；`velox-registry/src/lib.rs:94`；`velox-registry/tests/common/mod.rs:39-40` | 同上 + 测试夹具 `LogConfig::default()`/`ResolvedLevel::default()`（测试侧触点） |
| `velox-mesh/src/config.rs:4,108-109`；`velox-mesh/src/main.rs:43` | 同 config 模式 |
| `velox-demo/provider/src/config.rs:7,137-138`；`velox-demo/provider/src/main.rs:38` | 同 config 模式 |
| `velox-demo/consumer/src/config.rs:7,132-133`；`velox-demo/consumer/src/main.rs:39` | 同 config 模式 |

### 8.2 文档清单（活文档须改，历史工件默认豁免）

| 文件 | 现状 | 处置 |
|---|---|---|
| `velox-mono/docs/adr/0002-log-rotation.md` | 6 处 velox-log 字面（行 1/18/41/46/57/58） | OP-4 默认：新增 ADR + 0002 注记 |
| `velox-mono/docs/subsystems.md` | `:61/:111/:171` 三处「子系统日志（addlog）」注释，无 velox-log 字面 | OP-6 默认：不动，核对准确性 |
| `velox-mono/docs/development.md` | `:67`「（addlog，ADR-0002）」，无 velox-log 字面 | OP-6 默认：不动 |
| `velox-mono/docs/architecture.md`、根 `README.md` | 无 velox-log/velox_log/addlog 字面（已核实） | 无改动义务 |
| `velox-mono/docs/specs/addlog/01~05`（含 review 共 7 文件）、`specs/instance-concurrency-limit/04-test-report.md:81`、`05-acceptance.md:86` | 历史工件含 velox-log 引用 | OP-3 默认：原样保留，不计残留 |
| `velox-mono/docs/specs/_pipeline-state.md` | 含 velox-log 引用（2 处） | 流水线状态文件，不触碰 |

### 8.3 已核实无需改动

- `velox-demo/scripts/acceptance.sh`：无 velox-log 字面引用（`VELOX_LOG_DIR` 为脚本自设变量），应零修改 PASS（NFR-2）；
- `rust-toolchain.toml`、`[profile.release]`、workspace 统一元数据：不受影响。

### 8.4 验证方式（不回退约束的落地证明）

1. AC-2 单测对照：迁移后 log 组件测试统计 = 28/28（数量核对 `grep -c "#\[test\]"`）；
2. AC-4 `cargo build --workspace --locked`：lock 一致性直接证明；
3. AC-5 分层 grep：`grep -rn "velox-log\|velox_log"` 按层①②清零、层③④按豁免口径核对；
4. AC-8 五条基线命令全绿 + 提交 diff 文件清单核对（不含 8.5 纪律文件）。
