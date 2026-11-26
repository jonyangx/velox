//! HTTP 层：协议转换，不含业务逻辑（见开发规范第 2 节）。
//!
//! v0.2 新增：`GET /api/v1/registry/watch` 长轮询端点（FR-8，缺省 index 分支 +
//! 参数错误统一映射，架构 §4.7/G2-2）；`register`/`deregister` 挂接写穿落盘
//! （失败 500 `persist_failed` + 置回 dirty 供周期重试，Q3/G2-1）。
//! v0.1 既有六端点的路径、方法、JSON 结构、状态码与错误 `code` 全部不变（FR-11）。

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::Duration;

use axum::extract::rejection::QueryRejection;
use axum::extract::{Path, Query, State};
use axum::routing::{delete, get, post, put};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use crate::error::AppError;
use crate::registry::{InstanceView, Registry};
use crate::store::SharedStore;

/// watch 超时缺省值（FR-10/Q5 定案：缺省精确 30s）
pub const WATCH_TIMEOUT_DEFAULT_SECS: u64 = 30;
/// watch 超时上限（0 与 >300 → 400，AC-11）
pub const WATCH_TIMEOUT_MAX_SECS: u64 = 300;

/// 应用状态：领域注册表 + 可选快照存储（`enabled=false` 时为 None）+ 关停广播。
pub struct AppState {
    pub registry: Arc<Registry>,
    pub store: Option<SharedStore>,
    /// 复用 main 的关停广播：优雅关停时唤醒挂起中的 watch 请求（AC-9/R1）
    pub shutdown: tokio::sync::watch::Receiver<bool>,
}

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/api/v1/registry/services", get(list_services))
        .route(
            "/api/v1/registry/services/{name}/instances",
            post(register).get(list_instances),
        )
        .route(
            "/api/v1/registry/services/{name}/instances/{id}/heartbeat",
            put(heartbeat),
        )
        .route(
            "/api/v1/registry/services/{name}/instances/{id}",
            delete(deregister),
        )
        // v0.2 新增：长轮询订阅（追加，不动既有路由）
        .route("/api/v1/registry/watch", get(watch))
        .with_state(state)
}

/// 结构变更（注册/注销）后的写穿挂接（§4.7）：store 为 None（`enabled=false`）时
/// 直接 Ok；失败 → `registry.mark_dirty()`（供周期任务重试，G2-1/TC-16）+
/// ERROR 日志 + `AppError::Persist`（500，响应不含内部细节）。
async fn persist_after_change(state: &AppState) -> Result<(), AppError> {
    let Some(store) = &state.store else {
        return Ok(());
    };
    let registry = &state.registry;
    if let Err(e) = store.flush(|| registry.snapshot()).await {
        registry.mark_dirty();
        tracing::error!(error = %e, "registry state persist failed");
        return Err(AppError::Persist(e.to_string()));
    }
    Ok(())
}

async fn healthz() -> Json<serde_json::Value> {
    Json(json!({ "status": "ok", "service": "velox-registry" }))
}

#[derive(Debug, Deserialize)]
struct RegisterRequest {
    address: String,
    port: u16,
    #[serde(default)]
    metadata: HashMap<String, String>,
}

async fn register(
    State(state): State<Arc<AppState>>,
    Path(service): Path<String>,
    Json(req): Json<RegisterRequest>,
) -> Result<Json<InstanceView>, AppError> {
    let view = state
        .registry
        .register(&service, req.address, req.port, req.metadata);
    // 注册成功 ⇒ 已落盘（Q3）：响应前 await flush
    persist_after_change(&state).await?;
    Ok(Json(view))
}

#[derive(Debug, Default, Deserialize)]
struct ListQuery {
    /// `?healthy=true` 仅返回 TTL 内有心跳的实例（mesh 快照拉取使用）
    healthy: Option<bool>,
}

async fn list_instances(
    State(state): State<Arc<AppState>>,
    Path(service): Path<String>,
    Query(q): Query<ListQuery>,
) -> Result<Json<Vec<InstanceView>>, AppError> {
    let views = state
        .registry
        .list_instances(&service, q.healthy.unwrap_or(false))
        .map_err(AppError::Registry)?;
    Ok(Json(views))
}

async fn heartbeat(
    State(state): State<Arc<AppState>>,
    Path((service, id)): Path<(String, Uuid)>,
) -> Result<Json<serde_json::Value>, AppError> {
    // 心跳只在领域内置 dirty（FR-2），由周期 flusher 合并刷盘——永不因落盘失败报错
    state
        .registry
        .heartbeat(&service, id)
        .map_err(AppError::Registry)?;
    Ok(Json(json!({ "ok": true })))
}

async fn deregister(
    State(state): State<Arc<AppState>>,
    Path((service, id)): Path<(String, Uuid)>,
) -> Result<Json<serde_json::Value>, AppError> {
    state
        .registry
        .deregister(&service, id)
        .map_err(AppError::Registry)?;
    // 注销成功 ⇒ 已落盘（Q3）：响应前 await flush
    persist_after_change(&state).await?;
    Ok(Json(json!({ "ok": true })))
}

async fn list_services(State(state): State<Arc<AppState>>) -> Json<Vec<String>> {
    Json(state.registry.list_services())
}

// ---- watch 长轮询端点（FR-8，架构 §4.7 定案逻辑） ----

#[derive(Debug, Deserialize)]
struct WatchQuery {
    /// None = 首次快照请求（立即返回）；Some(i) 参与与当前版本的比较
    index: Option<u64>,
    timeout_secs: Option<u64>,
}

#[derive(Debug, Serialize)]
struct WatchResponse {
    index: u64,
    services: BTreeMap<String, Vec<InstanceView>>,
}

/// timeout_secs 解析：缺省精确 30；0 或 >300 非法（返回错误消息含配置项名与实际值）。
/// 独立成纯函数以便单测断言「缺省精确 ==30」（TC-23/AC-11）。
fn resolve_timeout_secs(raw: Option<u64>) -> Result<u64, String> {
    match raw {
        None => Ok(WATCH_TIMEOUT_DEFAULT_SECS),
        Some(0) => Err(format!(
            "timeout_secs=0（要求 1..={WATCH_TIMEOUT_MAX_SECS}）"
        )),
        Some(t) if t > WATCH_TIMEOUT_MAX_SECS => Err(format!(
            "timeout_secs={t}（要求 1..={WATCH_TIMEOUT_MAX_SECS}）"
        )),
        Some(t) => Ok(t),
    }
}

/// 缺省 index（None）= 首次快照请求立即返回；显式 Some(i) 相等才挂起（G2-2/RQ-3）。
fn should_wait_for_change(index: Option<u64>, current_version: u64) -> bool {
    match index {
        None => false,
        Some(i) => i == current_version,
    }
}

/// `Result<Query, QueryRejection>` 签名（G2-2）：解析失败（非数字、负数、u64 溢出、
/// 空值等）最先映射 400 `invalid_parameter`，仅本端点捕获；既有端点错误行为不变。
async fn watch(
    State(state): State<Arc<AppState>>,
    params: Result<Query<WatchQuery>, QueryRejection>,
) -> Result<Json<WatchResponse>, AppError> {
    // 步骤 1：参数解析（先于一切业务分支）
    let Query(q) =
        params.map_err(|rej| AppError::InvalidParam(format!("query 参数解析失败：{rej}")))?;
    // 步骤 2：范围校验（先完整校验所有参数，再进入 index 分支——缺省 index 亦然，TC-22）
    let timeout_secs = resolve_timeout_secs(q.timeout_secs).map_err(AppError::InvalidParam)?;
    // 步骤 3：订阅先于读（杜绝「读后变、通道漏通知」窗口）
    let mut changes = state.registry.subscribe();
    let (version, services) = state.registry.watch_snapshot();
    // 步骤 4：index 分支——缺省（None）立即返回；显式不等（含过旧/过大）立即返回
    if !should_wait_for_change(q.index, version) {
        return Ok(Json(WatchResponse {
            index: version,
            services,
        }));
    }
    // 步骤 5：index == version → select 三路等待
    let watch_index = version; // 挂起基准（与 q.index 相等）
    let deadline = tokio::time::Instant::now() + Duration::from_secs(timeout_secs);
    let mut shutdown = state.shutdown.clone();
    // 关停已发生（如信号早于本请求）→ 立即返回当前快照
    if *shutdown.borrow_and_update() {
        return Ok(Json(WatchResponse {
            index: version,
            services,
        }));
    }
    loop {
        tokio::select! {
            changed = changes.changed() => {
                if changed.is_err() {
                    break; // 发送端消失（防御性）：返回当前快照
                }
                let new_version = *changes.borrow_and_update();
                if new_version != watch_index {
                    break; // 长轮询唤醒（通道即时 notify，满足 AC-8 的 1s 内）
                }
                // 版本未越过 index（理论不发生）：防御性 continue
            }
            changed = shutdown.changed() => {
                // 优雅关停：立即返回当前快照（200），不等超时（AC-9/R1）；
                // 发送端消失（通道关闭）按关停处理，避免忙等
                if changed.is_err() || *shutdown.borrow_and_update() {
                    break;
                }
            }
            _ = tokio::time::sleep_until(deadline) => {
                break; // 超时：返回当前 (version, services)，HTTP 200（FR-8）
            }
        }
    }
    let (version, services) = state.registry.watch_snapshot();
    Ok(Json(WatchResponse {
        index: version,
        services,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::Registry;
    use std::time::Duration;
    use tower::ServiceExt; // oneshot

    fn app() -> Router {
        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        let state = Arc::new(AppState {
            registry: Arc::new(Registry::new(Duration::from_secs(30))),
            store: None,
            shutdown: shutdown_rx,
        });
        std::mem::forget(shutdown_tx); // 保持发送端存活（测试进程生命周期）
        router(state)
    }

    async fn register_instance(
        app: &Router,
        service: &str,
        address: &str,
        port: u16,
    ) -> serde_json::Value {
        let body = serde_json::json!({ "address": address, "port": port });
        let res = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri(format!("/api/v1/registry/services/{service}/instances"))
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), 200);
        serde_json::from_slice(
            &axum::body::to_bytes(res.into_body(), usize::MAX)
                .await
                .unwrap(),
        )
        .unwrap()
    }

    // ---- v0.1 断言原样保留（FR-11/AC-12；setup 适配 AppState，构造签名变更不判 API 不兼容） ----

    #[tokio::test]
    async fn register_and_query_flow() {
        let app = app();
        let registered = register_instance(&app, "svc-a", "10.0.0.1", 9000).await;
        let id = registered["id"].as_str().unwrap().to_string();

        // 实例列表（?healthy=true 过滤）
        let res = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .uri("/api/v1/registry/services/svc-a/instances?healthy=true")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), 200);

        // 未知服务 → 404 + 统一错误结构
        let res = app
            .oneshot(
                axum::http::Request::builder()
                    .uri("/api/v1/registry/services/nope/instances")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), 404);
        let err: serde_json::Value = serde_json::from_slice(
            &axum::body::to_bytes(res.into_body(), usize::MAX)
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(err["code"], "service_not_found");
        let _ = id;
    }

    #[tokio::test]
    async fn heartbeat_and_deregister_ok_shape() {
        let app = app();
        let registered = register_instance(&app, "svc-b", "10.0.0.2", 9001).await;
        let id = registered["id"].as_str().unwrap().to_string();

        let res = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .method("PUT")
                    .uri(format!(
                        "/api/v1/registry/services/svc-b/instances/{id}/heartbeat"
                    ))
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), 200);
        let v: serde_json::Value = serde_json::from_slice(
            &axum::body::to_bytes(res.into_body(), usize::MAX)
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(v["ok"], true);

        // 未知实例心跳 → 404 instance_not_found（v0.1 错误语义不变）
        let res = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .method("PUT")
                    .uri(format!(
                        "/api/v1/registry/services/svc-b/instances/{}/heartbeat",
                        Uuid::new_v4()
                    ))
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), 404);
        let err: serde_json::Value = serde_json::from_slice(
            &axum::body::to_bytes(res.into_body(), usize::MAX)
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(err["code"], "instance_not_found");

        // 注销 → {"ok":true}；服务清空
        let res = app
            .oneshot(
                axum::http::Request::builder()
                    .method("DELETE")
                    .uri(format!("/api/v1/registry/services/svc-b/instances/{id}"))
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), 200);
        let v: serde_json::Value = serde_json::from_slice(
            &axum::body::to_bytes(res.into_body(), usize::MAX)
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(v["ok"], true);
    }

    // ---- watch：timeout 解析与缺省精确值（AC-11/TC-23） ----

    #[test]
    fn timeout_default_is_exactly_30() {
        assert_eq!(resolve_timeout_secs(None).unwrap(), 30);
    }

    #[test]
    fn timeout_range_validation() {
        for ok in [1, 30, 300] {
            assert_eq!(resolve_timeout_secs(Some(ok)).unwrap(), ok);
        }
        for bad in [0, 301, 10_000] {
            let err = resolve_timeout_secs(Some(bad)).unwrap_err();
            assert!(err.contains("timeout_secs"), "实际：{err}");
            assert!(err.contains(&bad.to_string()), "错误含实际值，实际：{err}");
        }
    }

    #[test]
    fn default_index_branch_is_immediate() {
        // 缺省 index：任何当前版本都立即返回（RQ-3 定案）
        assert!(!should_wait_for_change(None, 0));
        assert!(!should_wait_for_change(None, 5));
        // 显式 index：仅相等挂起
        assert!(should_wait_for_change(Some(0), 0));
        assert!(!should_wait_for_change(Some(0), 1));
        assert!(should_wait_for_change(Some(3), 3));
        assert!(!should_wait_for_change(Some(99), 3));
    }

    // ---- watch：解析失败统一映射（G2-2/TC-40，仅新端点） ----

    #[tokio::test]
    async fn watch_rejects_invalid_query_with_invalid_parameter() {
        let app = app();
        // 非数字 / 负数 / u64 溢出 / 空值 —— 全部 400 invalid_parameter
        for uri in [
            "/api/v1/registry/watch?index=abc",
            "/api/v1/registry/watch?index=-1",
            "/api/v1/registry/watch?index=99999999999999999999999",
            "/api/v1/registry/watch?index=",
            "/api/v1/registry/watch?timeout_secs=abc",
            "/api/v1/registry/watch?timeout_secs=",
            // 缺省 index 请求也必须先通过参数校验（TC-22）
            "/api/v1/registry/watch?timeout_secs=abc",
        ] {
            let res = app
                .clone()
                .oneshot(
                    axum::http::Request::builder()
                        .uri(uri)
                        .body(axum::body::Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            let status = res.status();
            let body: serde_json::Value = serde_json::from_slice(
                &axum::body::to_bytes(res.into_body(), usize::MAX)
                    .await
                    .unwrap(),
            )
            .unwrap();
            assert_eq!(status, 400, "{uri}");
            assert_eq!(body["code"], "invalid_parameter", "{uri}");
            assert!(
                !body["message"].as_str().unwrap_or_default().is_empty(),
                "{uri}"
            );
        }
    }

    #[tokio::test]
    async fn watch_range_errors_return_400_even_without_index() {
        let app = app();
        for uri in [
            "/api/v1/registry/watch?timeout_secs=0",
            "/api/v1/registry/watch?timeout_secs=301",
            "/api/v1/registry/watch?index=0&timeout_secs=0",
            "/api/v1/registry/watch?timeout_secs=301&index=0",
        ] {
            let res = app
                .clone()
                .oneshot(
                    axum::http::Request::builder()
                        .uri(uri)
                        .body(axum::body::Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(res.status(), 400, "{uri}");
        }
    }

    // ---- watch：缺省 index = 首次快照立即返回（RQ-3/G2-2 三场景） ----

    #[tokio::test]
    async fn watch_default_index_returns_immediately() {
        // 场景 1：空表 version=0 → 立即 200、index=0、空 map
        let app = app();
        let res = app
            .clone()
            .oneshot(get_req("/api/v1/registry/watch"))
            .await
            .unwrap();
        assert_eq!(res.status(), 200);
        let v: serde_json::Value = serde_json::from_slice(
            &axum::body::to_bytes(res.into_body(), usize::MAX)
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(v["index"], 0);
        assert_eq!(v["services"], serde_json::json!({}));

        // 场景 3：version>0 → 立即返回当前版本与实例
        register_instance(&app, "svc", "10.0.0.1", 9000).await;
        let res = app
            .oneshot(get_req("/api/v1/registry/watch"))
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_slice(
            &axum::body::to_bytes(res.into_body(), usize::MAX)
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(v["index"], 1);
        assert_eq!(v["services"]["svc"].as_array().unwrap().len(), 1);
        // 场景 2（恢复有实例 version=0）在 tests/watch.rs 以 restored 构造覆盖
    }

    #[tokio::test]
    async fn watch_stale_or_oversized_index_returns_immediately() {
        let app = app();
        register_instance(&app, "svc", "10.0.0.1", 9000).await; // version=1
                                                                // index 落后 → 立即返回（AC-8b）
        let v = watch_once(&app, "/api/v1/registry/watch?index=0&timeout_secs=5").await;
        assert_eq!(v["index"], 1);
        // index 过大（B-7）→ 立即返回并以响应内新 index 纠正客户端
        let v = watch_once(&app, "/api/v1/registry/watch?index=99&timeout_secs=5").await;
        assert_eq!(v["index"], 1);
    }

    fn get_req(uri: &str) -> axum::http::Request<axum::body::Body> {
        axum::http::Request::builder()
            .uri(uri)
            .body(axum::body::Body::empty())
            .unwrap()
    }

    async fn watch_once(app: &Router, uri: &str) -> serde_json::Value {
        let res = app.clone().oneshot(get_req(uri)).await.unwrap();
        assert_eq!(res.status(), 200);
        serde_json::from_slice(
            &axum::body::to_bytes(res.into_body(), usize::MAX)
                .await
                .unwrap(),
        )
        .unwrap()
    }
}
