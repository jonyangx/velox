# GATE1 需求评审意见：addlog（子系统日志文件 rotate）

> 工序：F-GATE1（需求评审关口）· 评审人：QA · 日期：2026-11-26
> 评审对象：`docs/specs/addlog/01-requirements.md`（F-SA 产出）
> 需求原文：`velox-mono/docs/brd/001-addlog.md`（3 条，已逐字比对一致）
> 性质：关口评审（非终验），只读取证，未执行测试，除本文件外未修改任何文件。

---

## 本轮（GATE2 退回后定向复审，2026-11-26）：**PASS**

> 本轮为 **GATE2 REWORK 后对 01-requirements.md 的 GATE1 定向复审**，不是终验、不检查实现提交。复审对象是 SA 按驳回契约完成的四处定向修改（FR-4/AC-9、AC-3、FR-5），其余 FR/AC 按契约应保持不动。下文 §R 为本轮核对，§H 为历史轮次记录（2026-11-26 上午首轮 GATE1 **通过**意见原文，未删改）。

### R0. 结论与依据

**结论：PASS（定向复审通过）。** 四处定向修改全部落实 GATE2 退回要求且内容正确、可判定；改动范围与驳回契约严格一致，未越限修改其他 FR/AC；修订条目满足 GATE1 五项标准。仅余 1 项 P3 编辑性备注，不阻断冻结。

本轮输入与指纹（SHA-256）：

- `01-requirements.md`（当前，修订后）：`ce54e32520d21ca953ffc46d74bba564cb9bc0cb537cd9e0e3452d42643554df`
- GATE2 REWORK 契约来源 `02-architecture-review.md`：`c1db832dfffd8b832440bac166a308c52db0642ca56c95fee32320e8b2fe1e70`（其 §11 记录的 GATE2 时需求文件 SHA `24613278…` 与本轮 before 基线完全一致，证明对照基线无误）
- before 基线 `/tmp/dev-pipeline-cli.jHGxMGJH/addlog-01-requirements.before.md`：`246132785fc544a667b8cc4f465832384f572d488922d66ebb6c8ceb94572fb4`
- SA 落盘 diff 证据 `addlog-requirements.diff`：`c5c9e4d91260dc73fc96d481eca43ebf743bf1a71e76831fe4845b02e3905a8e`

SA 未输出本轮完成标记（上游 CLI 限流中断），按工件事实以当前文件为准确认返工已完成，进入复审。

### R1. 越限核对：当前文件 = before + 恰好四处定向修改

对 before 基线与当前文件做全文 diff（独立复核，非仅采信 SA 自证），全部差异仅以下五处文本，聚合为契约允许的四处条目：

| 条目 | 位置（当前文件） | 契约依据 |
| --- | --- | --- |
| FR-4 | :74（user story 增"重启进程生效"）+ :77-81（渠道优先级/校验/语法/错误四条重写） | G2-8/RQ-3：只修 FR-4 与 AC-9 |
| FR-5 | :87（改为"config 目录下开发默认 toml"，core 三份 + demo 两份实名） | G2-8/RQ-6：仅 FR-5 文件名措辞 |
| AC-3 | :114（日志块定义 R=min(M,64 KiB)、0 字节容差、超限旧文件收敛） | G2-8/RQ-4：仅 AC-3 块/容差定义 |
| AC-9 | :120（三渠道并存、高优先级覆盖非法低级、空串语义） | G2-8/RQ-3 |

**未越限确认**：FR-1/2/3/6、NFR-1~7、AC-1/2/4~8/10~14、§5 E1~E13、§6 范围外、§7 Q1~Q9、§8 影响面均与 before 逐字一致；**AC-13 未动**（契约明令"AC-13 及其他 FR/AC 不动"），业务范围未扩大，未引入生产轮转配置。before 基线两份副本（`addlog-before.md`/`addlog-01-requirements.before.md`）经 cmp 比对完全相同，且当前文件与 diff 证据的 after 侧逐行吻合——SA 自证与工件事实一致。

### R2. 四处修改逐项核验（对照 GATE2 退回要求，以当前文件为准）

**① FR-4/AC-9 —— 等级校验与高优先级覆盖非法低层（RQ-3/G2-3）**

| GATE2 要求 | 当前文件落实 | 核验 |
| --- | --- | --- |
| 把 `RUST_LOG > VELOX_*__LOG__LEVEL > 文件 > warn` 写成已定案契约 | FR-4:77"生效来源优先级固定为：非空 RUST_LOG > 已设置的 LOG__LEVEL > 文件显式 [log].level > 内置 warn"；AC-9 同链 | ✅ |
| AC-9 增加并存场景（高优先级生效、低优先级无效） | AC-9：三渠道不同等级严格按链生效；文件 warnn + LOG__LEVEL=info、文件 warnn + 空 LOG__LEVEL + RUST_LOG=debug → 启动成功且记录来源为高优先级 | ✅ |
| 新配置 warnn/空串按 AC-10/E7 拒绝；RUST_LOG 语法边界明确 | FR-4:79 新渠道语法白名单（裸小写等级或 `target=level`，target 字符集/非空项/拒空项尾逗号，附正反例）；FR-4:80 RUST_LOG 保留 EnvFilter 完整语法，裸 `warnn` 是合法 target、`svc=warnn` 非法，两套语法不混同 | ✅ |
| 生效值记录准确来源，错误含实际值与具体来源，不用"或" | FR-4:81"错误含实际值与准确来源（文件路径及 log.level 键，或具体变量名）；生效值与来源绑定记录，较低来源不冒充生效来源" | ✅ |
| 只校验生效等级字符串；新 env 保留"已设空"事实且不影响存量空值惯例 | FR-4:77"新 LOG__LEVEL 只要已设置就参与选择（含空串）"+ :78"先选择最高优先来源，只校验该生效等级字符串；TOML 语法/类型错误仍须报错"；`RUST_LOG=""` 视作未设置，其余渠道空串非法 | ✅ |
| 语法声明与真实 EnvFilter 语义一致（只读抽查本机锁定源码） | `~/.cargo/registry/.../tracing-subscriber-0.3.23/src/filter/env/directive.rs:215~225`：裸词无法按级别解析时转 target+TRACE（`warnn` 合法、`svc=warnn` 走 Level 分支解析失败）；`builder.rs:168~172`：空串按默认指令构造成功——FR-4/AC-9 的取舍与上游事实相符 | ✅ |

边界与异常覆盖核查：空串（分渠道）、仅空白非空 RUST_LOG、非 UTF-8 选中环境值、TOML 语法错误与等级字符串错误分流、高合法覆盖低非法、`RUST_LOG=""` 回落、全缺省 warn——均有明确判定结果。与 E7（文件 warnn/空串 fail-fast）、E13（off）、§6"调级须重启生效"（FR-4:74）无冲突。

**② AC-3 —— 硬字节边界/超长截断/0 超限容差/旧文件收敛（RQ-4/G2-2）**

| GATE2 要求 | 当前文件落实 | 核验 |
| --- | --- | --- |
| 定案不突破单文件/总量硬上界的写入与超长事件策略、字节定义 | AC-3：日志块=事件聚合后提交的记录，处理后长度 ≤ R=min(M,64 KiB)；剩余空间不足下一块则写前先归档、整块写入新文件；相等时恰好填满；初始化成功后受管文件逻辑长度恒 ≤ M、**超限容差 0 字节**；提前轮转浪费 < 块长（≤ R−1） | ✅ |
| G2-2 静态反例（连续写 2M 缓冲突破 3×M）被消除 | 单块 post-processing ≤ 64 KiB ≪ M，超 R 事件截断为"UTF-8 完整前缀 + R 上限内的截断标记，不另存尾部"——任何注入序列都无法使单文件超过 M，总量上界回到 3×M | ✅ |
| 已有超限旧活跃文件/归档的首次处置及验证时点 | AC-3："已有超限活跃文件或归档须在启动初始化成功前收敛到 ≤ M 并告警，无法收敛则启动失败；正常未超限旧文件仍按 E5 续写/保留"——与 E5 的分工明确、无矛盾 | ✅ |
| AC-13 不变、轮转参数不开放为生产配置 | AC-3 文末"AC-13 总字节上界不变；轮转参数不作为生产配置开放"；AC-13 原文逐字未动（R1 已核）；与 GATE2 RQ-1 闭合（生产常量 + 测试注入）一致 | ✅ |

数值自洽：M=200 MiB 时 R=64 KiB；0 字节容差与"相等时可恰好填满"组合下文件可达恰 M 而不超；浪费上界 R−1 由整块提前轮转直接推出，无内部矛盾。

**③ FR-5 —— 真实开发 toml 名（RQ-6）**

FR-5:87 改为"core 三份为 velox-mesh.toml、velox-config.toml、velox-registry.toml，demo 两份为 demo-provider.toml、demo-consumer.toml"。只读实查仓库五个 config 目录，五个文件名逐一存在且与措辞一致（`velox-mesh/config/`、`velox-config/config/`、`velox-registry/config/`、`velox-demo/provider/config/demo-provider.toml`、`velox-demo/consumer/config/demo-consumer.toml`）。与 §8 影响面既有列表、AC-2 五文件名无冲突。✅

**④ 范围纪律**

四处修改的每一处都能对应到 G2-8 明确授权的条目与子句；未见"顺带重写"、未见放宽 AC-13、未新增 FR/AC 编号、未改 Q1~Q9 实质内容。✅

### R3. 修订条目 GATE1 标准复核

| 标准 | 结论 |
| --- | --- |
| 可判定真伪 | 通过。FR-4 给出优先级/选择/白名单语法与正反例、错误内容与来源；AC-9 为具体 Given/Then 场景（渠道组合、非法组合、回落链）；AC-3 全部量化（M、R、0 容差、R−1、UTF-8 前缀）；FR-5 五个实名文件 |
| 边界与异常覆盖 | 通过。见 R2①/② 的边界清单；新增规则与既有 E5/E7/E8/E13 兼容 |
| 范围外明确 | 未受影响（§6 未动，仍明确排除热调级、按时间轮转、格式改造等） |
| 待确认问题附建议默认方案 | §7 Q1~Q9 原样保留，全部有建议默认方案；本轮修订将 Q5/Q8 相关歧义升级为 FR-4/AC-9/AC-3 固定契约，未新增开放问题 |
| 影响面准确 | §8 未动，首轮 GATE1 已实查吻合（见 §H1.5）；FR-5 修订后与 §8/仓库事实一致 |

### R4. 问题清单（本轮）

| 编号 | 描述 | 证据 | 级别 | 处置建议 |
| --- | --- | --- | --- | --- |
| RQ-A-1 | §7 Q5 行仍以"待确认问题/建议默认方案"措辞表述优先级，而 FR-4 已将同一优先级写为"固定为"。两侧内容完全一致（同一优先链），不存在实质冲突，但"建议"字样可能让读者误以为优先级仍待定 | 01 §7 Q5 vs FR-4:77 | P3（编辑性，不阻断） | 契约限定本轮"只改 FR-4/AC-9、AC-3、FR-5"，SA 未越限是对的；建议 AA/TA 在 GATE2 修订或文档同步时将 Q5 行补一句"已按 FR-4 定案"即可，不构成本轮驳回条件 |

无其他问题；无 REWORK 修改清单。

### R5. 本轮取证记录

只读手段：git status/log、before vs 当前全文 diff、cmp 基线一致性、`~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/tracing-subscriber-0.3.23`（directive.rs/builder.rs 静态核对 EnvFilter 语义）、五个 config 目录文件名实查。未运行 cargo/测试/服务，除本文件外未改动任何文件。未检查实现提交（本 feature 尚无实现，属 GATE2 之后工序）。

### R6. 历史脉络

- **首轮 GATE1（2026-11-26 上午）：通过**。发现 QA-1~QA-5 五项非阻断意见（QA-1 轮转阈值注入悬空、QA-2 默认 warn vs development.md §3.4、QA-3 AC-2 命名悬于 Q3/Q9、QA-4 优先级链缺 AC、QA-5 FR-5 demo 文件名不准），其中 QA-1/QA-2 移交 GATE2 定案。原文全文见 §H。
- **GATE2（2026-11-26）：REWORK**（见 `02-architecture-review.md`，对象为 02 架构与 03 用例，需求侧问题 G2-8/RQ-3/RQ-4/RQ-6 定向退回 SA，限域 FR-4/AC-9、AC-3、FR-5）。首轮意见 QA-4≈RQ-3、QA-5≈RQ-6，均在本轮修订中闭合；QA-1/QA-2/QA-3 由 GATE2 RQ-1/RQ-2/RQ-5 闭合（架构/用例侧，非本轮复审范围）。
- **本轮（2026-11-26）：GATE1 定向复审 PASS。** 需求文件可冻结；AA/TA 须按 GATE2 意见继续修订 02/03，需求侧不得再以"待确认"为由偏离 FR-4/AC-9/AC-3/FR-5 的固定契约。

---

## H. 历史轮次存档：首轮 GATE1 评审意见（2026-11-26，结论：通过）

> 以下为首轮 GATE1 意见原文存档（对应需求文件 SHA `24613278…`，即本轮 before 基线）。其中的行号/引用以当时版本为准，不代表本轮修订后的当前文件。

### H0. 评审结论：**通过**

01-requirements.md 质量达标：BRD 三条逐字引用无误；影响面代码证据全部实查吻合（含行号）；14 条 AC 均为 Given/When/Then 可判定形式；边界与异常覆盖充分；范围外明确；9 个待确认问题全部附建议默认方案。发现的 5 项问题（QA-1~QA-5）均为非阻断性修正意见，其中 2 项须随 v1/GATE2 定案落实（见 §H3 移交清单）。

### H1. 逐项检查结论

#### H1.1 检查项 1：FR/AC 可判定性 —— 通过（附 QA-1/QA-3）

- FR-1~FR-6 均为行为级描述，实现载体明确留给 F-TA-C（"本需求只约束行为"），层级划分正确。
- AC-1~AC-14 全部为 Given/When/Then，且多数给出可操作观测手段（裸进程复现、文件 grep、文件计数、磁盘占用上界、CI 四命令）。逐条判定：
  - AC-1/2/5/6/7/9/10/11/12/14：可直接执行判定，无歧义。
  - AC-3：可判定；"触发点误差不超过一个日志块"中"日志块"未定义，建议 v1 固化为具体容差（如 ≤ 单次写入缓冲上限），见 QA-1。
  - AC-4：可判定（≤3 计数、最旧删除均可观测）。
  - AC-8：可判定；WARN/ERROR 探针的触发手段依赖具体子系统场景（TC-10 已给出 registry 心跳超时方案），v1 回填时对齐即可。
  - AC-13：主分支"长时间运行"可判定但成本高；备选分支"小上限加速模拟"依赖一个未在任何 FR/Q 中定义的能力（轮转阈值可配置/可注入），属内部悬空引用，见 QA-1。

#### H1.2 检查项 2：边界与异常覆盖 —— 通过

| 要求覆盖的点 | 文档落点 | 结论 |
| --- | --- | --- |
| rotate 满 3 份后的滚动 | FR-2 + AC-4（总数恒 ≤3、最旧删除）+ E5（重启续写、归档计入保留数） | 已覆盖 |
| 200m 上限语义 | Q2（MiB/MB 之辨，建议 200 MiB）+ E12（单位书写歧义收敛为数值 MiB） | 已覆盖 |
| 等级生效范围与优先级 | FR-4 + Q5（RUST_LOG > env > 文件 > 默认）+ E8 + E13（off 关文件与 stdout）+ Q6（双写） | 已覆盖；AC 层缺 RUST_LOG 并存用例，见 QA-4 |
| 异常启动 | E1（目录创建失败 fail-fast）+ E6（缺 `[log]`/未知字段容忍）+ E7（非法级别 fail-fast）+ AC-10/11 | 已覆盖 |
| 磁盘场景 | E2（运行中写失败限频 ERROR 不退出）+ E3（磁盘满不拖垮数据面）+ NFR-2（关停 flush） | 已覆盖 |
| 其他边界 | E4（多实例串写声明不做，文档约定隔离）、E9（容器临时层）、E10（脚本双写冲突，Q7）、E11（轮转并发竞争） | 已覆盖 |

#### H1.3 检查项 3：范围外明确性 —— 通过

§6 列出 10 项（集中采集、热调级、按时间轮转/压缩、格式改造、脱敏审计、metrics、容器卷挂载、velox-mono 本身、历史 feature 工件修订、跨机聚合），与 BRD 三条的边界一致，无范围蔓延；E9 对 docker-compose "最小适配"的表述留了必要口子且不失控。

#### H1.4 检查项 4：9 个待确认问题均附建议默认方案 —— 通过

Q1~Q9 逐条核验：每条均有加粗建议默认方案且给出理由（Q1 含活跃=3、Q2 200 MiB、Q3 纳入 demo、Q4 cwd 相对+可覆盖、Q5 RUST_LOG 最高、Q6 双写、Q7 run.sh 入范围、Q8 fail-fast、Q9 固定序号）。建议方案之间自洽（Q1 口径与 NFR-1 的 3×200MiB=600MiB 计算一致；Q7 与 E10 互为呼应）。另注意到一个未列入清单的开放点（轮转阈值可配置性），见 QA-1。

#### H1.5 检查项 5：影响面准确性 —— 通过（只读代码实查）

| 文档断言 | 实查证据 | 结论 |
| --- | --- | --- |
| 5 个运行时二进制 | 根 `Cargo.toml` members = mesh/config/registry/demo-provider/demo-consumer | ✅ |
| 各 main.rs 现状为 stdout-only fmt()+EnvFilter(缺省 info) | `velox-mesh/src/main.rs:38-42`、`velox-config/src/main.rs:30-35`、`velox-registry/src/main.rs:30-35`、`velox-demo/provider/src/main.rs:34-39`、`velox-demo/consumer/src/main.rs:35-40` —— 行号逐一精确吻合 | ✅ |
| 唯一落盘来自 run.sh 重定向、无限增长 | `scripts/run.sh:43` `nohup ... >>"$VELOX_LOG_DIR/$crate.log" 2>&1 &`；`scripts/common.sh:23` `VELOX_LOG_DIR="$VELOX_ROOT/logs"` | ✅ |
| 当前依赖不具备按大小轮转能力 | 根 `Cargo.toml` `[workspace.dependencies]` 仅 `tracing-subscriber`(env-filter)，Cargo.lock 无 rotation/appender 类 crate | ✅ |
| `.gitignore` 已忽略 `logs/` | `.gitignore:31` `logs/` | ✅ |
| `RUST_LOG` 存量依赖 | `scripts/run.sh:17`、`scripts/README.md:66` | ✅ |
| 历史 e2e 断言 `logs/<crate>.log` | `docs/specs/registry-persistence-watch/03-test-cases.md:38,105` | ✅ |
| 新增依赖须入 `[workspace.dependencies]` 且属架构决策 | `velox-mono/docs/development.md` §1 —— NFR-6 引用准确 | ✅ |
| env 覆盖命名 `VELOX_<CRATE>__LOG__*` | development.md §5 —— FR-3/FR-4 命名符合规范 | ✅ |
| 配置文件与 config.rs 现状 | 5 份 `src/config.rs` 均存在且已用 `#[serde(default)]`；config 目录 5 份 toml 与 §8 影响面所列文件名一致（demo 两份为 `demo-provider.toml`/`demo-consumer.toml`） | ✅ |
| 技术栈约束 tracing | `velox-mono/docs/architecture.md:116`（§6）；`subsystems.md` 确认 velox-mono 为知识中枢非运行时 | ✅ |

影响面未发现遗漏关键模块、未发现虚构文件/行号。

### H2. 问题清单（首轮）

| 编号 | 描述 | 证据 | 严重级别 | 处置建议 |
| --- | --- | --- | --- | --- |
| QA-1 | AC-13（连带 AC-3/AC-4 高成本执行）的备选分支"用小上限加速模拟"预设"轮转阈值可配置/可注入"，但 FR/Q 均未定义该能力，属悬空引用 | 01 AC-13、FR-2；03 TC-02/TC-05（OP-4） | 中 | SA 补 Q10 或注明随 GATE2 OP-4 定案 |
| QA-2 | 默认 warn 与 development.md §3.4"启动日志必含版本/地址/配置路径/关键值"存在张力 | 01 §FR-4/E13；development.md §3.4 | 中 | FR-6 修订 §3.4 时显式处置 |
| QA-3 | AC-2 五文件名为确定项，但 FR-1 文件名仅为示例且悬于 Q3/Q9 | 01 AC-2 vs FR-1/Q3/Q9 | 低 | AC-2 加注"以 Q3/Q9 定案为准" |
| QA-4 | 优先级链仅存在于 Q5 建议方案，AC 层无 RUST_LOG 并存场景 | 01 Q5/E8 vs §4 | 低 | 定案后补优先级 AC |
| QA-5 | FR-5"config/velox-<name>.toml（含 demo 两份）"与实际文件名不符 | 01 FR-5 vs 仓库 demo config 目录 | 低 | FR-5 措辞实名化 |

（首轮不阻断；QA-1/QA-2 建议在 v1 定稿前落实。）

### H3. 移交 GATE2 / v1 定稿的事项（首轮）

1. QA-1：轮转阈值可配置性（与 03 的 OP-3/OP-4 同题）须在 GATE2 一并仲裁，并回填 AC-13 执行手段。
2. QA-2：development.md §3.4 修订必须显式处置"默认 warn vs 启动日志必含信息"。
3. Q1~Q9 九项建议默认方案按工序在 GATE2 定案；其中 Q5/Q7 定案直接影响 AC-9 表述与 run.sh 改动边界。

### H4. 首轮取证清单（只读）

- `velox-mono/docs/brd/001-addlog.md`（逐字比对）
- 5 个 `src/main.rs`（行号实查）、`scripts/run.sh:43`、`scripts/common.sh:23`、`scripts/README.md:66`
- 根 `Cargo.toml`、`Cargo.lock`、`.gitignore:31`
- `velox-mono/docs/development.md` §1/§3.4/§5、`velox-mono/docs/architecture.md:116`、`velox-mono/docs/subsystems.md`
- 5 份 `src/config.rs`、5 份 config toml 文件名
- `docs/specs/registry-persistence-watch/03-test-cases.md:38,105`、`docs/specs/addlog/03-test-cases.md`

---

PIPELINE addlog GATE1 PASS GATE2退回的四处定向修改（FR-4/AC-9等级校验与高优先级覆盖、AC-3硬字节边界与0容差及旧文件收敛、FR-5真实toml名）全部落实且未越限改动其他FR/AC，修订条目经只读抽查（含tracing-subscriber源码与5份config目录）满足GATE1标准，仅余P3编辑备注（§7 Q5措辞与FR-4"固定"表述对齐），需求可冻结进入AA/TA修订
