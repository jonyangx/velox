//! HTTP 层：协议转换，不含业务逻辑（见开发规范第 2 节）。

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::routing::{delete, get, post, put};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::json;
use uuid::Uuid;

use crate::error::AppError;
use crate::registry::{InstanceView, Registry};

pub fn router(registry: Arc<Registry>) -> Router {
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
        .with_state(registry)
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
    State(registry): State<Arc<Registry>>,
    Path(service): Path<String>,
    Json(req): Json<RegisterRequest>,
) -> Json<InstanceView> {
    Json(registry.register(&service, req.address, req.port, req.metadata))
}

#[derive(Debug, Default, Deserialize)]
struct ListQuery {
    /// `?healthy=true` 仅返回 TTL 内有心跳的实例（mesh 快照拉取使用）
    healthy: Option<bool>,
}

async fn list_instances(
    State(registry): State<Arc<Registry>>,
    Path(service): Path<String>,
    Query(q): Query<ListQuery>,
) -> Result<Json<Vec<InstanceView>>, AppError> {
    let views = registry
        .list_instances(&service, q.healthy.unwrap_or(false))
        .map_err(AppError)?;
    Ok(Json(views))
}

async fn heartbeat(
    State(registry): State<Arc<Registry>>,
    Path((service, id)): Path<(String, Uuid)>,
) -> Result<Json<serde_json::Value>, AppError> {
    registry.heartbeat(&service, id).map_err(AppError)?;
    Ok(Json(json!({ "ok": true })))
}

async fn deregister(
    State(registry): State<Arc<Registry>>,
    Path((service, id)): Path<(String, Uuid)>,
) -> Result<Json<serde_json::Value>, AppError> {
    registry.deregister(&service, id).map_err(AppError)?;
    Ok(Json(json!({ "ok": true })))
}

async fn list_services(State(registry): State<Arc<Registry>>) -> Json<Vec<String>> {
    Json(registry.list_services())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::Registry;
    use std::time::Duration;
    use tower::ServiceExt; // oneshot

    fn serve() -> Router {
        router(Arc::new(Registry::new(Duration::from_secs(30))))
    }

    #[tokio::test]
    async fn register_and_query_flow() {
        let app = serve();
        let body = serde_json::json!({ "address": "10.0.0.1", "port": 9000 });
        let res = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/api/v1/registry/services/svc-a/instances")
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), 200);
        let registered: serde_json::Value = serde_json::from_slice(
            &axum::body::to_bytes(res.into_body(), usize::MAX)
                .await
                .unwrap(),
        )
        .unwrap();
        let id = registered["id"].as_str().unwrap().to_string();

        // 实例列表
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
}
