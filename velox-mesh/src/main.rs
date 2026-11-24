//! velox-mesh 服务网关入口（宿主机 daemon，per-host sidecar）。
//!
//! 职责边界（见 velox-mono/docs/subsystems.md）：只做流量转发与服务发现接入，
//! 不做注册数据管理、不做配置存储。main 只负责装配。

mod api;
mod config;
mod discovery;
mod proxy;

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tracing::info;

use crate::config::Config;
use crate::discovery::Discovery;

/// mesh 共享状态（数据面与管理面共用）。
pub struct MeshState {
    pub discovery: Arc<Discovery>,
    /// 数据面转发 client（独立于控制面，超时参数独立）
    pub http: reqwest::Client,
    pub started_at: Instant,
}

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

    // 控制面 client：registry 拉取，短超时快速失败走降级
    let control_http = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(3))
        .timeout(Duration::from_secs(5))
        .build()?;
    // 数据面 client：代理转发，超时由 proxy 配置决定
    let proxy_http = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(cfg.proxy.connect_timeout_secs))
        .timeout(Duration::from_secs(cfg.proxy.request_timeout_secs))
        .build()?;

    let discovery = Arc::new(Discovery::new(cfg.registry.endpoint.clone(), control_http));

    // 启动首拉：失败不阻断启动（降级语义），由 poller 持续重试
    match discovery.refresh().await {
        Ok(n) => info!(instances = n, "initial registry snapshot loaded"),
        Err(e) => tracing::warn!(error = %e, "initial registry snapshot failed, will retry"),
    }

    let state = Arc::new(MeshState {
        discovery: Arc::clone(&discovery),
        http: proxy_http,
        started_at: Instant::now(),
    });

    // 后台任务：周期刷新服务快照
    {
        let discovery = Arc::clone(&discovery);
        let interval = Duration::from_secs(cfg.registry.poll_secs);
        tokio::spawn(discovery::run_poller(discovery, interval));
    }

    let inbound_app = api::inbound_router(Arc::clone(&state));
    let admin_app = api::admin_router(state);

    let inbound_addr: SocketAddr = cfg
        .server
        .inbound_addr
        .parse()
        .map_err(|e| anyhow::anyhow!("非法数据面地址 '{}'：{e}", cfg.server.inbound_addr))?;
    let admin_addr: SocketAddr = cfg
        .server
        .admin_addr
        .parse()
        .map_err(|e| anyhow::anyhow!("非法管理面地址 '{}'：{e}", cfg.server.admin_addr))?;

    let inbound_listener = tokio::net::TcpListener::bind(inbound_addr).await?;
    let admin_listener = tokio::net::TcpListener::bind(admin_addr).await?;
    info!(
        version = env!("CARGO_PKG_VERSION"),
        inbound = %inbound_addr,
        admin = %admin_addr,
        registry = discovery.endpoint(),
        poll_secs = cfg.registry.poll_secs,
        "velox-mesh started (per-host daemon)"
    );

    // 数据面与管理面并行服务；v0.1 ctrl_c 直接中止（优雅排空列入 v0.2）
    let inbound = tokio::spawn(async move {
        if let Err(e) = axum::serve(inbound_listener, inbound_app).await {
            tracing::error!(error = %e, "inbound server exited with error");
        }
    });
    let admin = tokio::spawn(async move {
        if let Err(e) = axum::serve(admin_listener, admin_app).await {
            tracing::error!(error = %e, "admin server exited with error");
        }
    });

    tokio::signal::ctrl_c().await?;
    info!("shutdown signal received");
    inbound.abort();
    admin.abort();
    let _ = inbound.await;
    let _ = admin.await;
    Ok(())
}
