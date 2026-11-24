//! HTTP 层：协议转换，不含业务逻辑（见开发规范第 2 节）。

use std::collections::BTreeMap;
use std::sync::Arc;

use axum::extract::{Path, State};
use axum::routing::get;
use axum::{Json, Router};
use serde_json::{json, Value};

use crate::error::AppError;
use crate::store::Store;

pub fn router(store: Arc<Store>) -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .route(
            "/api/v1/config/{namespace}/{key}",
            get(get_item).put(put_item).delete(delete_item),
        )
        .route("/api/v1/config/{namespace}", get(list_namespace))
        .route("/api/v1/config", get(list_namespaces))
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
