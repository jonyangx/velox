//! HTTP 层：协议转换，不含业务逻辑（见开发规范第 2 节）。

use std::collections::BTreeMap;
use std::sync::Arc;

use axum::extract::{Path, State};
use axum::middleware;
use axum::routing::get;
use axum::{Json, Router};
use serde_json::{json, Value};

use crate::error::AppError;
use crate::limit::{self, ConcurrencyLimit};
use crate::store::Store;

/// 仅给业务路由挂限流层。`route_layer` 语义：路由未命中（404/405）不执行中间件，
/// 不占名额——与 FR-2“覆盖业务路由”口径一致。healthz 豁免（Q1）由结构保证：
/// 它注册在该层之外。
pub(crate) fn limit_routes<S>(limit: ConcurrencyLimit, routes: Router<S>) -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    routes.route_layer(middleware::from_fn_with_state(limit, limit::guard))
}

/// 签名变更：新增上限参数。仅 main.rs 调用，无外部消费者，非破坏性。
pub fn router(store: Arc<Store>, max_concurrent_requests: usize) -> Router {
    let limit = ConcurrencyLimit::new(max_concurrent_requests);
    limit_routes(
        limit,
        Router::new()
            .route(
                "/api/v1/config/{namespace}/{key}",
                get(get_item).put(put_item).delete(delete_item),
            )
            .route("/api/v1/config/{namespace}", get(list_namespace))
            .route("/api/v1/config", get(list_namespaces)),
    )
    .route("/healthz", get(healthz)) // 豁免限流（Q1 默认方案）
    .with_state(store)
}

async fn healthz() -> Json<Value> {
    Json(json!({ "status": "ok", "service": "velox-config" }))
}

async fn get_item(
    State(store): State<Arc<Store>>,
    Path((namespace, key)): Path<(String, String)>,
) -> Result<Json<Value>, AppError> {
    Ok(Json(store.get(&namespace, &key)?))
}

async fn put_item(
    State(store): State<Arc<Store>>,
    Path((namespace, key)): Path<(String, String)>,
    Json(value): Json<Value>,
) -> Result<Json<Value>, AppError> {
    store.put(&namespace, &key, value).await?;
    Ok(Json(json!({ "ok": true })))
}

async fn delete_item(
    State(store): State<Arc<Store>>,
    Path((namespace, key)): Path<(String, String)>,
) -> Result<Json<Value>, AppError> {
    let removed = store.delete(&namespace, &key).await?;
    if removed {
        Ok(Json(json!({ "ok": true })))
    } else {
        // 与 get 语义对齐：删除不存在的项报 404
        Err(AppError(crate::store::StoreError::NotFound {
            namespace,
            key,
        }))
    }
}

async fn list_namespace(
    State(store): State<Arc<Store>>,
    Path(namespace): Path<String>,
) -> Result<Json<BTreeMap<String, Value>>, AppError> {
    let items = store.list(&namespace).ok_or_else(|| {
        AppError(crate::store::StoreError::NotFound {
            namespace: namespace.clone(),
            key: "*".to_string(),
        })
    })?;
    Ok(Json(items))
}

async fn list_namespaces(State(store): State<Arc<Store>>) -> Json<Vec<String>> {
    Json(store.list_namespaces())
}

