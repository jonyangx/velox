# velox-demo —— 示例与验收环境（建设中）

为 velox-mesh / velox-registry / velox-config 提供可运行的示例业务服务（容器化）与端到端验收脚本，随 v0.4 里程碑建设（见 [velox-mono/docs/architecture.md](../velox-mono/docs/architecture.md) 第 7 节）。

规划内容：

1. `echo-service`：最小示例服务，启动即向 velox-registry 注册并心跳，出口流量走 velox-mesh；
2. `docker-compose.yml`：一键拉起 registry + config + mesh + echo-service，验证 per-host sidecar 链路；
3. 验收脚本：注册 → 发现 → 转发 → 心跳超时剔除 全链路断言。
