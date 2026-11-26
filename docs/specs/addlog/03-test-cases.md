# 测试用例（v1 定稿）：addlog —— 子系统日志文件 rotate / 目录指定 / 等级调整

> 状态：v1 定稿（F-TA-D 同步升级轮）——已对齐 `docs/specs/addlog/01-requirements.md` **冻结 AC**（AC-1~14；GATE1 定向复审 **PASS**，2026-11-26 11:06，四处限域修订 FR-4/AC-9、AC-3、FR-5 全部落实且未越限，需求文件 SHA `ce54e325…` 与复审 R0 记录一致）。
> 架构读取版本：`02-architecture.md` 工作树当前版 SHA `a154e264…`（F-AA 第 1 轮架构返工 + 冻结基线映射核对最小同步已完成；本轮按其 §6.3「逐项 v1 TC 回填合同」逐条对齐，TC-13/TC-14→AC-9 映射、TC-06 活体夹具、TC-19 常量口径等均按该合同落位，无 AA/TA 接口分歧）。
> 底稿：v1（2026-11-26 08:45，SHA `2f9250d9…`，对齐修订前需求 `24613278…` 版，头部含待同步标注）。本轮为冻结 AC 同步：**保留 FC 分组（A~F）与 TC-01~TC-25 编号体系，不撤销任何 AC**；底稿 §6 RQ-1~RQ-6 与 §7 OP-1~OP-10 已全部由 GATE2 仲裁＋SA 限域修订＋GATE1 复审闭合（映射见 §6/§7），相应「挂起/待定/双轨」标注全部解除。
> 本轮只做静态设计与文档更新（唯一输出为本文件），只读取证代码/脚本/依赖；未执行构建、测试、服务启动或轮转写入，文中全部期望为验收判据，**无任何通过结论**。

## 1. 测试范围与分层策略（单测/集成/e2e 边界）

**测试对象**：5 个运行时二进制（velox-mesh / velox-config / velox-registry / demo-provider / demo-consumer）的日志初始化与轮转行为——文件落盘（AC-1/2）、硬字节边界轮转与保留（AC-3/4/5/13）、目录与命名（AC-6/7）、等级链与校验（AC-8/9/10）、默认值兼容（AC-11/12）、文档与脚本同步（AC-14）。velox-mono 无运行时进程，不在测试范围。

**同一实现的分层测试接口**（架构 §6.1 定案，消除双轨）：

| 层 | 接口/位置 | 覆盖内容 | 约定 |
|----|----------|---------|------|
| 单元（writer） | `SizeRotatingWriter::new` 私有构造注入 limits/FileOps(含 FaultFs)/ReportSink；目录须已存在 | 字节边界、归档链、分阶段故障、短写计数 | 不用它验证 init 的 mkdir |
| 局部 dispatch 集成 | `build_dispatch(BuildOptions)` + `tracing::dispatcher::with_default`，不安装全局 subscriber | 完整过滤/限长/队列/worker 链、等级矩阵 | 生产线程显式 clone Dispatch；测试结束先停生产者、释放 dispatch 再 drop guard |
| 全局 init（子进程/真实二进制） | 独立测试子进程（§6.2 自举夹具）与五个真实二进制，每进程一次 | 活体并发轮转（TC-06）、裸进程行为（TC-01/02）、脚本链路（TC-21/22） | 同一测试进程不重复安装全局 subscriber |
| 隔离夹具 | TempDir（temp_dir+pid+原子计数，RAII 清理）、FaultFs（每操作第 N 次失败/短写/WriteZero/Interrupted/flush 失败）、TestReportSink、可注入时间源 | 目录/故障/报告/限频 | 无 uuid/tempfile 新 dev-dep；不用 chmod 已开句柄或 unlink 必致写失败作探针 |

CI 门槛：`scripts/test.sh`（fmt + clippy --workspace --all-targets -D warnings + build + test）；实施后补 `cargo test -p velox-log`、`cargo test --workspace --locked`、`--locked` build；TC-06 活体夹具**默认随 `cargo test --workspace` 执行**（非可选手工路径）。每条 AC 至少 1 条 P0/P1 用例（§4 矩阵，无缺口）。

## 2. 测试环境与基建（框架、命令、夹具、e2e 手段；含侦察结论更新）

**现状日志实现（只读取证，本轮复核）**
- 5 个二进制现状一致：`tracing_subscriber::fmt()` + `EnvFilter::try_from_default_env()`（读 `RUST_LOG`，缺省回退 `"info"`），仅 stdout，无文件落盘、无轮转、无 `[log]` 配置节（velox-mesh/velox-config/velox-registry `main.rs` ~:30-42、demo provider/consumer ~:34-40）。
- 唯一落盘来自 `scripts/run.sh:43` shell 重定向（`nohup ... >>"$VELOX_LOG_DIR/$crate.log" 2>&1 &`，本轮实查仍在；改造为 DA 按 D7 实施项，`scripts/common.sh` 仅管理 core 三服务）；`logs/` 已在 `.gitignore:31`。
- workspace 现有日志依赖仅 `tracing` + `tracing-subscriber 0.3.23`（Cargo.lock 实查），无轮转组件；拟新增 `velox-log`（workspace path）与 `tracing-appender =0.2.4`（架构 §4.8）。
- tracing-subscriber 0.3.23 语义基线（GATE2/GATE1 只读核对）：裸词无法按级别解析时转 target+TRACE（`RUST_LOG=warnn` 为**合法** target）；空串可正常构造过滤器（`RUST_LOG=""`/空 LOG__LEVEL 的分渠道语义见 FR-4）。
- 存量消费方：`RUST_LOG`（`scripts/README.md:66`、`run.sh:17`）、`logs/<crate>.log` 路径（`docs/specs/registry-persistence-watch/03-test-cases.md:38,105` e2e 断言）——回归确认不破坏（NFR-3，TC-21 覆盖），不重写。

**侦察证据更新（本轮③：失效证据修正）**
- ~~「当前无任何 crate 顶层 `tests/` 目录」~~ 已失效：`velox-registry/tests/` 已存在（`watch.rs`/`persistence.rs`/`compat.rs`/`common/mod.rs`/`registry-e2e.sh`/`registry-perf.sh`，registry-persistence-watch feature 交付，develop @ af28e65）。addlog 的子进程/集成测试组织可参照该目录先例；velox-log 落点为其自身 `src/tests.rs`（架构 §4.1/§6.2）。
- **config 目录文件名清单（冻结 FR-5 实名，本轮 `ls` 逐一复核存在）**：`velox-mesh/config/velox-mesh.toml`、`velox-config/config/velox-config.toml`、`velox-registry/config/velox-registry.toml`、`velox-demo/provider/config/demo-provider.toml`、`velox-demo/consumer/config/demo-consumer.toml`。原「config/velox-<name>.toml（demo 两份 QA-5 修正）」泛化写法废弃，用例与文档判定一律按实名五份。
- runtime crate 现有 `*.rs` 源文件 30 个（本轮实查），内嵌 `#[cfg(test)]`/`#[tokio::test]` 先例充分；velox-log 为拟新增库，非第 6 个运行时二进制。
- e2e：`velox-demo/scripts/acceptance.sh`（host 模式，curl+jq，固定端口 8100/8101/8200/8300/9000/9001/9100）；`scripts/run.sh start/stop/status/logs`。macOS 开发机不跑 docker-compose 容器活体（E9 卷挂载范围外）。
- 配置注入：TOML 实名五份（上）+ env `VELOX_<CRATE>__LOG__LEVEL` / `VELOX_<CRATE>__LOG__DIR`（前缀：VELOX_MESH / VELOX_CONFIG / VELOX_REGISTRY / VELOX_PROVIDER / VELOX_CONSUMER，架构 §4.1 表）；测试隔离一律临时目录 + 等级 env 用可注入 EnvInputs（§4.4.1），不改真实进程环境。

**风险与局限（更新后）**
1. 200 MiB 真值直写成本高：生产阈值以**常量检查**（TC-19）+ 可选抽查覆盖，行为判定一律走注入小阈值（能力已由架构 §4.2.1 `RotationLimits` 定案，GATE2 RQ-1 闭合，不再是悬空引用）。
2. 轮转/故障语义敏感：自研 writer 属 M～L 故障敏感实现，必须 §6 全状态测试（TC-04/24/25），不能靠行数或限频替代。
3. 滚动边界丢日志窗口：AC-5 只断言「轮转前后新日志行可 grep + 进程不崩溃」（尽力语义，NFR-2/E11）；队列满、IO 故障、guard 握手期限内未完成允许丢失（§4.6），TC-24 专项覆盖，不写绝对不丢声明。
4. 等级「不可见」断言依赖文件集完整性：等级用例短窗口断言、与小上限隔离运行；off 时文件与 stdout 均无业务日志，内部 stderr 报告不受 off 隐藏。
5. `du` 分配块含文件系统块对齐/目录项开销，不作字节阈值等价断言（TC-07 逻辑字节求和为准，物理占用另报）。
6. 权限类探针（chmod 555）仅作补充，非 root 环境结果为准；确定性失败以路径被文件占用等 FaultFs/真实冲突为主（TC-10）。
7. 默认 warn 行为变更（info→warn）：启动期 INFO 在文件与 stdout 均静默，排障路径 `RUST_LOG=info`（RQ-2 已闭合定案）；历史 INFO grep 需设等级，TC-01/02/20/21/22 探针已按此设计。

## 3. 用例表（TC-x）

> 列说明：类型 = 单元/集成/e2e/手工（审查）；优先级 P0 冒烟 / P1 核心 / P2 健壮性；可自动化 = 是／部分／否。映射列含 AC 与 FR/E 关联（冻结基准）。所有「待定/挂起」标注已按 GATE2 仲裁与冻结需求解除。

### A 组：文件落盘与进程隔离（FR-1）

| TC | 功能点 | 对应 AC | 类型 | 优先级 | 前置条件 | 步骤 | 期望结果 | 可自动化 |
|----|--------|---------|------|--------|---------|------|---------|---------|
| TC-01 | 默认落盘（真实 WARN 探针） | AC-1 / FR-1 | e2e | P0 | 干净临时 cwd（无 `logs/`）；二进制已构建 | ① **裸进程**（非 run.sh）默认配置启动 velox-registry；② 触发真实业务 WARN（短 TTL 注册不续约 → TTL 剔除 WARN）；③ 检查 `<cwd>/logs/` | ① 出现 `logs/velox-registry.log`（活跃名=包名+`.log`，RQ-5 已定案）；② 文件含该 WARN 事件（时间戳+级别+消息）；③ 全程未依赖 run.sh 重定向。默认 warn 下普通启动/注册 INFO 不作落盘证据（§6.3） | 是（bash 驱动 + grep） |
| TC-02 | 5 进程隔离（实名五份） | AC-2 / FR-1 | e2e | P0 | 干净 logs 环境；5 个二进制已构建 | ① 从仓库根并行启动全部 5 个：core 三件套经 run.sh + demo 两个裸进程（config 用实名 `demo-provider.toml`/`demo-consumer.toml`）；② 各触发 ≥1 条可归属事件（默认 warn 下用真实业务 WARN，如 registry TTL；内容归属核对辅以显式 info 启动行交叉验证）；③ 清点 `logs/` 并抽查内容 | ① 5 个独立文件：`velox-mesh.log`/`velox-config.log`/`velox-registry.log`/`demo-provider.log`/`demo-consumer.log`（冻结 AC-2 原文名，**无 Q3/Q9 待定标注**）；② 内容与对应进程输出一致、无串写交错；③ 无多余运行名副本 | 是 |
| TC-03 | 重启续写与旧文件收敛 | AC-4 关联 + AC-3 收敛分支 / FR-2+E5（架构 §4.2.3） | 集成 | P1 | 预置日志目录：① 正常 ≤M 旧活跃+归档；② 旧活跃 >M；③ 归档 .1/.2 >M；④ 部分 rename 后（半途）状态；⑤ 遗留 `.log.N`（N≥3）与旧 `<stem>.stderr.log` | 分别重启进程；对②③记录收敛告警与长度；触发一次轮转后清点 | ① 正常文件续写不清空、归档保留并计入保留数（E5）；②③ 超限文件在**启动初始化成功前**收敛 `set_len(M)`（保留前 M 字节）+ WARN（含原/新长度/路径），无法收敛则启动失败不接客；④ 允许归档空洞、不猜测性补轮转，已有归档保留；⑤ 精确清理遗留 `.log.N`（N≥3）与旧 stderr.log 并 WARN，不误删其他包文件 | 是（FaultFs/临时目录注入） |

### B 组：大小轮转与保留（FR-2，AC-3 冻结硬边界）

| TC | 功能点 | 对应 AC | 类型 | 优先级 | 前置条件 | 步骤 | 期望结果 | 可自动化 |
|----|--------|---------|------|--------|---------|------|---------|---------|
| TC-04 | 精确阈值与硬上界（0 字节超限容差） | AC-3 / FR-2 | 单元（writer 注入）+ 局部 dispatch 集成 | P0 | `RotationLimits` 注入小阈值（max≥32、keep=3）；R=min(M,64 KiB) | 覆盖边界序列：① 写至 M−1 再写小记录；② 恰好填满 M 后再写非空记录；③ 剩余空间不足下一块；④ 单事件 >R（超长）；⑤ 连续多条 >R 超长事件（2M 注入反例）；⑥ 每次轮转后量各文件长度 | ① 不轮转；② M 相等时恰好填满，下一非空块触发轮转；③ **写前先归档，再整块写入新活跃文件**，提前轮转未用空间 ≤R−1；④ 超 R 事件仅保留 UTF-8 完整前缀 + R 内截断标记 ` [truncated]`，不另存尾部；⑤ 任意注入序列下**任何受管文件逻辑长度恒 ≤M、超限容差 0 字节（无「上限+单行」容差）**、文件数恒 ≤3；⑥ 归档序号 .1→.2 固定推进 | 是 |
| TC-05 | 保留上限（含活跃共 3） | AC-4 / FR-2 | 单元/集成（同 TC-04 注入） | P0 | 延续 TC-04 | ① 持续超限滚动 ≥4 次；② 每次滚动后清点该进程日志文件总数并检查最旧文件 | ① 总数恒 ≤3（**含活跃共 3，Q1 闭合定案**）；② 最旧 .2 被删除、其余为最近内容；③ 每次轮转/删除事件经独立 ReportSink 可观测（NFR-4，TC-25） | 是 |
| TC-06 | 轮转并发稳定（活体夹具） | AC-5 / FR-2+NFR-2/E11 | e2e（自举子进程，§6.2） | P1 | velox-log 测试可执行 + 内部 `VELOX_LOG_FIXTURE=1`；M=1024/K=3/队列 1024 | 父测试 `current_exe()` 自举子进程（真实 subscriber+NonBlocking+同一 writer，TCP 夹具）；8 并发客户端各 ≥64 次请求反复轮询健康接口；确认 ≥4 次真实轮转；轮转前后窗口各发 marker；`/stop` 后等待退出 | ① 进程不崩溃；② 轮转前后 marker 均可 grep，最终 marker 在最近三份内；③ 正常节流负载无队列丢失（溢出另设专项，TC-24）；④ stop 后子进程正常退出、文件数 ≤3 且各 ≤M；⑤ 父端有超时与 kill+wait 清理，无孤儿进程。**默认随 `cargo test --workspace` 执行**（§6.2 定案，取消「实现不可注入再降级」双轨） | 是 |
| TC-07 | 磁盘占用上界（逻辑字节口径） | AC-13 / FR-2+NFR-1+G2-1 | 集成/e2e | P1 | 延续 TC-04/05/06 至稳定状态；连续大量输出后 | ① 遍历该进程**所有**受管日志文件求 `metadata.len()` 总和；② 全目录检查无 `stderr.log`/`nohup.out`/第二运行名日志等未受管副本；③ `du -sk` 另录分配块 | ① 逻辑字节总和 ≤ 保留数×上限 = 3M（生产 3×209715200=629145600 字节）；② **无任何不受管无限增长副本（G2-1）**；③ du 仅独立观测（块对齐开销），不作字节阈值等价判定 | 是 |

### C 组：日志目录与命名（FR-3）

| TC | 功能点 | 对应 AC | 类型 | 优先级 | 前置条件 | 步骤 | 期望结果 | 可自动化 |
|----|--------|---------|------|--------|---------|------|---------|---------|
| TC-08 | 目录自动创建 | AC-6 / FR-3 | 集成（完整 init） | P0 | 日志目录指向不存在路径：① 单级 `<tmp>/logs-x`；② 多级缺失 `<tmp>/a/b/c/logs` | 分别经完整 init（create_dir_all→规范化→打开）初始化/启动；触发日志；检查目录与文件 | 两场景目录均自动创建（含父目录），日志正常落盘；**不以要求目录已存在的 `writer::new` 代替 mkdir 路径**（§6.1） | 是 |
| TC-09 | 目录覆盖 | AC-7 / FR-3 | 集成 | P0 | 临时目录 X、Y | ① `[log].dir = X` 启动 → 断言落 X；② env `VELOX_<CRATE>__LOG__DIR=Y` → 断言落 Y；③ 两者均不设 → 落 cwd 相对 `logs`；④ 绝对路径变体 | 三+1 场景落点均正确；dir env 沿用非空过滤惯例；绝对/相对均可（生产 daemon 绝对路径） | 是 |
| TC-10 | 目录创建失败 | FR-3/E1 | 集成 | P2 | 日志 dir 指向：① 被普通文件占用的路径（确定性失败）；② 只读父目录（chmod 555，非 root，补充探针） | 分别启动；观察退出码与错误输出 | 均按冻结 E1 **fail-fast**：启动报错退出，错误含目录路径与底层原因/阶段；不静默降级丢文件日志。chmod 分支仅补充，不替代确定性失败（G2-5） | 部分（chmod 环境依赖） |

### D 组：等级链与校验（FR-4 冻结：RUST_LOG > LOG__LEVEL > 文件 > warn）

| TC | 功能点 | 对应 AC | 类型 | 优先级 | 前置条件 | 步骤 | 期望结果 | 可自动化 |
|----|--------|---------|------|--------|---------|------|---------|---------|
| TC-11 | 默认 warn | AC-8 / FR-4 | 局部 dispatch 集成 + e2e 交叉验证 | P0 | 干净环境，不设任何级别配置/env | ① 注入 INFO 事件（启动生命周期/业务 INFO）；② 真实 WARN 探针（registry TTL 剔除）；③ 读文件与 stdout | ① INFO 行不出现（文件与 stdout 均无）；② WARN 行可见；③ ERROR 由 TC-12 补全三级别判定。现状缺省 info → 本条为行为变更判定项 | 是 |
| TC-12 | ERROR 级默认可见（正常 error! 探针） | AC-8 / FR-4 | 局部 dispatch 集成 | P0 | 同 TC-11 环境 | ① 局部 dispatch 注入**正常 `error!` 事件**；② 检查文件 | ERROR 行在默认 warn 下可见；**文件写失败的 stderr ERROR 报告不等价于文件中 ERROR 行（G2-6 纠正，不作探针）**；与 TC-11 构成 AC-8 完整判定 | 是 |
| TC-13 | 三渠道生效与严格优先级 | **AC-9（冻结映射，TC-13/TC-14 一并映射）** / FR-4 | 集成（EnvInputs 注入） | P0 | 注入文件配置/env/RUST_LOG 组合 | ① 仅文件 `[log].level=info`；② 仅 `LOG__LEVEL=info`；③ 仅非空 `RUST_LOG=info`；④ 三渠道设不同等级并存 | ①②③ 各自 INFO 及以上可见；④ 严格按 `RUST_LOG > LOG__LEVEL > 文件 > warn` 生效，低优先级值不生效；**生效值与来源绑定记录**：来源为具体变量名 / 文件绝对路径+`log.level` 键 / Default，较低来源不冒充（FR-4 冻结） | 是 |
| TC-14 | 高层合法覆盖低层非法、空串回落（RQ-3 闭合） | **AC-9（冻结映射）** / FR-4 | 集成（EnvInputs 注入） | P1 | 按 §4.4.3 例表组合 | 覆盖例表全行：① 文件 warnn+LOG__LEVEL=info；② 文件 warnn+LOG__LEVEL 空串+RUST_LOG=debug；③ 文件 warn+LOG__LEVEL 空串+RUST_LOG 空串；④ `RUST_LOG=""`（视作未设）回落下一来源；⑤ 全缺省；⑥ 文件 level 缺省+无 env | ① 成功、来源=具体 LOG__LEVEL 变量，文件非法字符串不校验；② 成功、来源=RUST_LOG，两个低层非法字符串都不校验；③ **失败**（值空、来源=LOG__LEVEL）；④ `RUST_LOG=""` 回落下一来源，全缺省 → warn/Default；⑤ 来源=Default 而非「某个存在但未配置 level 的文件」；⑥ 同⑤。启动成功场景行为与记录来源均为高优先级 | 是 |
| TC-15 | 非法值 fail-fast 与两套语法分流 | AC-10 / FR-4+E7 | 集成（EnvInputs 注入） | P0 | 注入非法组合 | 新渠道分支：文件 warnn、文件空串、LOG__LEVEL=warnn、LOG__LEVEL 空串（**已设置即参与选择、空串非法，不走存量 env_override 过滤**）、`warn,,svc=info`、`svc=warnn`、数字等级/未知等级/裸 target；RUST_LOG 分支：裸 `RUST_LOG=warnn`、`RUST_LOG=svc=warnn`、仅空白非空 RUST_LOG、选中 env 非 UTF-8 | 新渠道非法值全部启动失败（fail-fast）：错误含实际值 + **准确来源**（文件=绝对路径+log.level 键；env=具体变量名），无「或」歧义、无静默回退；RUST_LOG 分支：**裸 warnn 为合法 target 接受（不得误测为非法等级）**，`svc=warnn` 类显式指令 fail-fast 不回退，仅空白非空非法，非 UTF-8 报错标变量名；TOML 语法/类型错误仍报错但属文件反序列化错误，不混入等级字符串校验 | 是 |
| TC-16 | off 级别 | FR-4+E13 | 集成 | P2 | level="off" 启动 | ① 注入各级别事件；② 检查文件、stdout、stderr | 文件与 stdout 均无业务日志（启动横幅静默属预期）；**内部 stderr 初始化/IO 报告不受 off 隐藏**；进程正常运行 | 是 |
| TC-17 | 五二进制口径一致 | FR-5（AC-11 关联） | 集成/e2e 抽查 | P2 | 5 个二进制均可启动 | 同一等级配置分别作用于 mesh/config/registry/provider/consumer（≥3 抽查），断言同配置同过滤行为 | 行为一致，无「漏改某个 main.rs」类缺陷；共享 velox-log 单一初始化路径；demo 两份与 core 三件套同标准 | 是 |

### E 组：兼容性与默认值（FR-5 冻结实名五份）

| TC | 功能点 | 对应 AC | 类型 | 优先级 | 前置条件 | 步骤 | 期望结果 | 可自动化 |
|----|--------|---------|------|--------|---------|------|---------|---------|
| TC-18 | 旧配置兼容 | AC-11 / FR-5+E6 | 集成 | P0 | ① 各子系统现行配置文件（无 `[log]` 节）原样使用；② 追加未知字段变体 | ① 旧配置启动；② 断言日志行为 = 内置默认值（文件落盘+轮转+warn）；③ 未知字段不报错 | 启动正常；行为与内置默认完全一致（serde(default) 填缺省、不设 deny_unknown_fields 容忍未知字段）；老配置零改动可运行 | 是 |
| TC-19 | CI 与默认值/常量单测（口径修订） | AC-12 / FR-5 | 单元 + CI | P0 | 仓库工作树（velox-log 落地后） | ① `scripts/test.sh` 四命令；② 抽查五份 config.rs tests：**只测 dir/level 默认值（logs/warn）**；③ 共享常量断言：M=209715200、K=3、事件 cap R=65536、队列 1024；④ `RotationLimits` 注入构造单独测试（max≥32、keep=3）；⑤ 新依赖只在 `[workspace.dependencies]`（velox-log path、tracing-appender =0.2.4） | ① 四命令全绿；②③④ 默认值/常量/注入构造均有单测佐证；⑤ 无生产轮转参数、**无生产轮转数值/单位字符串解析测试项（D3 定案删除原该项）**；NFR-6 集中依赖约束成立 | 是 |

### F 组：文档、脚本与健壮性（FR-6 / NFR）

| TC | 功能点 | 对应 AC | 类型 | 优先级 | 前置条件 | 步骤 | 期望结果 | 可自动化 |
|----|--------|---------|------|--------|---------|------|---------|---------|
| TC-20 | 启动日志规范符合性（RQ-2 闭合回填） | AC-14 / FR-6 | e2e | P2 | 子系统可启动 | ① 默认 warn（无任何配置）启动 → 检查启动行可见性；② 显式 `RUST_LOG=info` 启动 → 检查启动行字段；③ 构造一次初始化失败看 stderr | ① **INFO 启动行静默属预期**（RQ-2 定案，不作缺陷）；② 启动行含版本、监听地址、**真实配置文件路径**、各子系统关键配置，**及**日志有效配置（生效等级 source/value、解析后绝对 dir、M、KEEP、事件 cap、队列容量）——不能只查日志配置记录代替完整启动字段；③ 初始化错误始终 stderr 可见，不受日志过滤器影响 | 是（三分支均判定，无挂起） |
| TC-21 | run.sh 适配（无本地副本，G2-1） | FR-6/Q7/E10+NFR-3 | e2e | P1 | DA 按 D7 完成两启动点改造（stdout=/dev/null、stderr 匿名管道回显） | ① `run.sh start` 后持续制造大量输出；② `run.sh logs <crate>` 与 follow；③ stop/restart 循环 2 次；④ 检查 logs 目录全部相关文件；⑤ healthz 失败分支提示核对；⑥ 历史 e2e 路径回归 | ① 无 stderr.log/nohup.out/完整 stdout 本地副本，持续输出后**全部相关文件仍受管**（TC-07 口径）；② `logs` 子命令读 `<crate>.log`、follow 用 `tail -F` 跟随轮转换名；③ 无双写串写；④ 失败分支提示「启动错误已输出到 stderr」，不 tail 不存在的 `.stderr.log`；⑤ registry-persistence-watch 依赖的 `logs/<crate>.log` 路径约定不破坏（NFR-3 最小兼容确认） | 是 |
| TC-22 | acceptance 链路与第二实例 | FR-5/NFR-3/NFR-5+G2-1 | e2e | P1 | 端口 8100/8101/8200/8300/9000/9001/9100 空闲；cargo/curl/jq 就绪 | 运行 `bash velox-demo/scripts/acceptance.sh`（host 模式）；检查两启动点分流与第二 provider 目录 | 全链路 HTTP 断言通过；第二 provider `VELOX_PROVIDER__LOG__DIR=<logdir>/demo-provider-2` 分目录、文件名仍 `demo-provider.log`、**无脚本运行名 `demo-provider-2.log`**；确认旧实例停止后 acceptance 清理旧精确副本（`demo-provider-2.log`/`demo-provider-2.stderr.log`）；demo 进程内文件受管（E9 容器语义：卷挂载范围外） | 是 |
| TC-23 | 文档一致性（含 RQ-A-1 P3 注记） | AC-14 / FR-6 | 手工（审查） | P1 | 代码与文档变更就绪 | 抽查：development.md §2/§3.4（默认 warn 静默、显式 info 启动字段完整）、subsystems.md 关键配置、scripts/README.md、五 crate README、**五份实名默认 toml**（冻结 FR-5 清单）、ADR-0002（D1~D8、依赖锁图、截断/收敛代价、stderr 无文件方案、guard 有界语义）；另核对需求 §7 Q5 行补「已按 FR-4 定案」注记（QA P3 RQ-A-1 文档同步项） | 文档与实际行为一致；配置项名/默认值与代码一致（`[log]` 仅 dir/level+覆盖说明，无 max_bytes/keep 等生产选项）；无速率/配额类误述；Q5 措辞注记到位 | 否（人工抽查，辅以 grep） |
| TC-24 | IO 故障状态机与关停保证（G2-5） | NFR-2/E2/E3 | 集成（FaultFs） | P2 | FaultFs 逐操作注入；共享测试控制器 | 覆盖 §4.3.2 失败表：remove .2 / .1→.2 / 活跃→.1 失败（仅重试当前 phase，不重复移位）；open 失败；open 后 metadata 失败（重 open+metadata）；短写 n<len（同记录续写不轮转）；Interrupted（同次 write ≤3 次）；write 0/错误、flush 失败（Reopen、报 ERROR、丢未写尾部、不重放前缀）；Ready 前身份检查（unlink/replace：dev/ino 不符、nlink=0 → Reopen 不写入不可见 inode）；Reopen 时路径 >M（set_len(M)+WARN）；worker 确定阻塞时 guard 在 2s 测试预算内返回（允许末条不丢）；队列满专项（gate 阻塞 worker、容量 2、error_counter>0，释放后 QueueDropped 可见，生产者不被反压） | 每个/每步失败后 `current/written/归档状态` 与下一条动作符合 §4.3.2 确定表；所有 IO 错误报告为 ERROR（不因 `?` 提前返回遗漏）；任一时刻文件 ≤3 且各 ≤M；未知计数重读不沿用旧值；**不用 chmod 已开句柄/unlink 必致写失败作探针** | 是（FaultFs 确定注入） |
| TC-25 | 无反馈运维报告（G2-4） | NFR-4 / FR-2 | 集成（真实 dispatch+队列） | P2 | M 小于一条轮转报告文本长度；真实 subscriber+NonBlocking | ① 触发轮转至报告产生；② 生产者 stop、队列排空 ack；③ 记录 event/rotate/file-length 计数，连续两个短观测窗观测（测试时钟可快进，不等 60s）；④ 检查 stderr 报告行与限频 | ①② 报告经独立 ReportSink/stderr 可见（轮转/删除/截断/规范化 WARN、IO ERROR；`component=velox-log`、stage、累计计数；限频 60s 首次立即输出、可注入时间源）；③ 业务停止后计数静止——**无自激轮转、无「stderr→subscriber」回边**；④ 不能只用无 subscriber 的 writer 单测证明无反馈 | 是 |

## 4. AC 覆盖矩阵（AC × TC，冻结基准）

| AC | 覆盖 TC | 最高优先级 | 缺口 | 备注 |
|----|---------|-----------|------|------|
| AC-1 | TC-01 | P0 | 无 | 裸进程 + 真实 WARN 探针 |
| AC-2 | TC-02 | P0 | 无 | 实名五文件，待定标注已解除（RQ-5 闭合） |
| AC-3 | TC-04（收敛分支 TC-03） | P0 | 无 | R=min(M,64KiB)、0 超限容差、超长截断、旧文件收敛（RQ-4 闭合） |
| AC-4 | TC-05（+TC-03 归档计数） | P0 | 无 | 含活跃共 3（Q1 定案） |
| AC-5 | TC-06 | P1 | 无 | §6.2 活体夹具定案，双轨取消 |
| AC-6 | TC-08 | P0 | 无 | 完整 init 验 mkdir；反面 TC-10（E1） |
| AC-7 | TC-09 | P0 | 无 | 文件+env 双途径 |
| AC-8 | TC-11 + TC-12 | P0 | 无 | 正常 error! 探针；stderr IO 报告不作等价 |
| AC-9 | **TC-13（P0）+ TC-14（P1）** | P0/P1 | 无 | **冻结映射**：三渠道并存、高合法覆盖低非法、空串回落（RQ-3 闭合） |
| AC-10 | TC-15 | P0 | 无 | 新渠道空串/warnn 拒绝；RUST_LOG 语法分支单独断言 |
| AC-11 | TC-18 | P0 | 无 | 旧配置+未知字段 |
| AC-12 | TC-19 | P0 | 无 | CI 四命令 + dir/level 默认值 + 共享常量/注入构造（无生产轮转解析项） |
| AC-13 | TC-07 | P1 | 无 | 逻辑字节求和 ≤3M + 无未受管副本；du 另报 |
| AC-14 | TC-23（+TC-20） | P1 | 无 | 手工抽查 + 启动规范两分支均判定（RQ-2 闭合） |

**P0/P1 缺口：无**。底稿全部 ★ 挂起项已按 GATE2 仲裁＋冻结需求解除；TC-13/TC-14→AC-9 映射按架构 §6.3 合同回填。

## 5. 边界场景覆盖对照（E1~E13，冻结基准）

| 边界 | 需求期望（01 §5） | 覆盖 TC | 备注 |
|------|------------------|---------|------|
| E1 目录创建失败 | fail-fast，错误含路径与原因 | TC-10（+TC-25 报告面） | 确定性失败为主，chmod 仅补充 |
| E2 运行中写失败 | 限频 ERROR、进程不退出、恢复继续 | TC-24（FaultFs 确定注入） | write 0/短写/flush/身份检查分支全覆盖 |
| E3 磁盘满 | 同 E2，不拖垮数据面 | TC-24（+TC-06 mesh/业务并发场景回归） | 故障损失允许、不越界、不退出 |
| E4 同 crate 多实例 | 不做隔离，文档声明 dir 覆盖 | TC-23（文档声明核对）+TC-22（第二 provider 分目录实证） | 行为本身范围外（01 §6） |
| E5 重启续写 | 不覆盖、归档计入保留数；超限旧文件收敛（AC-3） | TC-03 | 正常 E5 续写与超限收敛分工明确 |
| E6 缺 `[log]`/未知字段 | 内置默认值、容忍未知字段 | TC-18 | AC-11 |
| E7 非法级别（含空串） | fail-fast，含值与**准确来源** | TC-15 | AC-10；新 LOG__LEVEL 空串不走存量过滤 |
| E8 RUST_LOG 并存 | 冻结链 `RUST_LOG > LOG__LEVEL > 文件 > warn` | TC-13/TC-14（映射 AC-9） | Q5/RQ-3 闭合，无 AC 缺口；Q5 行措辞注记归 TC-23 |
| E9 容器临时层 | 进程行为不变，卷挂载范围外 | TC-22（host 模式语义确认）；容器活体不在本机执行 | macOS 无 docker host 网络 |
| E10 脚本双写冲突 | 禁止双写同一文件；无未受管副本 | TC-21/TC-22/TC-07 | D7 定案：stdout 丢弃、stderr 管道回显 |
| E11 轮转并发竞争 | 写入器内部串行、不丢已提交行（尽力） | TC-06（+TC-24 队列专项） | NFR-2 尽力语义 |
| E12 单位书写 | 轮转参数为生产常量，不存在单位/字符串解析项 | TC-19（常量断言）+TC-04（注入构造） | D3 定案，原解析测试项删除 |
| E13 off | 文件与 stdout 均无、横幅静默属预期；stderr 报告不受隐藏 | TC-16 | P2 |

## 6. 需求问题清单（RQ-x）

**无**（RQ 数量 0：冻结 AC-1~14 经逐条核对均可判定、可观测、无自相矛盾）。底稿 §6 RQ-1~RQ-6 全部闭合，映射如下（保留编号以便追溯）：

| 底稿 RQ | 闭合去向 | 证据 |
|---------|---------|------|
| RQ-1（轮转阈值注入悬空） | GATE2 仲裁闭合：生产常量 M=200 MiB/K=3，内部 `RotationLimits` 测试注入 | 01-requirements-review.md §R2②；02-architecture §4.2.1、§9.3 |
| RQ-2（默认 warn vs §3.4） | GATE2 仲裁闭合：默认 warn 下 INFO 启动行静默、显式 info 完整记录、初始化错误 stderr 始终可见 | 02-architecture §9.1 RQ-2 行、§4.5、TC-20 已按此回填 |
| RQ-3（优先级链无 AC） | SA 限域修订冻结：FR-4 固定链 + AC-9 并存/覆盖/空串场景，GATE1 复审 PASS | 01 FR-4:77、AC-9:120；review §R2① |
| RQ-4（日志块未定义） | SA 限域修订冻结：AC-3 块=R=min(M,64 KiB)、0 超限容差、超长截断、旧文件收敛 | 01 AC-3:114；review §R2② |
| RQ-5（AC-2 命名悬 Q3/Q9） | GATE2 仲裁闭合：纳入五二进制、活跃名=包名.log、归档 .1/.2 | 02-architecture §9.1 RQ-5 行；TC-01/02 待定标注已解除 |
| RQ-6（demo toml 文件名） | SA 限域修订冻结：FR-5 实名五份 toml | 01 FR-5:87；review §R2③；本轮 `ls` 逐一复核存在 |

QA 复审 P3 备注处置（不属需求问题）：RQ-A-1（§7 Q5 行仍用「建议」措辞，与 FR-4「固定为」存在编辑性差异）→ 按 review 处置建议纳入 TC-23 文档审查项（核对补「已按 FR-4 定案」注记），优先级链执行一律以冻结 FR-4/AC-9 为准。

架构侧遗留（非需求问题，AA/DA 责任，用例已作为设计依赖衔接）：G2-1~5/7 的实施验收点由 02-architecture §9.1 合同承接（TC-07/21/22、TC-03/04/07、TC-13/14/15、TC-25、TC-24、TC-19/23 分别对应）；本轮 03 已按 §6.3 合同回填，与 AA 无接口分歧。

## 7. 开放问题清单（OP → GATE2 仲裁结果，全部闭合）

| v0 OP | 议题 | 仲裁结果（GATE2 §7 / 02-architecture §9.3） | 用例落点 |
|-------|------|--------------------------------------------|---------|
| OP-1 | demo 二进制纳入 | 闭合：纳入五二进制（Q3） | TC-02/17 |
| OP-2 | 保留计数语义 | 闭合：含活跃共 3（Q1） | TC-05；诊断副本不得绕过上界（TC-07） |
| OP-3 | 等级渠道与优先级 | 闭合：链冻结于 FR-4/AC-9（RQ-3 闭合） | TC-13/14/15 |
| OP-4 | 轮转参数配置/注入 | 闭合：内部注入定案、生产无参数配置（RQ-1） | TC-04/05/07 注入路径；活体 TC-06 按 §6.2 |
| OP-5 | 单位/归档命名/目录基准 | 闭合：200 MiB、.log.1/.log.2、cwd/logs+覆盖（Q2/Q4/Q9） | TC-04/01/09 |
| OP-6 | 脚本双写与重定向 | 闭合：两启动点 stdout 丢弃、stderr 匿名管道回显、无本地副本（D7，取代原副本方案） | TC-21/22/07 |
| OP-7 | 初始化失败契约 | 闭合：fail-fast（Q8） | TC-10；运行中恢复另归 TC-24 |
| OP-8 | 非法配置容错 | 闭合：新渠道严格语法/空串拒绝、RUST_LOG 兼容语法、轮转常量无解析（§4.4） | TC-15/19 |
| OP-9 | 轮转选型与依赖 | 闭合：自研硬边界 writer + tracing-appender =0.2.4 非阻塞层；file-rotate 0.7.6 具体 API 对比与依赖锁图边界落 §4.8/§7（G2-7 由 DA 实施时补实证） | TC-19/23 核对 |
| OP-10 | 记录日志有效配置 | 闭合：init 成功后 INFO 记录 source/value/dir/M/KEEP/cap/队列容量；默认 warn 静默（RQ-2） | TC-20 |

OP-1~OP-10 全部闭合，无遗留开放问题进入 DA 实施；「TA 回填状态不等于实现已通过」——本文件为验收判据，EXECUTE/FINAL 轮按上述口径实跑留痕。本轮不创建 04 报告、不启动验证轮、不修改其他文档/代码/状态文件。
