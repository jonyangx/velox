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

cargo build --workspace      # 或 scripts/build.sh（支持 --release 与指定子系统）
cargo test --workspace       # 或 scripts/test.sh（CI 门槛：fmt + clippy + build + test）

scripts/run.sh start         # 按 registry → config → mesh 顺序启动全部服务
scripts/run.sh status        # 查看各服务状态
scripts/run.sh stop          # 反向顺序停止
scripts/package.sh           # release 构建 + 打包 dist/velox-<版本>-<os>-<arch>.tar.gz
```

工程脚本说明见 [scripts/README.md](scripts/README.md)。

文档入口：[velox-mono/README.md](velox-mono/README.md)

## License

Apache-2.0
