//! velox-registry 库入口。
//!
//! 职责：导出各模块供集成测试（`tests/`）直接 import（bin crate 私有模块不可从
//! `tests/` import，架构 §4.1/G2-7 配套定案），并承载进程装配 `run()`：
//! 配置 → 日志 → 持久化初始化（恢复在接客之前）→ 后台任务 → HTTP 服务 → 关停最终落盘。

pub mod api;
pub mod config;
pub mod error;
pub mod registry;
pub mod store;

use std::sync::Arc;
use std::time::Duration;

use tracing::info;

use crate::config::Config;
use crate::registry::{now_epoch, Registry, RestoreStats};
use crate::store::{LoadOutcome, SharedStore, SnapshotStore};

/// 启动期持久化初始化产物（NFR-4 启动日志字段来源）。
pub struct PersistenceBoot {
    pub registry: Arc<Registry>,
    /// `enabled=false` 时为 None（与 v0.1 完全一致，FR-5）
    pub store: Option<SharedStore>,
    pub restored_services: usize,
    pub restored_instances: usize,
    pub dropped_expired: usize,
    /// 跳过的损坏文件数（单文件布局取值 0 或 1，TC-39）
    pub skipped: usize,
}

/// 持久化初始化（架构 §4.4 步骤 3，接客之前的一次性同步 IO；独立成函数供集成测试复用）。
///
/// - `enabled=true`：`SnapshotStore::open`（递归建目录，不可建 → 启动失败含路径，B-3）
///   → `load`（Corrupted → WARN 含路径+原因，按空表启动，FR-6）
///   → `Registry::restored`（FR-4 TTL 过滤；dropped_expired>0 → 置 dirty 延迟清理）；
/// - `enabled=false`：`Registry::new`、store=None，**不调用 `load()`**（§4.2 注释为
///   `Missing` 语义，禁用路径不存在「恢复」动作——GATE2 第 2 轮复审非阻塞提示）。
pub fn init_persistence(cfg: &Config) -> anyhow::Result<PersistenceBoot> {
    let ttl = Duration::from_secs(cfg.registry.heartbeat_ttl_secs);
    if !cfg.persistence.enabled {
        return Ok(PersistenceBoot {
            registry: Arc::new(Registry::new(ttl)),
            store: None,
            restored_services: 0,
            restored_instances: 0,
            dropped_expired: 0,
            skipped: 0,
        });
    }
    let store = SnapshotStore::open(&cfg.persistence.data_dir)?;
    let path = store.snapshot_path();
    let (registry, stats, skipped) = match store.load() {
        LoadOutcome::Missing => {
            tracing::info!(path = %path.display(), "持久化快照不存在，按空注册表启动");
            (Registry::new(ttl), RestoreStats::default(), 0)
        }
        LoadOutcome::Corrupted(reason) => {
            tracing::warn!(
                path = %path.display(),
                reason = %reason,
                "持久化快照损坏，跳过并按空注册表启动（FR-6）"
            );
            (Registry::new(ttl), RestoreStats::default(), 1)
        }
        LoadOutcome::Restored(snapshot) => {
            let now = now_epoch();
            let (reg, stats) = Registry::restored(ttl, snapshot, now);
            (reg, stats, 0)
        }
    };
    Ok(PersistenceBoot {
        registry: Arc::new(registry),
        store: Some(Arc::new(store)),
        restored_services: stats.services,
        restored_instances: stats.instances,
        dropped_expired: stats.dropped_expired,
        skipped,
    })
}

/// 服务装配与运行（main.rs 薄壳调用；架构 §4.4 时序）。
pub async fn run() -> anyhow::Result<()> {
    // 1. 配置（解析期拒绝 + validate/等级链解析在 load 内完成，fail fast）
    let cfg = Config::load(std::env::args().nth(1).as_deref())?;

    // 2. 日志初始化（addlog）：spawn_blocking 执行启动期同步 IO（建目录/规范化/打开活跃），
    //    不占用 tokio worker；guard 持有至 run() 结束（sweeper/flusher/最终落盘日志先于其释放）
    let log_cfg = cfg.log.clone();
    let log_level = cfg.log_level.clone();
    let _log_guard = tokio::task::spawn_blocking(move || {
        velox_log::init(env!("CARGO_PKG_NAME"), &log_cfg, &log_level)
    })
    .await
    .map_err(|e| anyhow::anyhow!("日志初始化任务失败：{e}"))??;

    // 3. 持久化初始化（接客之前；恢复/损坏跳过/禁用三路见 init_persistence）
    let boot = init_persistence(&cfg)?;
    let registry = boot.registry;
    let store = boot.store;
    info!(
        enabled = cfg.persistence.enabled,
        data_dir = %cfg.persistence.data_dir,
        flush_interval_secs = cfg.persistence.flush_interval_secs,
        restored_services = boot.restored_services,
        restored_instances = boot.restored_instances,
        dropped_expired = boot.dropped_expired,
        skipped = boot.skipped,
        "persistence initialized"
    );

    // 4. 关停广播通道：ctrl_c 后通知后台任务优雅退出并唤醒挂起 watch（AC-9），
    //    机制与 velox-demo/provider/src/lifecycle.rs 一致（`watch` 广播 + `changed()` 感知）
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);

    // 5. 后台任务：周期剔除心跳超时实例；removed>0 时即时落盘（写穿），
    //    失败 → mark_dirty + ERROR，由周期任务重试（G2-1，后台任务不外发）
    let sweeper = {
        let registry = Arc::clone(&registry);
        let store = store.clone();
        let mut shutdown = shutdown_rx.clone();
        let interval = Duration::from_secs(cfg.registry.sweep_interval_secs);
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(interval);
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                tokio::select! {
                    changed = shutdown.changed() => {
                        if changed.is_err() || *shutdown.borrow_and_update() {
                            info!("剔除任务收到关停信号，退出");
                            return;
                        }
                    }
                    _ = ticker.tick() => {
                        let removed = registry.sweep_expired();
                        if removed > 0 {
                            tracing::warn!(removed, "swept expired instances");
                            if let Some(store) = &store {
                                let registry = &registry;
                                if let Err(e) = store.flush(|| registry.snapshot()).await {
                                    registry.mark_dirty();
                                    tracing::error!(error = %e, "剔除后落盘失败，等待周期任务重试");
                                }
                            }
                        }
                    }
                }
            }
        })
    };

    // 6. 后台任务：周期脏刷盘（仅 enabled，FR-2）。每 tick：take_dirty → true 才 flush：
    //    Ok → 无动作（不清除任何标记，快照之后的新 dirty 留给下个周期）；
    //    Err → mark_dirty + ERROR → 下个周期自动重试直至成功或进程退出，不退避（TC-05b）
    let flusher = store.as_ref().map(|store| {
        let registry = Arc::clone(&registry);
        let store = Arc::clone(store);
        let mut shutdown = shutdown_rx.clone();
        let interval = Duration::from_secs(cfg.persistence.flush_interval_secs);
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(interval);
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                tokio::select! {
                    changed = shutdown.changed() => {
                        if changed.is_err() || *shutdown.borrow_and_update() {
                            info!("刷盘任务收到关停信号，退出");
                            return;
                        }
                    }
                    _ = ticker.tick() => {
                        if registry.take_dirty() {
                            let registry = &registry;
                            if let Err(e) = store.flush(|| registry.snapshot()).await {
                                registry.mark_dirty();
                                tracing::error!(error = %e, "周期刷盘失败，下个周期重试");
                            }
                        }
                    }
                }
            }
        })
    });

    // 7. bind（恢复完成后才接客，FR-3）
    let app = api::router(Arc::new(api::AppState {
        registry: Arc::clone(&registry),
        store: store.clone(),
        shutdown: shutdown_rx,
    }));
    let addr: std::net::SocketAddr = cfg
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

    // 8. serve（shutdown_tx.send(true) → 挂起 watch 全部唤醒，AC-9）
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal(shutdown_tx))
        .await?;

    // 9. 关停收尾：等后台任务退出后最终落盘一次
    //    （B-8 尽力而为：失败仅 ERROR——进程即将退出，无重试目标；
    //    与 flusher 最后一 tick 经 flush_lock 串行，幂等无害）
    let _ = sweeper.await;
    if let Some(flusher) = flusher {
        let _ = flusher.await;
    }
    if let Some(store) = &store {
        let registry = &registry;
        if let Err(e) = store.flush(|| registry.snapshot()).await {
            tracing::error!(error = %e, "关停最终落盘失败（进程即将退出，不重试）");
        }
    }
    Ok(())
}

/// 等待 ctrl_c 并通过 `watch` 通道向后台任务与挂起 watch 广播关停信号。
async fn shutdown_signal(tx: tokio::sync::watch::Sender<bool>) {
    let _ = tokio::signal::ctrl_c().await;
    info!("shutdown signal received");
    let _ = tx.send(true);
}
