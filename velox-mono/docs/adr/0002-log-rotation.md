# ADR 0002：子系统日志本地文件 rotate（共享 velox-log + 自研硬边界轮转 writer）

- 状态：已接受
- 日期：2026-11-26
- 决策人：addlog feature（BRD `velox-mono/docs/brd/001-addlog.md`；架构定稿 `docs/specs/addlog/02-architecture.md`，GATE2 PASS）
- 关联：ADR 0001（workspace 布局）、development.md §3.4（日志规范）、§5（配置规范）

## 背景

BRD 要求：mono 纳管的五个运行时子系统（velox-mesh / velox-config / velox-registry / demo-provider / demo-consumer）日志本地文件 rotate——保留 3 个文件、每个 200 MiB 上限、落在项目 logs 目录、等级可调默认 warn。

现状（实现前）：日志全部输出 stdout（`tracing-subscriber` fmt + EnvFilter，缺省 info），唯一的"文件日志"来自 `scripts/run.sh` 的 `nohup ... >>logs/<crate>.log 2>&1` 重定向——无限增长、无轮转、仅覆盖脚本启动场景。

## 决策（D1～D8）

| # | 定案 |
|---|------|
| D1 | 新共享库 `velox-log`（workspace path dependency），五包统一初始化；tracing fmt 文本保留，文件侧禁 ANSI。不是第六个运行时子系统。 |
| D2 | **自研** `SizeRotatingWriter` 负责硬字节边界、重启规范化、分阶段失败恢复；官方 `tracing-appender 0.2.4`（`default-features = false`）仅复用有界队列/独立写线程/有界 guard。 |
| D3 | M = 200 MiB（209,715,200 B）、KEEP = 3（含活跃）；**生产不开放轮转参数**，仅库内部测试注入小阈值（`RotationLimits::injected`，max_bytes ≥32、keep 固定 3）。 |
| D4/D5 | 默认 `cwd/logs`，`[log].dir` + `VELOX_<CRATE>__LOG__DIR`（非空惯例）可覆盖，启动自动建目录；命名固定 `<包名>.log / .log.1 / .log.2`；同包多实例必须分目录（如 demo 第二实例 `VELOX_PROVIDER__LOG__DIR=logs/demo-provider-2`）。 |
| D6 | 等级链：非空 `RUST_LOG` > 已设置 `VELOX_<CRATE>__LOG__LEVEL`（含空串，空串非法）> 文件显式 `[log].level` > 内置 warn。先选最高来源、只校验生效字符串；新渠道严格白名单语法（裸六等级或 `target=level`，不接受裸 target/空值），`RUST_LOG` 保留 EnvFilter 完整兼容语法（裸 `warnn` 是合法 target）；生效解析失败 fail-fast 不回退；错误含实际值 + 精确来源（env 变量名 / 绝对文件路径 + `log.level` 键）。调级须重启。 |
| D7 | 后台脚本（run.sh / acceptance.sh）stdout 投 `/dev/null`，stderr 经匿名管道（`2> >(trap '' HUP; exec cat >&2)`）直达调用端；**不创建 stderr.log / nohup.out / 诊断归档**——每个进程的本地日志只有受管的三份。 |
| D8 | 默认 warn 静默 INFO（启动横幅静默）；排障 `RUST_LOG=info` 时版本/监听地址/配置路径/关键配置与日志有效配置全部可查；init 成功后 INFO 记录 source/value/dir/M/KEEP/事件上限/队列容量。 |

## 关键机制与语义

- **硬上界（AC-13 不放宽）**：初始化成功后任一受管文件逻辑长度 ≤ M、总和 ≤ 3M = 600 MiB/进程（五进程最坏 3000 MiB ≈ 2.93 GiB）。验收口径为 `metadata.len()` 求和；`du` 分配块另报不作等价判定。
- **超长事件**：单事件落盘上限 R = min(M, 64 KiB)。超长事件保留 UTF-8 完整前缀并在 R 内预留 ASCII 标记 ` [truncated]\n`；超限容差 0 字节，整块提前轮转的未用空间 < 该块长度（至多 R−1）。
- **启动规范化**（接客之前、装 subscriber 之前）：受管三文件 >M → `set_len(M)` 保留前 M 字节并 WARN（末行可能截断，明确披露）；遗留精确副本 `<stem>.stderr.log` 与 `.log.N`（N≥3）删除并 WARN；其他包文件不动。规范化失败 = 初始化失败不接客。
- **失败状态机**：轮转链 `RemoveOldest → MoveOne → MoveActive → OpenFresh → Ready` 逐 phase 推进，仅成功后前进；`written: Option<u64>` None=未知、绝不沿用旧计数；单条非空写入触发一次有界恢复尝试；write 短写循环补齐、Interrupted ≤3 次、write 0/错误/flush 失败转 Reopen 并丢当前记录尾部；Ready 每条写前做 dev/ino/nlink 身份检查（外部 unlink 恢复）。磁盘故障允许当前事件尾部与恢复前事件丢失（有损披露），不放任越界、不持续反馈、不退出进程。
- **无反馈报告**：轮转/删除/截断/遗留清理 → WARN，IO 失败/队列丢弃 → ERROR，写 stderr 直达 + 固定原子计数；按类别限频 60s（首次立即、抑制计数累加、下次输出带 counts）。**任何内部报告不调用 tracing、不回流队列、不写本地文件**——无 stderr→subscriber 回边，业务停止后小阈值也不会自激轮转。worker 卡死/进程被 kill 时报告不可达，为尽力诊断（不承诺）。
- **异步与关停**：lossy 队列 1024 条（不阻塞数据面；丢弃经 `error_counter` 采样为 QueueDropped ERROR）；文件 IO 全在 appender 专用线程；五个入口以 `spawn_blocking` 执行 init（启动期同步 IO 不占 tokio worker）；后台任务保存 JoinHandle，关停次序 = 广播关停 → 等 producer 结束 → 释放 LogGuard（appender 握手有界：发送 Shutdown ≤100ms、等待完成 ≤1000ms）→ **尽力**排空，不承诺队列满/IO 故障/超期/kill -9 下不丢。
- **历史兼容**：老配置（无 `[log]` 节/未知字段）可用，行为 = 内置默认；正常旧文件续写不清空（E5）。

## 依赖

根 `[workspace.dependencies]` 新增（成员 `workspace = true` 引用；NFR-6）：

```toml
velox-log = { path = "velox-log" }
tracing-subscriber = { version = "0.3", features = ["env-filter", "registry"] }
tracing-appender = { version = "=0.2.4", default-features = false }
```

新增直接外部依赖仅 `tracing-appender`（共享 path crate 不计外部新增）。**传递依赖非零**，实际锁图（`cargo tree -p velox-log --locked`，Cargo.lock 已提交）：

- crossbeam-channel 0.5.17 / crossbeam-utils 0.8.23
- time 0.3.36 / time-core 0.1.2 / time-macros 0.2.18（已用 `cargo update -p time --precise 0.3.36` 固定基线）
- thiserror 2（沿既有）；parking_lot 为 optional、未启用

全部纯 Rust、无 native-tls 类传递依赖（NFR-6）。锁图为提交时解析结果；升级 appender 或 time 越出已验版本须重新评审（CI/build 用 `--locked`）。

## MSRV 验证结果

- workspace 声明 `rust-version = 1.80`；`rust-toolchain.toml` 为 `channel = "stable"`（非精确版本锁，历史现状）。
- 已执行 `cargo +1.80.0 check -p velox-log --all-targets --locked`：**通过**（本 ADR 记录为新图验证证据）。
- 边界（如实声明）：该项只验证 velox-log 新增子图在 1.80 可编译；既有依赖全图是否满足 1.80 属历史问题，不在本 feature 断言范围。

## 备选方案（已否决）

| 备选 | 否决理由 |
|------|---------|
| file-rotate 0.7.6 | API 可满足计数/命名，但错误/诊断/历史收敛契约仍需外围状态与同等故障测试；且 manifest 连 `Compression::None` 都带 chrono/flate2，新增图更大。责任集中到受控 writer。 |
| 复制五份 logging.rs | 错误状态、过滤语法、限长实现维护五套，漏改风险高。 |
| 官方 RollingFileAppender | 仅时间轮转，无 M 硬限制。 |
| flexi_logger 整栈替换 | 需重新适配 tracing 格式/结构化字段，无必要。 |
| stdout 再存 stderr.log / tee / 第四文件 | 仍是无界副本或改变三文件预算；选管道回显，持久诊断交部署侧 journal。 |
| 轮转经 tracing::warn! 反馈 | 小阈值自激轮转；独立 ReportSink 无回边。 |
| 阻塞队列 / guard 无限 join | 可能阻塞数据面/无限等待；选 lossy 1024 + 有界握手。 |
| BytesSurpassed 变体（若用 file-rotate） | 整 buffer 写穿，违背 0 容差。 |

## 后果

- 正面：磁盘占用有硬上界且可预测；重启恢复运维语义清晰（超限收敛/遗留清理/续写）；默认 warn 降噪；等级链兼容存量 `RUST_LOG` 依赖（脚本/历史 e2e）。
- 负面/取舍（已在需求与测试对齐）：超长事件丢尾部；超限旧文件收敛丢尾部；队列满/IO 故障/关停超期限定下允许尽力丢失（不承诺关键日志绝对不丢）；pre-init 失败无本地日志（可查渠道是本次启动 stderr）；stdout 双写在脚本场景被丢弃（裸进程/容器保留）。
- 中立：日志格式沿用 tracing fmt 不变；调级须重启；多实例分目录为部署约定（代码不防御同目录互踩）。

## 参考

- 需求：`docs/specs/addlog/01-requirements.md`（冻结）；架构：`docs/specs/addlog/02-architecture.md`（GATE2 PASS，D1～D8 与 §4 全部机制定案）
- 上游源码取证：tracing-appender 0.2.4（队列/lossy/guard 100ms/1000ms 握手）、file-rotate 0.7.6（否决对比）、time 0.3.36/time-core 0.1.2/time-macros 0.2.18（MSRV 1.67 ≤ 1.80）
