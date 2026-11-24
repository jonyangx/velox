# velox-mono —— velox 项目统一入口

`velox-mono` 是 velox 微服务体系的知识中枢与工程入口，不承载运行时代码，负责：

- 记录 velox 各子系统的**定位与职责边界**
- 维护**总体架构设计**与架构决策记录（ADR）
- 制定并演进**开发规范**（编码、API、Git、测试、配置）

## 子系统总览

| 子项目 | 定位 | 部署形态 | 文档 |
|--------|------|----------|------|
| [velox-mesh](../velox-mesh/) | 服务网关：流量转发、服务发现、负载均衡 | 宿主机 daemon（per-host sidecar） | [定位说明](docs/subsystems.md#velox-mesh--服务网关) |
| [velox-config](../velox-config/) | 配置中心：服务治理与系统配置的存储与分发 | 独立服务（单实例起步） | [定位说明](docs/subsystems.md#velox-config--配置中心) |
| [velox-registry](../velox-registry/) | 服务注册中心：服务注册、心跳、生命周期管理 | 独立服务（单实例起步） | [定位说明](docs/subsystems.md#velox-registry--服务注册中心) |
| [velox-demo](../velox-demo/) | 示例与验收环境 | 容器 | 待建 |

## 文档导航

1. [总体架构设计](docs/architecture.md) —— 拓扑、数据流、端口规划、技术选型
2. [子系统定位](docs/subsystems.md) —— 每个子系统的职责边界、API 概览、演进路线
3. [开发规范](docs/development.md) —— 工具链、编码、API、Git、测试规范（**编码前必读**）
4. [架构决策记录（ADR）](docs/adr/) —— 重大决策的背景、结论与代价

## 快速开始

```bash
# 1. 安装工具链（版本由 rust-toolchain.toml 锁定）
rustup update

# 2. 构建全部子系统
cargo build

# 3. 依次启动（开发环境）
cargo run -p velox-registry   # :8300 服务注册中心
cargo run -p velox-config     # :8200 配置中心
cargo run -p velox-mesh       # :8100 服务网关（宿主机 daemon）

# 4. 静态检查
cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings
```

## 工程布局

```
velox/                    # git 仓库根 = Cargo workspace 根
├── Cargo.toml            # workspace 定义 + 统一依赖版本
├── rust-toolchain.toml   # 工具链锁定
├── velox-mono/           # 本目录：文档中枢（非 crate）
├── velox-mesh/           # 服务网关 crate
├── velox-config/         # 配置中心 crate
├── velox-registry/       # 服务注册中心 crate
└── velox-demo/           # 示例与验收环境
```

> 新增子系统、调整架构前，先阅读[开发规范](docs/development.md)与 [ADR 流程](docs/adr/)。
