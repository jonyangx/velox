# velox 子系统定位

> 每个子系统的职责边界在此固化。**边界变更必须先改本文档并提交 ADR，再动代码。**

---

## velox-mono —— 项目统一入口

**定位**：知识中枢，非运行时组件。

- **做**：总体架构、子系统定位、开发规范、ADR 的唯一权威来源；
- **不做**：不包含可编译的 Rust crate，不放运行时配置。

---

## velox-mesh —— 服务网关

**定位**：宿主机级数据面网关（per-host sidecar daemon），为同宿主机全部业务容器提供统一的出口代理与服务发现接入。

### 职责边界

| 做 | 不做 |
|----|------|
| 入站 HTTP 反向代理（`/proxy/{service}/...`） | 业务逻辑处理 |
| 基于 velox-registry 的服务发现与本地快照缓存 | 服务注册数据的管理（那是 registry 的事） |
| 负载均衡（v0.1 轮询） | 复杂路由规则（灰度、熔断留待 v0.3+） |
| 注册中心不可用时的只读降级 | 配置的存储与分发 |
| 管理 API：路由表/实例表查询（`:8101`） | 跨宿主机的全局流量调度 |

### 部署模型（关键约束）

- 以**宿主机 daemon** 运行（systemd 单元 / 裸进程），**禁止**打进业务容器镜像；
- 一台宿主机 **一个** mesh 进程，服务该机上**全部**容器实例；
- 业务容器通过 `localhost:8100`（host 网络或端口映射）接入；
- 崩溃恢复交由 systemd `Restart=always`。

### API 概览（v0.1）

```
GET  /healthz                        # 数据面健康检查（:8100）
ANY  /proxy/{service}/*path          # 反向代理到目标服务健康实例
GET  /api/v1/mesh/routes             # 当前路由快照（:8101）
GET  /api/v1/mesh/status             # mesh 运行状态（:8101）
```

### 关键配置

```toml
[server]
inbound_addr = "0.0.0.0:8100"   # 数据面监听
admin_addr   = "127.0.0.1:8101" # 管理 API 仅本机可访问

[registry]
endpoint     = "http://127.0.0.1:8300"
poll_secs    = 5                # 快照刷新周期

[proxy]
connect_timeout_secs = 3
request_timeout_secs = 30
```

### 演进路线

v0.1 轮询负载均衡 → v0.2 降级策略完善 / 最少连接 → v0.3 mTLS、路由热更新 → v0.4+ gRPC 代理、灰度。

---

## velox-config —— 配置中心

**定位**：服务治理配置与系统配置的**唯一权威存储**，提供写入、读取与（后续）变更监听。

### 职责边界

| 做 | 不做 |
|----|------|
| 配置的写入、读取、删除（namespace + key 二级结构） | 配置内容的校验规则（由消费方负责） |
| 落盘持久化（SQLite 单文件 `data/velox-config.db`，重启不丢） | 服务发现（那是 registry 的事） |
| 按 namespace 列举配置 | 加密存储（v0.3+ 规划） |
| 变更监听（长轮询，v0.2 规划） | 配置渲染/模板 |

### API 概览（v0.1）

```
GET    /healthz                                  # 健康检查
GET    /api/v1/config/{namespace}                # 列举 namespace 下全部配置
GET    /api/v1/config/{namespace}/{key}          # 读取单项配置
PUT    /api/v1/config/{namespace}/{key}          # 写入（body = JSON 值）
DELETE /api/v1/config/{namespace}/{key}          # 删除
```

### 存储模型

- 逻辑结构：`namespace`（如 `velox-mesh`、`service-a`）→ `key`（如 `governance.toml`）→ JSON 值；
- 物理存储：SQLite 单文件 `data/velox-config.db`，KV 表 `config(namespace, key, value, updated_at)`，复合主键聚簇（`WITHOUT ROWID`），库文件即唯一真相；
- 读缓存：namespace 快照级 TTL 缓存（默认 300 秒，`cache.ttl_secs` 可配，0 表示禁用）；写操作即时驱逐缓存，读己之写不受影响；单实例部署下 TTL 只控制回源频率；
- 启动迁移：目录中的旧版 `{namespace}.json` 在首次启动时一次性导入，并改名 `{namespace}.json.migrated`。

### 演进路线

v0.1 SQLite 存储 + TTL 读缓存 → v0.2 长轮询监听 + 版本号/etag（`updated_at` 列已预留）→ v0.3 加密项、审计日志 → v0.4+ 集群与备份。

---

## velox-registry —— 服务注册中心

**定位**：服务实例的**唯一事实来源**，管理服务注册、心跳续约、健康状态与生命周期。

### 职责边界

| 做 | 不做 |
|----|------|
| 实例注册 / 注销 / 心跳续约 | 流量转发（那是 mesh 的事） |
| 心跳 TTL 超时自动剔除（后台任务） | 配置存储 |
| 健康/全量实例查询，供 mesh 拉取快照 | 实例间调用链追踪 |
| 实例元数据（version、zone 等）存储与查询 | 复杂服务治理规则 |

### 生命周期状态机

```
              register
   ┌──────┐ ─────────► ┌──────────┐  heartbeat(TTL 内)  ┌──────────┐
   │ 不存在 │            │ HEALTHY  │ ──────────────────► │ HEALTHY  │
   └──────┘            └────┬─────┘                     └──────────┘
        ▲                   │ TTL 超时 / deregister          │ deregister
        │                   ▼                                ▼
        └───────────── ┌───────────┐ ◄──────────────── ┌──────┴───┐
                       │  剔除/移除 │                   │  REMOVED │
                       └───────────┘                   └──────────┘
```

### API 概览（v0.1）

```
GET    /healthz                                              # 健康检查
POST   /api/v1/registry/services/{name}/instances            # 注册 → instance_id
PUT    /api/v1/registry/services/{name}/instances/{id}/heartbeat  # 心跳续约
DELETE /api/v1/registry/services/{name}/instances/{id}       # 注销
GET    /api/v1/registry/services                             # 服务名列表
GET    /api/v1/registry/services/{name}/instances            # 实例列表（?healthy=true 过滤）
```

### 关键配置

```toml
[server]
addr = "0.0.0.0:8300"

[registry]
heartbeat_ttl_secs   = 30   # 心跳超时阈值
sweep_interval_secs  = 5    # 后台剔除扫描周期
```

### 演进路线

v0.1 内存注册表 + TTL 剔除 → v0.2 状态持久化、watch 推送 → v0.3 多节点仲裁（Raft）/ etcd 后端适配 → v0.4+ 多机房 zone 感知。

---

## velox-demo —— 示例与验收环境

**定位**：提供可运行的示例业务服务（容器化）与端到端验收脚本，验证 mesh / registry / config 协同工作。已建成：`demo-provider` / `demo-consumer` 两个示例服务（注册 + 心跳 + 经 mesh 转发）、`docker-compose.yml` 一键拉起、`scripts/acceptance.sh` 端到端验收，详见 [velox-demo/README.md](../../velox-demo/README.md)。
