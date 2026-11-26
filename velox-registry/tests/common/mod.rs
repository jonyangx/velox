//! 集成测试共享辅助：临时目录、AppState 装配、oneshot 请求工具。
//! 每个测试文件通过 `mod common;` 引用（Rust tests 子目录模块约定）。

#![allow(dead_code)]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use serde_json::Value;
use tower::ServiceExt; // oneshot
use uuid::Uuid;
use velox_registry::api::AppState;
use velox_registry::config::Config;
use velox_registry::registry::Registry;
use velox_registry::store::SharedStore;

/// UUID 隔离的系统临时目录（测试自行清理）
pub fn temp_dir() -> PathBuf {
    std::env::temp_dir().join(format!("velox-registry-it-{}", Uuid::new_v4()))
}

/// 独立构建测试用 Config（不走进程 env）
pub fn config(data_dir: &str, enabled: bool, ttl_secs: u64, flush_secs: u64) -> Config {
    Config {
        server: velox_registry::config::ServerConfig {
            addr: "127.0.0.1:0".to_string(),
        },
        registry: velox_registry::config::RegistryConfig {
            heartbeat_ttl_secs: ttl_secs,
            sweep_interval_secs: 1,
        },
        persistence: velox_registry::config::PersistenceConfig {
            enabled,
            data_dir: data_dir.to_string(),
            flush_interval_secs: flush_secs,
        },
    }
}

/// 测试装配返回类型：(router, 可选 store, registry, 关停发送端)
pub type TestApp = (
    Router,
    Option<SharedStore>,
    Arc<Registry>,
    tokio::sync::watch::Sender<bool>,
);

/// 装配 router；返回 (router, 关停发送端)——挂起 watch / shutdown 测试需要持有 tx
pub fn app_with(
    registry: Arc<Registry>,
    store: Option<SharedStore>,
) -> (Router, tokio::sync::watch::Sender<bool>) {
    let (tx, rx) = tokio::sync::watch::channel(false);
    let state = Arc::new(AppState {
        registry,
        store,
        shutdown: rx,
    });
    (velox_registry::api::router(state), tx)
}

/// 经 `init_persistence`（真实启动路径，架构 §4.4 步骤 3）装配：
/// 返回 (router, store, 关停发送端)
pub fn app_from_config(cfg: &Config) -> anyhow::Result<TestApp> {
    let boot = velox_registry::init_persistence(cfg)?;
    let registry = Arc::clone(&boot.registry);
    let (app, tx) = app_with(Arc::clone(&boot.registry), boot.store.clone());
    Ok((app, boot.store, registry, tx))
}

pub fn get(uri: &str) -> axum::http::Request<axum::body::Body> {
    axum::http::Request::builder()
        .uri(uri)
        .body(axum::body::Body::empty())
        .unwrap()
}

pub fn method(method: &str, uri: &str) -> axum::http::Request<axum::body::Body> {
    axum::http::Request::builder()
        .method(method)
        .uri(uri)
        .body(axum::body::Body::empty())
        .unwrap()
}

pub fn post_json(uri: &str, body: Value) -> axum::http::Request<axum::body::Body> {
    axum::http::Request::builder()
        .method("POST")
        .uri(uri)
        .header("content-type", "application/json")
        .body(axum::body::Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap()
}

/// 执行请求，断言状态码并返回响应 JSON
pub async fn json_of(app: &Router, req: axum::http::Request<axum::body::Body>) -> (u16, Value) {
    let res = app.clone().oneshot(req).await.unwrap();
    let status = res.status().as_u16();
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, value)
}

/// 在后台发起挂起请求，返回 JoinHandle；调用方用 `still_pending`/`finish_within` 判定相位
pub fn spawn_request(
    app: &Router,
    req: axum::http::Request<axum::body::Body>,
) -> tokio::task::JoinHandle<(u16, Value)> {
    let app = app.clone();
    tokio::spawn(async move {
        let res = app.oneshot(req).await.unwrap();
        let status = res.status().as_u16();
        let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap();
        let value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        (status, value)
    })
}

/// 断言请求在 `secs` 内仍未完成（挂起中）
pub async fn still_pending(handle: &mut tokio::task::JoinHandle<(u16, Value)>, secs: u64) {
    let result = tokio::time::timeout(Duration::from_millis(secs * 1000), &mut *handle).await;
    assert!(
        result.is_err(),
        "请求应在 {secs}s 内保持挂起，实际已提前完成：{result:?}"
    );
}

/// 断言请求在 `secs` 内完成并返回 (状态码, JSON, 耗时)
pub async fn finish_within(
    handle: tokio::task::JoinHandle<(u16, Value)>,
    secs: u64,
) -> (u16, Value, Duration) {
    let start = std::time::Instant::now();
    let (status, value) =
        match tokio::time::timeout(Duration::from_millis(secs * 1000), handle).await {
            Ok(Ok(pair)) => pair,
            Ok(Err(join_err)) => panic!("请求任务失败：{join_err}"),
            Err(_) => panic!("请求未在 {secs}s 内完成（超时）"),
        };
    (status, value, start.elapsed())
}

/// 注册一个实例并返回 instance_id
pub async fn register_one(app: &Router, service: &str, address: &str, port: u16) -> String {
    let (status, body) = json_of(
        app,
        post_json(
            &format!("/api/v1/registry/services/{service}/instances"),
            serde_json::json!({"address": address, "port": port, "metadata": {"v": "it"}}),
        ),
    )
    .await;
    assert_eq!(status, 200, "注册失败：{body}");
    body["id"].as_str().unwrap().to_string()
}
