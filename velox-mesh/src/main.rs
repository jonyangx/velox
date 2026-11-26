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

    // 日志初始化（addlog）：spawn_blocking 执行启动期同步 IO，不占用 tokio worker；
    // poller/两个 server 的 JoinHandle 已在下方保存，ctrl_c 路径统一 await 后释放 guard
    let log_cfg = cfg.log.clone();
    let log_level = cfg.log_level.clone();
    let _log_guard = tokio::task::spawn_blocking(move || {
        velox_log::init(env!("CARGO_PKG_NAME"), &log_cfg, &log_level)
    })
    .await
    .map_err(|e| anyhow::anyhow!("日志初始化任务失败：{e}"))??;

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

    // 关停广播通道：ctrl_c 后通知后台任务与两个 HTTP server 优雅退出（开发规范第 3.3 节）
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);

    // 后台任务：周期刷新服务快照（带优雅退出通道）
    let poller = {
        let discovery = Arc::clone(&discovery);
        let interval = Duration::from_secs(cfg.registry.poll_secs);
        tokio::spawn(discovery::run_poller(
            discovery,
            interval,
            shutdown_rx.clone(),
        ))
    };

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

    // 数据面与管理面并行服务；收到关停信号后用 with_graceful_shutdown 排空在途请求，
    // 替代原先的 abort() 硬中止
    let inbound = {
        let mut shutdown = shutdown_rx.clone();
        tokio::spawn(async move {
            if let Err(e) = axum::serve(inbound_listener, inbound_app)
                .with_graceful_shutdown(async move {
                    // 关停信号到达（或发送端关闭）即开始排空
                    let _ = shutdown.changed().await;
                })
                .await
            {
                tracing::error!(error = %e, "inbound server exited with error");
            }
        })
    };
    let admin = {
        let mut shutdown = shutdown_rx.clone();
        tokio::spawn(async move {
            if let Err(e) = axum::serve(admin_listener, admin_app)
                .with_graceful_shutdown(async move {
                    let _ = shutdown.changed().await;
                })
                .await
            {
                tracing::error!(error = %e, "admin server exited with error");
            }
        })
    };

    tokio::signal::ctrl_c().await?;
    info!("shutdown signal received");
    // 广播关停：两个 server 优雅排空在途请求，poller 退出刷新循环；
    // 等全部任务确认退出后再结束进程
    let _ = shutdown_tx.send(true);
    let _ = poller.await;
    let _ = inbound.await;
    let _ = admin.await;
    Ok(())
}
