//! HTTP 路由与 handler：只做协议转换，业务在 consumer.rs / lifecycle.rs。

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::routing::get;
use axum::{Json, Router};
use serde_json::{json, Value};

use crate::consumer::Caller;
use crate::error::MeshCallError;
use crate::lifecycle::Lifecycle;

/// 路由共享状态。
#[derive(Clone)]
pub struct AppState {
    pub lifecycle: Arc<Lifecycle>,
    pub caller: Arc<Caller>,
}

impl AppState {
    pub fn new(lifecycle: Arc<Lifecycle>, caller: Arc<Caller>) -> Self {
        Self { lifecycle, caller }
    }
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/api/v1/consumer/greeting/{name}", get(greeting))
        .route("/api/v1/consumer/status", get(status))
        .with_state(state)
}

async fn healthz() -> Json<Value> {
    Json(json!({ "status": "ok", "service": "demo-consumer" }))
}

/// 经 velox-mesh 调用 demo-provider 的问候语接口。
async fn greeting(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<Json<Value>, MeshCallError> {
    let provider = state.caller.fetch_greeting(&name).await?;
    let consumer_instance_id = state.lifecycle.instance_id().map(|id| id.to_string());
    Ok(Json(json!({
        "via": "velox-mesh",
        "consumer_instance_id": consumer_instance_id,
        "provider": {
            "greeting": provider.greeting,
            "name": provider.name,
            "template_source": provider.template_source,
            "instance_id": provider.instance_id,
        },
    })))
}

/// 消费方运行状态：注册情况与 mesh 入口。
async fn status(State(state): State<AppState>) -> Json<Value> {
    let (registered, instance_id) = match state.lifecycle.instance_id() {
        Some(id) => (true, Some(id.to_string())),
        None => (false, None),
    };
    Json(json!({
        "service": "demo-consumer",
        "registered": registered,
        "instance_id": instance_id,
        "mesh_endpoint": state.caller.mesh_endpoint(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    /// 组装指向不可达 mesh 的路由（端口 1 无服务，连接立即失败）。
    fn test_router() -> Router {
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(2))
            .build()
            .expect("构建测试客户端失败");
        let lifecycle = Arc::new(Lifecycle::new(
            crate::config::RegistryConfig::default(),
            http.clone(),
        ));
        let caller = Arc::new(Caller::new("http://127.0.0.1:1", http));
        router(AppState::new(lifecycle, caller))
    }

    async fn get_json(app: Router, path: &str) -> (StatusCode, Value) {
        let resp = app
            .oneshot(
                Request::builder()
                    .uri(path)
                    .body(Body::empty())
                    .expect("构造测试请求失败"),
            )
            .await
            .expect("oneshot 调用失败");
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .expect("读取响应体失败");
        let body: Value = serde_json::from_slice(&bytes).expect("响应体应为合法 JSON");
        (status, body)
    }

    #[tokio::test]
    async fn healthz_returns_ok() {
        let (status, body) = get_json(test_router(), "/healthz").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["status"], "ok");
        assert_eq!(body["service"], "demo-consumer");
    }

    #[tokio::test]
    async fn status_reports_unregistered_defaults() {
        let (status, body) = get_json(test_router(), "/api/v1/consumer/status").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["service"], "demo-consumer");
        assert_eq!(body["registered"], false);
        assert_eq!(body["instance_id"], serde_json::Value::Null);
        assert_eq!(body["mesh_endpoint"], "http://127.0.0.1:1");
    }

    #[tokio::test]
    async fn greeting_maps_mesh_failure_to_502_provider_unreachable() {
        let (status, body) = get_json(test_router(), "/api/v1/consumer/greeting/velox").await;
        assert_eq!(status, StatusCode::BAD_GATEWAY);
        assert_eq!(body["code"], "provider_unreachable");
        assert!(body["message"].as_str().is_some());
    }
}
