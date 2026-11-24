//! demo-provider 服务提供方入口：注册 + 心跳 + 提供业务 API（示例，非 velox 子系统）。
//!
//! 职责边界：装配层。配置加载 → 日志初始化 → 领域对象 → 后台任务 → HTTP server。
//! 业务流程在 lifecycle.rs / greeting.rs，HTTP 协议转换在本文件。

mod config;
mod error;
mod greeting;
mod lifecycle;

use std::net::SocketAddr;
use std::sync::Arc;

use axum::routing::get;
use axum::{Json, Router};
use serde_json::{json, Value};
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
    let cfg = Config::load(std::env::args().nth(1).as_deref())?;

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

    // 后台任务：注册 + 心跳
    {
        let lifecycle = Arc::clone(&lifecycle);
        let shutdown = shutdown_rx.clone();
        tokio::spawn(async move {
            lifecycle.run(shutdown).await;
        });
    }

    // 后台任务：周期刷新问候语模板
    {
        let greeting = Arc::clone(&greeting);
        let cc = cfg.config_center.clone();
        let shutdown = shutdown_rx.clone();
        tokio::spawn(async move {
            greeting
                .run_config_refresh(cc, http.clone(), shutdown)
                .await;
        });
    }

    let app = Router::new()
        .route("/healthz", get(healthz))
        .route("/greet/{name}", get(greet))
        .with_state(greeting.clone());

    let addr: SocketAddr = cfg
        .server
        .addr
        .parse()
        .map_err(|e| anyhow::anyhow!("非法监听地址 '{}'：{e}", cfg.server.addr))?;
    let listener = tokio::net::TcpListener::bind(addr).await?;
    info!(
        version = env!("CARGO_PKG_VERSION"),
        addr = %addr,
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

/// 关停：广播给各后台任务，等它们自行收尾。
async fn shutdown_signal(shutdown_tx: watch::Sender<bool>) {
    let _ = tokio::signal::ctrl_c().await;
    info!("shutdown signal received");
    let _ = shutdown_tx.send(true);
}

async fn healthz() -> Json<Value> {
    Json(json!({ "status": "ok", "service": "demo-provider" }))
}

async fn greet(
    axum::extract::Path(name): axum::extract::Path<String>,
    axum::extract::State(greeting): axum::extract::State<Arc<Greeting>>,
) -> Json<Value> {
    Json(json!({
        "message": greeting.render(&name),
        "template": greeting.template(),
        "is_from_config_center": greeting.is_from_config_center(),
    }))
}
