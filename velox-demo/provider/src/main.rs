//! demo-provider 服务提供方入口：注册 + 心跳 + 提供业务 API（示例，非 velox 子系统）。
//!
//! 职责边界：装配层（见开发规范第 2 节）。配置加载 → 日志初始化 → 领域对象 →
//! 后台任务 → HTTP server；HTTP 协议转换在 api.rs。

mod api;
mod config;
mod error;
mod greeting;
mod lifecycle;

use std::net::SocketAddr;
use std::sync::Arc;

use tokio::sync::watch;
use tracing::info;

use crate::config::Config;
use crate::greeting::Greeting;
use crate::lifecycle::Lifecycle;

fn main() -> anyhow::Result<()> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(run())
}

async fn run() -> anyhow::Result<()> {
    // 配置文件路径为第一个位置参数；缺省时全部使用内置默认值
    let config_path = std::env::args().nth(1);
    let cfg = Config::load(config_path.as_deref())?;

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    // 关停信号：watch 通道扇出给各常驻任务（开发规范第 3.3 节）
    let (shutdown_tx, shutdown_rx) = watch::channel(false);

    let http = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(3))
        .timeout(std::time::Duration::from_secs(5))
        .build()?;

    let lifecycle = Arc::new(Lifecycle::new(cfg.registry.clone(), http.clone()));
    let greeting = Arc::new(Greeting::new());

    // 后台任务：注册 + 心跳（registry 可能晚于本服务启动，内部自带退避重试）
    tokio::spawn(Arc::clone(&lifecycle).run(shutdown_rx.clone()));

    // 后台任务：周期刷新问候语模板
    tokio::spawn(Arc::clone(&greeting).run_config_refresh(
        cfg.config_center.clone(),
        http.clone(),
        shutdown_rx.clone(),
    ));

    let app = api::router(greeting, lifecycle);

    let addr: SocketAddr = cfg
        .server
        .addr
        .parse()
        .map_err(|e| anyhow::anyhow!("非法监听地址 '{}'：{e}", cfg.server.addr))?;
    let listener = tokio::net::TcpListener::bind(addr).await?;
    info!(
        version = env!("CARGO_PKG_VERSION"),
        addr = %addr,
        config = config_path.as_deref().unwrap_or("<内置默认值>"),
        service = %cfg.registry.service_name,
        registry = %cfg.registry.endpoint,
        config_center = %cfg.config_center.endpoint,
        "demo-provider started"
    );

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal(shutdown_tx))
        .await?;
    Ok(())
}

/// 关停：ctrl_c → watch 广播 true → 各后台任务自行收尾 → server 结束。
async fn shutdown_signal(shutdown_tx: watch::Sender<bool>) {
    let _ = tokio::signal::ctrl_c().await;
    info!("收到关停信号，开始优雅退出");
    let _ = shutdown_tx.send(true);
}
