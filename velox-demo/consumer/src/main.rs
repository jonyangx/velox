//! demo-consumer 服务消费方入口：注册 + 心跳 + 经 velox-mesh 调用 demo-provider
//! （示例，非 velox 子系统）。
//!
//! 职责边界：装配层。配置加载 → 日志初始化 → 领域对象 → 后台任务 → HTTP server。
//! 业务流程在 lifecycle.rs / consumer.rs，HTTP 协议转换在 api.rs。

mod api;
mod config;
mod consumer;
mod error;
mod lifecycle;

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::watch;
use tracing::info;

use crate::config::Config;
use crate::consumer::Caller;
use crate::lifecycle::Lifecycle;

fn main() -> anyhow::Result<()> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(run())
}

async fn run() -> anyhow::Result<()> {
    let config_path = std::env::args().nth(1);
    let cfg = Config::load(config_path.as_deref())?;

    // 日志初始化（addlog）：spawn_blocking 执行启动期同步 IO，不占用 tokio worker
    let log_cfg = cfg.log.clone();
    let log_level = cfg.log_level.clone();
    let _log_guard = tokio::task::spawn_blocking(move || {
        velox_log::init(env!("CARGO_PKG_NAME"), &log_cfg, &log_level)
    })
    .await
    .map_err(|e| anyhow::anyhow!("日志初始化任务失败：{e}"))??;

    // 关停信号：watch 通道扇出给各常驻任务（开发规范第 3.3 节）
    let (shutdown_tx, shutdown_rx) = watch::channel(false);

    // 共享 HTTP 客户端：单请求超时 = mesh.request_timeout_secs（经 mesh 调用上游的上限）
    let http = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(3))
        .timeout(Duration::from_secs(cfg.mesh.request_timeout_secs))
        .build()?;

    let lifecycle = Arc::new(Lifecycle::new(cfg.registry.clone(), http.clone()));
    let caller = Arc::new(Caller::new(cfg.mesh.endpoint.clone(), http.clone()));

    // 后台任务：注册 + 心跳 + 优雅注销；保存 JoinHandle（addlog §4.6 顺序契约）
    let lifecycle_task = {
        let lifecycle = Arc::clone(&lifecycle);
        let shutdown = shutdown_rx.clone();
        tokio::spawn(async move {
            lifecycle.run(shutdown).await;
        })
    };

    let app = api::router(api::AppState::new(lifecycle, caller));

    let addr: SocketAddr = cfg
        .server
        .addr
        .parse()
        .map_err(|e| anyhow::anyhow!("非法监听地址 '{}'：{e}", cfg.server.addr))?;
    let listener = tokio::net::TcpListener::bind(addr).await?;
    info!(
        version = env!("CARGO_PKG_VERSION"),
        addr = %addr,
        config = config_path.as_deref().unwrap_or("(内置默认值)"),
        service = %cfg.registry.service_name,
        registry = %cfg.registry.endpoint,
        mesh = %cfg.mesh.endpoint,
        "demo-consumer started"
    );

    let serve_result = axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal(shutdown_tx.clone()))
        .await;
    // 统一收尾：广播关停 → 等 producer 结束 → 传播结果（日志 guard 在 run() 结束时释放）
    let _ = shutdown_tx.send(true);
    let _ = lifecycle_task.await;
    serve_result?;
    Ok(())
}

/// 关停：广播给各后台任务，等它们自行收尾。
async fn shutdown_signal(shutdown_tx: watch::Sender<bool>) {
    let _ = tokio::signal::ctrl_c().await;
    info!("shutdown signal received");
    let _ = shutdown_tx.send(true);
}
