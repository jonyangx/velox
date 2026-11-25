# velox-demo —— 示例与验收环境

为 velox-mesh / velox-registry / velox-config 提供可运行的示例业务服务（demo-provider / demo-consumer）、单机容器化编排与端到端验收脚本。整体架构见 [velox-mono/docs/architecture.md](../velox-mono/docs/architecture.md)（per-host sidecar 模式 §3.1、端口规划 §5）。

> 状态：**已建成**。示例服务（provider/consumer）、docker-compose 一键拉起、`scripts/acceptance.sh` 端到端验收均可用。

## 拓扑与数据流

```
                       ┌─────────────────────────── 单机（host 网络模式） ───────────────────────────┐
                       │                                                                            │
  curl :9100 ──► demo-consumer ──► velox-mesh :8100 ──► demo-provider :9000 ──┐               │
                │  （出口统一走 mesh，       │  数据面轮询 LB；     │  注册+心跳（2~5s）     │               │
                │   /proxy/{service}/*）    │  快照每 5s 刷新     ▼                       ▼               │
                │                           ├─ 快照来源 ──► velox-registry :8300 ◄── demo-consumer 注册 │
                └─ 健康检查/状态 API         └─ 模板来源 ──► velox-config  :8200 ◄── provider 拉取模板      │
                       │                        mesh 管理面 :8101（routes / status）                       │
                       └────────────────────────────────────────────────────────────────────┘
```

- **demo-provider**（:9000）：问候语服务。启动即向 registry 注册并心跳；问候语模板优先从 config（`demo/greeting-template`）周期拉取，未配置时用内置默认。
- **demo-consumer**（:9100）：消费方。不直连 provider，出口流量统一走 mesh 数据面 `/proxy/demo-provider/*`，由 mesh 做服务发现 + 轮询负载均衡。

## 快速开始

前置：Linux（host 网络模式）；host 模式另需 `cargo`、`curl`、`jq`。固定端口 `8100/8101/8200/8300/9000/9001/9100` 必须空闲。

### 方式 A：host 模式（源码运行，推荐日常开发/验收）

```bash
# 一键端到端验收（编译 + 启动 + 全链路断言 + 自动收尾）
bash velox-demo/scripts/acceptance.sh

# 或手动运行：
scripts/build.sh
cargo build -p demo-provider -p demo-consumer
scripts/run.sh start                 # registry(:8300) → config(:8200) → mesh(:8100)
./target/debug/demo-provider velox-demo/provider/config/demo-provider.toml &
./target/debug/demo-consumer velox-demo/consumer/config/demo-consumer.toml &

curl -fsS http://127.0.0.1:9100/api/v1/consumer/greeting/velox
scripts/run.sh stop                  # 停 core 三件套
```

### 方式 B：docker compose 一键拉起（五容器，单镜像）

```bash
# 在仓库根执行（构建上下文 = 仓库根；需 BuildKit，docker 23+ 默认启用）
docker compose -f velox-demo/docker-compose.yml up --build -d
docker compose -f velox-demo/docker-compose.yml ps     # 等全部 healthy
curl -fsS http://127.0.0.1:9100/api/v1/consumer/greeting/velox
docker compose -f velox-demo/docker-compose.yml down
```

五服务共用镜像 `velox-demo:latest`（构建入口 `velox-demo/Dockerfile`），全部 `network_mode: host`，每服务以各自配置启动：

| 服务 | 端口 | 依赖（service_healthy） | 健康检查 |
|------|------|--------------------------|----------|
| velox-registry | :8300 | — | `curl -fsS :8300/healthz` |
| velox-config | :8200 | — | `curl -fsS :8200/healthz` |
| velox-mesh | :8100（数据）/ :8101（管理） | registry、config | `curl -fsS :8100/healthz` |
| demo-provider | :9000 | registry、config、mesh | `curl -fsS :9000/healthz` |
| demo-consumer | :9100 | registry、mesh | `curl -fsS :9100/healthz` |

## 服务 API

### demo-provider（:9000）

| 方法 路径 | 响应 |
|-----------|------|
| GET `/healthz` | `{"status":"ok","service":"demo-provider"}` |
| GET `/api/v1/greeting/{name}` | `{"greeting","name","template_source","instance_id"}`，`template_source` ∈ `builtin` \| `config_center`；`instance_id` 为本实例注册 id（未注册时 `null`），供 LB 去重验证 |
| GET `/api/v1/provider/status` | `{"service","registered","instance_id","template","template_source"}` |

### demo-consumer（:9100）

| 方法 路径 | 响应 |
|-----------|------|
| GET `/healthz` | `{"status":"ok","service":"demo-consumer"}` |
| GET `/api/v1/consumer/greeting/{name}` | `{"via":"velox-mesh","consumer_instance_id","provider":{greeting,name,template_source,instance_id}}`；provider 不可达时 502 `{"code":"provider_unreachable"}` |
| GET `/api/v1/consumer/status` | `{"service","registered","instance_id","mesh_endpoint"}` |

velox 子系统 API（registry / config / mesh）见各自 `README.md`；本 demo 用到的关键端点：`POST /api/v1/registry/services/{name}/instances`（注册）、`GET .../instances?healthy=true`（健康实例）、`PUT /api/v1/config/{ns}/{key}`（写任意 JSON 值）、`GET /api/v1/mesh/routes`（mesh 快照）。

## 配置说明

约定与 velox 子系统一致（开发规范第 5 节）：**argv[1] 可选 TOML 配置路径；环境变量双下划线分层覆盖**，形如 `VELOX_<服务前缀>__<SECTION>__<FIELD>`。

| 服务 | 默认 TOML（镜像内 `/opt/velox/config/`） | 环境变量前缀 |
|------|------------------------------------------|--------------|
| velox-registry | `velox-registry.toml` | `VELOX_REGISTRY__*` |
| velox-config | `velox-config.toml` | `VELOX_CONFIG__*` |
| velox-mesh | `velox-mesh.toml` | `VELOX_MESH__*` |
| demo-provider | `demo-provider.toml` | `VELOX_PROVIDER__*` |
| demo-consumer | `demo-consumer.toml` | `VELOX_CONSUMER__*` |

示例（覆盖 provider 心跳周期与监听地址）：

```bash
VELOX_PROVIDER__REGISTRY__HEARTBEAT_INTERVAL_SECS=2 \
VELOX_PROVIDER__SERVER__ADDR=0.0.0.0:9001 \
VELOX_PROVIDER__REGISTRY__ADVERTISE_PORT=9001 \
  ./target/debug/demo-provider velox-demo/provider/config/demo-provider.toml
```

关键默认值：provider :9000 / consumer :9100，registry `heartbeat_ttl_secs=30` / `sweep_interval_secs=5`，mesh `poll_secs=5`；心跳周期必须小于 registry TTL。

## 验收脚本

```bash
bash velox-demo/scripts/acceptance.sh
```

host 模式端到端验收，任一断言失败即打印原始响应并 `exit 1`（EXIT trap 自动收尾：杀 demo 进程 + `scripts/run.sh stop`）。步骤与断言：

| 步骤 | 内容 / 断言 |
|------|-------------|
| 0 前置 | jq/curl 存在；demo 配置文件在位；7 个固定端口空闲 |
| 1 编译 | `scripts/build.sh`（core 三件套）+ `cargo build -p demo-provider -p demo-consumer` |
| 2 启动 core | `VELOX_REGISTRY__REGISTRY__HEARTBEAT_TTL_SECS=6`、`SWEEP_INTERVAL_SECS=2` 后 `scripts/run.sh start`（短 TTL 仅限验收环境） |
| 3 启动 demo | provider / consumer 各带 `HEARTBEAT_INTERVAL_SECS=2`，pid 放 `.run/`、日志放 `logs/` |
| 4.1 健康检查 | 6 个 `/healthz`：registry :8300、config :8200、mesh :8100 与 :8101、provider :9000、consumer :9100 |
| 4.2 注册发现 | registry 服务清单含 demo-provider/demo-consumer，且 `?healthy=true` 各恰 1 实例 |
| 4.3 直连 | provider greeting `template_source = builtin` |
| 4.4 配置热更 | PUT config `demo/greeting-template`（JSON 字符串，读回一致）→ 轮询 provider status 直至 `config_center`（≤15s） |
| 4.5 转发一致 | consumer greeting：`via=velox-mesh`、`provider.greeting` 与直连一致、`provider.instance_id` 与 provider status 一致、template_source=config_center |
| 4.6 扩容 LB | 起第二 provider（:9001），等 registry 健康实例=2 且 mesh 快照=2，连打 consumer 6 次，去重 provider instance_id = 2（轮询 LB） |
| 4.7 宕机剔除 | SIGKILL 第二实例，等 registry 健康实例回落=1（≤20s，覆盖 TTL6 + sweep2 + mesh poll5） |
| 4.8 优雅注销 | SIGTERM 第一 provider，registry 服务清单不再含 demo-provider（≤15s；若实现仅处理 SIGINT 则由 TTL 剔除兜底） |
| 5 PASS | 打印覆盖摘要 |

## 目录结构

```
velox-demo/
├── Dockerfile                  # 多阶段构建：rust:1-slim 编译 → debian:bookworm-slim 运行
├── Dockerfile.dockerignore     # BuildKit 构建上下文裁剪（target/.git/logs 等）
├── docker-compose.yml          # 五服务一键拉起（host 网络模式，见上表）
├── scripts/acceptance.sh       # 端到端验收（host 模式）
├── provider/                   # demo-provider crate（config/ 为其默认 TOML）
└── consumer/                   # demo-consumer crate（config/ 为其默认 TOML）
```

镜像内布局（仿 `scripts/package.sh` 产物）：`/opt/velox/bin/` 五个二进制 + `/opt/velox/config/` 五份 TOML。

## 已知限制

- **host 网络 Linux 专用**：`network_mode: host` 在 mac/windows Docker Desktop 不可用。跨平台需改为端口映射（`ports: 8100:8100 ...`）并把 demo 配置中的 registry/config/mesh 地址与 `advertise_host` 指向 `host.docker.internal` 变体。
- **mesh 无优雅排空**：实例被剔除（TTL 过期/注销）后，mesh 快照最长滞后 `poll_secs=5`，期间仍可能向已死实例转发。
- **验收专用短 TTL**：脚本将 registry TTL 压到 6s / sweep 2s 以加速验收，仅限验收环境，不代表生产配置。
- **demo 镜像从简**：未做非 root 用户、体积裁剪（multi-stage 已去工具链）与 distroless 化；velox-config 数据落盘容器内 `/opt/velox/data`，容器删除即失（需要持久化时自行挂卷）。
- **provider/consumer 收尾信号**：tokio `ctrl_c` 默认只捕获 SIGINT；SIGTERM 行为以 demo 服务实现为准，验收脚本对 SIGTERM 的注销断言含 TTL 剔除兜底（见 4.8）。
- **BuildKit 依赖**：`Dockerfile.dockerignore` 与 cargo 缓存挂载需 BuildKit；旧构建器需改为在仓库根放 `.dockerignore`。
