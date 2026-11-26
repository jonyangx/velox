# velox 开发规范

> 编码前必读。规范与代码冲突时：先改规范（走 PR 评审），再改代码；禁止静默偏离。

## 1. 工具链

- Rust stable，版本由根目录 `rust-toolchain.toml` 锁定，本地不得使用其他版本；
- 提交前必须通过：

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo build --workspace
cargo test --workspace
```

- 依赖只允许声明在 `[workspace.dependencies]`（根 `Cargo.toml`），成员 crate 用 `{ workspace = true }` 引用；
- 新增/升级依赖属于架构决策，PR 描述中说明理由与替代方案。

## 2. 仓库与模块布局

- git 仓库根 = Cargo workspace 根；每个子系统一个顶层目录一个 crate；
- crate 内部模块划分约定：

```
src/
├── main.rs      # 入口：读取配置 → 初始化日志 → 启动 server（只做装配）
├── config.rs    # 配置结构体 + 加载（文件 + 环境变量覆盖）
├── api.rs       # HTTP 路由与 handler（只做协议转换，不写业务逻辑）
├── <domain>.rs  # 领域逻辑（registry.rs / store.rs / proxy.rs ...）
└── error.rs     # 领域错误类型（thiserror）
tests/           # 集成测试（起真实 server 打 HTTP）
```

- `main.rs` 禁止出现业务逻辑；handler 禁止直接操作存储，必须走领域模块。

## 3. 编码规范

### 3.1 命名

- 类型/ trait：`UpperCamelCase`；函数/变量：`snake_case`；常量：`SCREAMING_SNAKE_CASE`；
- 缩写词按 Rust 惯例处理（`HttpClient` 而非 `HTTPClient`）；
- 公共 API（对外 HTTP 接口）路径、JSON 字段一律英文小写 kebab/snake；**文档与代码注释以中文为主**。

### 3.2 错误处理

- 领域错误：定义 `thiserror` 枚举（如 `RegistryError`），实现 `IntoResponse` 供 handler 使用；
- 应用顶层（main / 任务 spawn）：`anyhow::Result`；
- **禁止** `unwrap()` / `expect()` 出现在请求处理路径（启动期配置解析允许 `expect` 并带明确信息）；
- 错误信息面向运维：包含上下文（service 名、实例 id、地址），不泄露内部细节给外部响应。

### 3.3 异步与并发

- 运行时统一 tokio；禁止在 async 上下文调用阻塞函数（如需阻塞用 `tokio::task::spawn_blocking`）;
- 共享状态优先 `Arc<RwLock<...>>`（读多写少）或 `Arc<Mutex<...>>`（tokio 版）；跨 await 持锁必须评估；
- 后台任务（心跳剔除、快照刷新）必须有优雅退出通道（cancellation token 或 channel 关闭）。

### 3.4 日志

- 统一 `tracing`；级别约定：
  - `ERROR`：需要人介入（存储写失败、监听失败）；
  - `WARN`：自动降级/重试可恢复（心跳超时剔除、registry 不可达走降级）；
  - `INFO`：生命周期事件（启动、监听地址、注册/注销）；
  - `DEBUG`/`TRACE`：请求级细节；
- 结构化字段优先于字符串拼接：`tracing::info!(service = %name, instance = %id, "registered")`；
- 启动日志必须包含：版本、监听地址、配置文件路径、关键配置值。
- 文件输出与轮转（addlog，ADR-0002）：运行时子系统进程自身写 `logs/<包名>.log`，
  按大小轮转保留含活跃共 3 份、单份 200 MiB 硬上限（总预算 600 MiB/进程）；
  归档命名固定 `.log.1/.log.2`。轮转参数为内部常量，不开放生产配置。
- 默认级别 `warn`：**默认情况下 INFO（含启动横幅）静默**；排障用 `RUST_LOG=info`
  （完整 EnvFilter 语法，优先级最高）或 `VELOX_<CRATE>__LOG__LEVEL`（严格语法）或
  配置 `[log].level`；调级须重启。生效非法等级启动失败（fail-fast，错误含实际值与来源）。
- 目录覆盖：`[log].dir` / `VELOX_<CRATE>__LOG__DIR`（默认相对 cwd 的 `logs`；
  同机同 crate 多实例必须各自配置不同目录）。

## 4. API 设计规范

- 路径前缀 `/api/v1/`，资源名复数名词，动作用 HTTP 方法表达；健康检查统一 `GET /healthz`；
- 请求/响应 body 一律 JSON，字段 snake_case；
- 统一错误响应结构：

```json
{ "code": "instance_not_found", "message": "instance 'xxx' not found in service 'yyy'" }
```

- `code` 为稳定标识符（客户端可编程处理），`message` 面向人；
- 破坏性 API 变更必须升版本（`/api/v2/`），旧版本保留一个 minor 周期。

## 5. 配置规范

- 配置文件格式 TOML，默认路径 `config/velox-<name>.toml`，可用 `--config` 覆盖；
- 环境变量覆盖：前缀 `VELOX_<CRATE>__`，双下划线分层（如 `VELOX_REGISTRY__SERVER__ADDR`）；
- 配置结构体派生 `Deserialize`，未知字段容忍（`#[serde(default)]`），新配置项必须给默认值向后兼容；
- 仓库中的配置文件只放**开发默认值**；生产配置不得入库（密码类永远走环境变量）。

## 6. 测试规范

- 领域逻辑：单元测试与实现同文件（`#[cfg(test)] mod tests`），覆盖正常路径 + 边界 + 并发；
- HTTP API：集成测试放 `tests/`，用 `axum::Router` 直接起测试服务，不占真实端口；
- 每个子系统的 README 中的 API 示例必须有对应集成测试佐证；
- CI 门槛：fmt + clippy + build + test 全绿。

## 7. Git 工作流

- 分支模型：`main` 保护（PR + CI 必绿），功能分支 `feat/<topic>`、修复 `fix/<topic>`、文档 `docs/<topic>`；
- 提交信息遵循 [Conventional Commits](https://www.conventionalcommits.org/zh-hans/)：

```
<type>(<scope>): <subject>

registry(api): support ?healthy=true filter on instance listing
mesh(proxy): fail over to snapshot when registry unreachable
docs(architecture): add per-host sidecar decision record
```

- type 取值：`feat` / `fix` / `docs` / `refactor` / `test` / `chore` / `perf`；scope 取子系统名（mesh/config/registry/mono）；
- 一次 PR 只做一件事；涉及架构决策的改动必须附带 ADR。

## 8. 评审检查单（Reviewer 用）

- [ ] 是否更新了受影响的 `docs/subsystems.md` 职责边界？
- [ ] 依赖变更是否走 workspace 依赖并说明理由？
- [ ] 错误处理、日志级别是否符合第 3 节？
- [ ] 新 API 是否有集成测试 + 文档同步？
- [ ] 配置新项是否有默认值 + README 更新？
