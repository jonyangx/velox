//! 反向代理：`/proxy/{service}[/...]` → registry 健康实例（轮询负载均衡）。

use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::{Path, RawQuery, State};
use axum::http::{HeaderMap, HeaderName, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;

use crate::MeshState;

#[derive(Debug, thiserror::Error)]
pub enum ProxyError {
    #[error("no healthy instance for service '{0}'")]
    NoInstance(String),
    #[error("upstream request failed: {0}")]
    Upstream(String),
}

impl IntoResponse for ProxyError {
    fn into_response(self) -> Response {
        let (status, code) = match &self {
            ProxyError::NoInstance(_) => (StatusCode::NOT_FOUND, "no_healthy_instance"),
            ProxyError::Upstream(_) => (StatusCode::BAD_GATEWAY, "upstream_error"),
        };
        (
            status,
            Json(json!({ "code": code, "message": self.to_string() })),
        )
            .into_response()
    }
}

/// hop-by-hop 头不应转发（RFC 9110/7230 §6.1）。
fn is_hop_by_hop(name: &HeaderName) -> bool {
    matches!(
        name.as_str(),
        "connection"
            | "keep-alive"
            | "proxy-authenticate"
            | "proxy-authorization"
            | "te"
            | "trailer"
            | "transfer-encoding"
            | "upgrade"
    )
}

/// 入口：`/proxy/{service}`（无尾路径）。
#[allow(clippy::too_many_arguments)]
pub async fn proxy_root(
    State(state): State<Arc<MeshState>>,
    Path(service): Path<String>,
    method: Method,
    headers: HeaderMap,
    RawQuery(query): RawQuery,
    body: Bytes,
) -> Result<Response, ProxyError> {
    forward(state, service, String::new(), method, headers, query, body).await
}

/// 入口：`/proxy/{service}/{*rest}`（带尾路径）。
#[allow(clippy::too_many_arguments)]
pub async fn proxy_rest(
    State(state): State<Arc<MeshState>>,
    Path((service, rest)): Path<(String, String)>,
    method: Method,
    headers: HeaderMap,
    RawQuery(query): RawQuery,
    body: Bytes,
) -> Result<Response, ProxyError> {
    forward(state, service, rest, method, headers, query, body).await
}

#[allow(clippy::too_many_arguments)]
async fn forward(
    state: Arc<MeshState>,
    service: String,
    rest: String,
    method: Method,
    headers: HeaderMap,
    query: Option<String>,
    body: Bytes,
) -> Result<Response, ProxyError> {
    let target = state
        .discovery
        .pick(&service)
        .ok_or_else(|| ProxyError::NoInstance(service.clone()))?;

    // 兼容不同 wildcard 行为：保证 rest 以 '/' 拼接
    let path = if rest.is_empty() {
        String::new()
    } else if let Some(stripped) = rest.strip_prefix('/') {
        format!("/{stripped}")
    } else {
        format!("/{rest}")
    };
    let mut url = format!("{}{}", target.base_url(), path);
    if let Some(q) = query {
        url.push('?');
        url.push_str(&q);
    }

    let mut upstream = state.http.request(method, &url);
    for (name, value) in &headers {
        if is_hop_by_hop(name) {
            continue;
        }
        upstream = upstream.header(name, value);
    }

    tracing::debug!(service = %service, target = %target.base_url(), "proxying request");
    let upstream_resp = upstream.body(body).send().await.map_err(|e| {
        tracing::warn!(
            service = %service,
            target = %target.base_url(),
            error = %e,
            "upstream request failed"
        );
        ProxyError::Upstream(e.to_string())
    })?;

    let status = upstream_resp.status();
    let mut builder = Response::builder().status(status);
    for (name, value) in upstream_resp.headers() {
        if is_hop_by_hop(name) {
            continue;
        }
        builder = builder.header(name, value);
    }
    let bytes = upstream_resp
        .bytes()
        .await
        .map_err(|e| ProxyError::Upstream(e.to_string()))?;
    builder
        .body(axum::body::Body::from(bytes))
        .map_err(|e| ProxyError::Upstream(e.to_string()))
}
