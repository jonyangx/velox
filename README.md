# velox

以 Rust 构建的轻量微服务体系。面向单机/少量宿主机 + 容器（Docker，无 Kubernetes）环境。

## 子系统

| 子项目 | 定位 |
|--------|------|
| [velox-mono](velox-mono/) | 项目统一入口：架构设计、子系统定位、开发规范（**从这里开始**） |
| [velox-mesh](velox-mesh/) | 服务网关：宿主机 daemon（per-host sidecar），服务多容器实例 |
| [velox-config](velox-config/) | 配置中心：服务治理与系统配置存储 |
| [velox-registry](velox-registry/) | 服务注册中心：服务注册与生命周期管理 |
| [velox-demo](velox-demo/) | 示例与验收环境（建设中） |

## 快速开始

```bash
rustup update          # 工具链由 rust-toolchain.toml 锁定
cargo build --workspace
cargo test --workspace
```

文档入口：[velox-mono/README.md](velox-mono/README.md)

## License

Apache-2.0
