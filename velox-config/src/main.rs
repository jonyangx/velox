//! velox-config 配置中心入口。
//!
//! 职责边界（见 velox-mono/docs/subsystems.md）：只做配置存储与读取，
//! 不做服务发现、不做配置校验规则。main 只负责装配。

mod api;
mod cache;
mod config;
mod error;
mod limit;
mod store;

use std::net::SocketAddr;
use std::sync::Arc;

use tracing::info;

use crate::config::Config;
use crate::store::Store;

fn main() -> anyhow::Result<()> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(run())
}

async fn run() -> anyhow::Result<()> {
    let cfg = Config::load(std::env::args().nth(1).as_deref())?;

    // 日志初始化（addlog）：spawn_blocking 执行启动期同步 IO（建目录/规范化/打开活跃），
    // 不占用 tokio worker；guard 持有至 run() 结束（尽力排空已入队记录）
    let log_cfg = cfg.log.clone();
    let log_level = cfg.log_level.clone();
    let _log_guard = tokio::task::spawn_blocking(move || {
        velox_log::init(env!("CARGO_PKG_NAME"), &log_cfg, &log_level)
    })
    .await
    .map_err(|e| anyhow::anyhow!("日志初始化任务失败：{e}"))??;

    let cache_ttl = std::time::Duration::from_secs(cfg.cache.ttl_secs);
    let store = Arc::new(Store::open(&cfg.storage.data_dir, cache_ttl)?);

    let app = api::router(store, cfg.max_concurrent_requests());
    let addr: SocketAddr = cfg
        .server
        .addr
        .parse()
        .map_err(|e| anyhow::anyhow!("非法监听地址 '{}'：{e}", cfg.server.addr))?;
    let listener = tokio::net::TcpListener::bind(addr).await?;
    info!(
        version = env!("CARGO_PKG_VERSION"),
        addr = %addr,
        data_dir = %cfg.storage.data_dir,
        max_concurrent_requests = cfg.max_concurrent_requests(), // NFR-2 / AC-5、AC-7
        cache_ttl_secs = cfg.cache.ttl_secs,
        "velox-config started"
    );

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
    info!("shutdown signal received");
}
