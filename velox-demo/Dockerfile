# syntax=docker/dockerfile:1
# velox-demo 容器镜像：单镜像承载五个二进制
#   velox-registry(:8300) / velox-config(:8200) / velox-mesh(:8100+:8101)
#   demo-provider(:9000) / demo-consumer(:9100)
# 配合 velox-demo/docker-compose.yml 以 network_mode: host 单机拉起（见 velox-demo/README.md）。
#
# 构建上下文必须是仓库根（需要 COPY 整个 workspace）：
#   docker build -f velox-demo/Dockerfile .
#   docker compose -f velox-demo/docker-compose.yml build
# 需 BuildKit（docker 23+ 默认启用；cargo 缓存挂载与 Dockerfile.dockerignore 均依赖它）。

# ---------- 阶段 1：编译（rust:1-slim 跟随仓库 rust-toolchain.toml 的 stable 通道） ----------
FROM rust:1-slim AS builder
WORKDIR /build

# 整个 workspace 入上下文；体积裁剪见 velox-demo/Dockerfile.dockerignore
COPY . .

# 一次性构建五个二进制（二进制名 = crate 名）。
# target/ 与 cargo registry 用缓存挂载：依赖不变时增量编译，
# 产物在挂载卷内，RUN 末尾拷出到 /out 才能被后续 COPY --from 引用。
RUN --mount=type=cache,target=/build/target \
    --mount=type=cache,target=/usr/local/cargo/registry \
    cargo build --release \
      -p velox-registry -p velox-config -p velox-mesh \
      -p demo-provider -p demo-consumer \
 && mkdir -p /out \
 && cp target/release/velox-registry target/release/velox-config target/release/velox-mesh \
      target/release/demo-provider target/release/demo-consumer /out/

# ---------- 阶段 2：运行时（目录布局仿 scripts/package.sh 产物） ----------
FROM debian:bookworm-slim AS runtime

# curl 仅用于 compose healthcheck；ca-certificates 供 TLS 出口流量
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates curl \
 && rm -rf /var/lib/apt/lists/*

WORKDIR /opt/velox

# /opt/velox/bin/  五个 release 二进制
COPY --from=builder /out/ /opt/velox/bin/

# /opt/velox/config/  各服务开发默认配置（来源路径 -> 容器内路径）：
#   velox-registry/config/velox-registry.toml -> /opt/velox/config/velox-registry.toml
#   velox-config/config/velox-config.toml     -> /opt/velox/config/velox-config.toml
#   velox-mesh/config/velox-mesh.toml         -> /opt/velox/config/velox-mesh.toml
#   velox-demo/provider/config/demo-provider.toml -> /opt/velox/config/demo-provider.toml
#   velox-demo/consumer/config/demo-consumer.toml -> /opt/velox/config/demo-consumer.toml
COPY velox-registry/config/velox-registry.toml /opt/velox/config/velox-registry.toml
COPY velox-config/config/velox-config.toml     /opt/velox/config/velox-config.toml
COPY velox-mesh/config/velox-mesh.toml         /opt/velox/config/velox-mesh.toml
COPY velox-demo/provider/config/demo-provider.toml /opt/velox/config/demo-provider.toml
COPY velox-demo/consumer/config/demo-consumer.toml /opt/velox/config/demo-consumer.toml

# velox-config 的 data_dir="./data" 相对 WORKDIR，落盘 /opt/velox/data（容器内临时，见 README 已知限制）
