//! HTTP 层：路由与 handler，只做协议转换，不含业务逻辑（见开发规范第 2 节）。
//!
//! 契约字段钉死，勿改：
//! - `GET /healthz` → `{"status":"ok","service":"demo-provider"}`
//! - `GET /api/v1/greeting/{name}` → `{"greeting","name","template_source","instance_id"}`
//! - `GET /api/v1/provider/status` → `{"service","registered","instance_id","template","template_source"}`
//!
//! `template_source` 取值仅 `"builtin" | "config_center"`。

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::routing::get;
use axum::{Json, Router};
use serde::Serialize;
use serde_json::json;

use crate::greeting::Greeting;
use crate::lifecycle::Lifecycle;

/// handler 共享状态：领域对象的只读引用（写入仅发生在后台刷新/注册任务内）。
#[derive(Clone)]
pub struct AppState {
    greeting: Arc<Greeting>,
    lifecycle: Arc<Lifecycle>,
}

pub fn router(greeting: Arc<Greeting>, lifecycle: Arc<Lifecycle>) -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/api/v1/greeting/{name}", get(greet))
        .route("/api/v1/provider/status", get(provider_status))
        .with_state(AppState {
            greeting,
            lifecycle,
        })
}

async fn healthz() -> Json<serde_json::Value> {
    Json(json!({ "status": "ok", "service": "demo-provider" }))
}

#[derive(Debug, Serialize)]
struct GreetingResponse {
    greeting: String,
    name: String,
    template_source: &'static str,
    /// 注册成功后的实例 id（LB 去重断言用）；未注册时为 null
    instance_id: Option<String>,
}

async fn greet(Path(name): Path<String>, State(state): State<AppState>) -> Json<GreetingResponse> {
    // render / instance_id 均为同步读锁调用，handler 内无 await，不跨 await 持锁（开发规范第 3.3 节）
    Json(GreetingResponse {
        greeting: state.greeting.render(&name),
        name,
        template_source: template_source(&state.greeting),
        instance_id: state.lifecycle.instance_id().map(|id| id.to_string()),
    })
}

#[derive(Debug, Serialize)]
struct StatusResponse {
    service: &'static str,
    registered: bool,
    /// 注册成功后的实例 id；未注册时为 null
    instance_id: Option<String>,
    template: String,
    template_source: &'static str,
}

async fn provider_status(State(state): State<AppState>) -> Json<StatusResponse> {
    // instance_id / template 均为同步读锁调用，handler 内无 await
    let instance_id = state.lifecycle.instance_id();
    Json(StatusResponse {
        service: "demo-provider",
        registered: instance_id.is_some(),
        instance_id: instance_id.map(|id| id.to_string()),
        template: state.greeting.template(),
        template_source: template_source(&state.greeting),
    })
}

/// 当前模板来源的稳定标识（契约字段）。
fn template_source(greeting: &Greeting) -> &'static str {
    if greeting.is_from_config_center() {
        "config_center"
    } else {
        "builtin"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::RegistryConfig;
    use crate::greeting::DEFAULT_TEMPLATE;
    use tower::ServiceExt; // oneshot

    /// 未注册状态下的最小 Router：不触发任何网络请求。
    fn app() -> Router {
        let lifecycle = Arc::new(Lifecycle::new(
            RegistryConfig::default(),
            reqwest::Client::new(),
        ));
        router(Arc::new(Greeting::new()), lifecycle)
    }

    async fn get_json(uri: &str) -> (axum::http::StatusCode, serde_json::Value) {
        let res = app()
            .oneshot(
                axum::http::Request::builder()
                    .uri(uri)
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = res.status();
        let body = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap();
        let value = serde_json::from_slice(&body).unwrap();
        (status, value)
    }

    #[tokio::test]
    async fn healthz_reports_ok() {
        let (status, body) = get_json("/healthz").await;
        assert_eq!(status, axum::http::StatusCode::OK);
        assert_eq!(
            body,
            serde_json::json!({ "status": "ok", "service": "demo-provider" })
        );
    }

    #[tokio::test]
    async fn greeting_uses_builtin_template_by_default() {
        let (status, body) = get_json("/api/v1/greeting/velox").await;
        assert_eq!(status, axum::http::StatusCode::OK);
        assert_eq!(body["name"], "velox");
        assert_eq!(
            body["greeting"],
            DEFAULT_TEMPLATE.replace("{name}", "velox")
        );
        assert_eq!(body["template_source"], "builtin");
        // 测试内 Lifecycle 未注册，instance_id 必须存在且为 null
        assert_eq!(body["instance_id"], serde_json::Value::Null);
    }

    #[tokio::test]
    async fn status_reports_unregistered_instance() {
        let (status, body) = get_json("/api/v1/provider/status").await;
        assert_eq!(status, axum::http::StatusCode::OK);
        assert_eq!(body["service"], "demo-provider");
        assert_eq!(body["registered"], false);
        assert_eq!(body["instance_id"], serde_json::Value::Null);
        assert_eq!(body["template"], DEFAULT_TEMPLATE);
        assert_eq!(body["template_source"], "builtin");
    }
}
