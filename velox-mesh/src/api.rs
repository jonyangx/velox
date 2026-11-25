//! HTTP 层：inbound 数据面 + admin 管理面（协议转换，不含业务逻辑）。

use std::sync::Arc;

use axum::extract::State;
use axum::routing::{any, get};
use axum::{Json, Router};
use serde_json::json;

use crate::proxy;
use crate::MeshState;

/// 数据面路由：业务容器的出口流量入口。
pub fn inbound_router(state: Arc<MeshState>) -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/proxy/{service}", any(proxy::proxy_root))
        .route("/proxy/{service}/{*rest}", any(proxy::proxy_rest))
        .with_state(state)
}

/// 管理面路由：运维查询，仅绑定本机地址。
pub fn admin_router(state: Arc<MeshState>) -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/api/v1/mesh/routes", get(routes))
        .route("/api/v1/mesh/status", get(status))
        .with_state(state)
}

async fn healthz() -> Json<serde_json::Value> {
    Json(json!({ "status": "ok", "service": "velox-mesh" }))
}

async fn routes(State(state): State<Arc<MeshState>>) -> Json<serde_json::Value> {
    let snapshot = state.discovery.snapshot();
    Json(json!({
        "snapshot_age_secs": snapshot.age_secs(),
        "services": snapshot.services,
    }))
}

async fn status(State(state): State<Arc<MeshState>>) -> Json<serde_json::Value> {
    let snapshot = state.discovery.snapshot();
    Json(json!({
        "version": env!("CARGO_PKG_VERSION"),
        "uptime_secs": state.started_at.elapsed().as_secs(),
        "registry_endpoint": state.discovery.endpoint(),
        "snapshot_age_secs": snapshot.age_secs(),
        "service_count": snapshot.service_count(),
        "instance_count": snapshot.instance_count(),
    }))
}
