//! velox-registry 服务注册中心入口。
//!
//! 职责边界（见 velox-mono/docs/subsystems.md）：只做注册数据与生命周期管理，
//! 不做流量转发。main 只负责装配：配置 → 日志 → 领域层 → HTTP 层。

mod api;
mod config;
mod error;
mod registry;

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use tracing::info;

use crate::config::Config;
use crate::registry::Registry;

fn main() -> anyhow::Result<()> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(run())
}

async fn run() -> anyhow::Result<()> {
    let cfg = Config::load(std::env::args().nth(1).as_deref())?;

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let registry = Arc::new(Registry::new(Duration::from_secs(
        cfg.registry.heartbeat_ttl_secs,
    )));

    // 关停广播通道：ctrl_c 后通知后台剔除任务优雅退出（开发规范第 3.3 节），
    // 机制与 velox-demo/provider/src/lifecycle.rs 一致（`watch` 广播 + `changed()` 感知）
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);

    // 后台任务：周期剔除心跳超时实例（生命周期管理的核心，带优雅退出通道）
    let sweeper = {
        let registry = Arc::clone(&registry);
        let interval = Duration::from_secs(cfg.registry.sweep_interval_secs);
        let mut shutdown = shutdown_rx;
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(interval);
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                tokio::select! {
                    _ = shutdown.changed() => {
                        if *shutdown.borrow_and_update() {
                            info!("剔除任务收到关停信号，退出");
                            return;
                        }
                    }
                    _ = ticker.tick() => {
                        let removed = registry.sweep_expired();
                        if removed > 0 {
                            tracing::warn!(removed, "swept expired instances");
                        }
                    }
                }
            }
        })
    };

    let app = api::router(registry);
    let addr: SocketAddr = cfg
        .server
        .addr
        .parse()
        .map_err(|e| anyhow::anyhow!("非法监听地址 '{}'：{e}", cfg.server.addr))?;
    let listener = tokio::net::TcpListener::bind(addr).await?;
    info!(
        version = env!("CARGO_PKG_VERSION"),
        addr = %addr,
        heartbeat_ttl_secs = cfg.registry.heartbeat_ttl_secs,
        sweep_interval_secs = cfg.registry.sweep_interval_secs,
        "velox-registry started"
    );

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal(shutdown_tx))
        .await?;
    // 等后台剔除任务确认退出，避免进程退出时它被静默 drop
    let _ = sweeper.await;
    Ok(())
}

/// 等待 ctrl_c 并通过 `watch` 通道向后台任务广播关停信号。
async fn shutdown_signal(tx: tokio::sync::watch::Sender<bool>) {
    let _ = tokio::signal::ctrl_c().await;
    info!("shutdown signal received");
    let _ = tx.send(true);
}
