# velox 总体架构设计

> 状态：生效中 · 维护者：velox-mono · 关联 ADR：[0001](adr/0001-mono-repo-layout.md)

## 1. 背景与目标

velox 是一套以 Rust 构建的轻量微服务体系，目标运行环境为**单机/少量宿主机 + 容器（Docker，无 Kubernetes）**。设计目标按优先级排列：

1. **部署简单**：不依赖 K8s、Service Mesh 控制面等重型基础设施；
2. **资源可控**：一个宿主机上一个网关进程服务多个容器，避免 per-pod sidecar 资源膨胀；
3. **可演进**：各子系统以清晰边界解耦，后续可独立替换实现（如 registry 换 etcd 后端）。

## 2. 总体拓扑

```
┌────────────────────────── 宿主机 (host) ──────────────────────────┐
│                                                                    │
│   ┌────────────┐   ┌────────────┐   ┌────────────┐                │
│   │ container  │   │ container  │   │ container  │   ... N 个     │
│   │ service A  │   │ service B  │   │ service C  │   业务容器      │
│   └─────┬──────┘   └─────┬──────┘   └─────┬──────┘                │
│         │ localhost:8100 │ localhost:8100 │ localhost:8100        │
│         └────────────────┼────────────────┘                        │
│                    ┌─────▼──────────────┐                           │
│                    │   velox-mesh       │  宿主机 daemon            │
│                    │  (per-host gateway)│  一个进程服务全部容器     │
│                    └───┬──────────┬─────┘                           │
│                        │          │                                 │
│         ┌──────────────▼──┐   ┌───▼──────────────┐                 │
│         │ velox-registry  │   │ velox-config     │                 │
│         │ 注册中心 :8300  │   │ 配置中心 :8200   │                 │
│         └─────────────────┘   └──────────────────┘                 │
└────────────────────────────────────────────────────────────────────┘
```

## 3. 关键设计决策

### 3.1 velox-mesh 采用宿主机 daemon 的 per-host sidecar 模式

- **形态**：mesh **不在业务容器内**，而是以宿主机守护进程（systemd / 裸进程）方式部署；
- **服务对象**：同一宿主机上的**全部**业务容器实例，而非单一容器；
- **接入方式**：容器启动时挂载 `network_mode: host` 或通过 `host.docker.internal` / 网关地址，将出口流量指向 `localhost:8100`；
- **收益**：
  - 业务镜像零侵入（无需注入 sidecar）；
  - 进程级复用：连接池、证书、限流配置全宿主机共享；
  - 故障域清晰：mesh 崩溃影响本机全部出口流量，以 systemd 自动拉起兜底。

详细职责见 [子系统定位 · velox-mesh](subsystems.md#velox-mesh--服务网关)。

### 3.2 控制面与数据面分离

- **数据面**：velox-mesh —— 只做转发，热路径不依赖任何中心组件宕机（注册表快照本地缓存）；
- **控制面**：velox-registry（服务发现数据源）、velox-config（配置数据源）—— 短暂不可用时 mesh 降级使用最后一份快照。

### 3.3 通信协议

| 链路 | 协议 | 说明 |
|------|------|------|
| 业务容器 → mesh | HTTP/1.1、HTTP/2 (h2c) | localhost，明文即可 |
| mesh → 业务服务 | HTTP/1.1 | 反向代理转发 |
| mesh → registry / config | HTTP + JSON | 周期拉取（pull），push 模式留待 v0.3+ |
| 业务服务 → registry | HTTP + JSON | 注册、心跳、注销 |
| 业务服务 → config | HTTP + JSON | 拉取配置 |

> v0.x 全部采用 HTTP + JSON；gRPC 与配置变更推送（长轮询 / watch）列入后续演进。

## 4. 核心数据流

### 4.1 服务上线（注册）

```
1. 容器内服务启动，读取自身 service name / port
2. POST /api/v1/registry/services/{name}/instances → registry
   （携带 address, port, metadata；registry 返回 instance_id）
3. 服务按 TTL（默认 30s）周期 PUT .../heartbeat
4. registry 心跳超时 → 自动将实例标记为不健康并剔除
```

### 4.2 服务发现与转发（mesh）

```
1. mesh 启动时从 registry 全量拉取服务快照，此后每 5s 增量刷新
2. 业务容器请求 http://localhost:8100/proxy/{service}/path
3. mesh 在快照中按 service 名取健康实例 → 轮询负载均衡 → 反向代理转发
4. registry 不可达时，mesh 使用最后一次成功快照（只读降级）
```

### 4.3 配置分发（config）

```
1. 运维通过 PUT /api/v1/config/{namespace}/{key} 写入配置
2. 服务（或 mesh）通过 GET /api/v1/config/{namespace}/{key} 拉取
3. 所有变更落盘（data 目录），config 重启不丢失
```

## 5. 端口规划（开发约定）

| 端口 | 归属 | 用途 |
|------|------|------|
| 8100 | velox-mesh | 入站代理（数据面） |
| 8101 | velox-mesh | 管理 API（路由表、状态） |
| 8200 | velox-config | HTTP API |
| 8300 | velox-registry | HTTP API |
| 9000+ | 业务服务 | 示例服务（velox-demo） |

所有端口均可通过配置文件覆盖；生产环境规划见各子系统 README。

## 6. 技术选型（workspace 统一）

| 领域 | 选型 | 理由 |
|------|------|------|
| 异步运行时 | tokio | 事实标准，生态最全 |
| HTTP 框架 | axum | tokio 官方系、类型安全、中间件成熟 |
| 序列化 | serde / serde_json | JSON 通信 |
| 配置文件 | toml | 人类可读，Rust 生态惯例 |
| 日志 | tracing / tracing-subscriber | 结构化日志 + span 上下文 |
| 错误处理 | thiserror（库/领域错误）+ anyhow（应用顶层） | 各司其职 |
| HTTP 客户端 | reqwest (rustls) | mesh 转发与注册中心访问 |
| TLS | rustls（禁用 native-tls 依赖） | 纯 Rust，交叉编译友好 |

版本统一由根 `Cargo.toml` 的 `[workspace.dependencies]` 管理，各 crate 禁止自行指定版本号。

## 7. 演进路线

| 阶段 | 范围 |
|------|------|
| v0.1（当前） | 三子系统可编译骨架：HTTP API + 内存/文件存储 + 轮询负载均衡 |
| v0.2 | 心跳 TTL 剔除完善、mesh 降级策略、配置监听（长轮询）、集成测试 |
| v0.3 | TLS / mTLS、mesh 热更新路由、registry 集群模式、metrics (Prometheus) |
| v0.4+ | gRPC 代理、灰度发布支持、velox-demo 端到端验收环境 |
