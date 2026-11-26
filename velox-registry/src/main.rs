//! velox-registry 服务注册中心入口（薄壳）。
//!
//! 职责边界（见 velox-mono/docs/subsystems.md）：只做注册数据与生命周期管理，
//! 不做流量转发。装配逻辑（配置 → 日志 → 领域层 → 持久化 → HTTP 层）
//! 在 `velox_registry::run()`（lib.rs），库入口同时供集成测试 import。

fn main() -> anyhow::Result<()> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(velox_registry::run())
}
