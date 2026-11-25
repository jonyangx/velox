//! 领域错误到 HTTP 响应的映射（统一错误结构见开发规范第 4 节）。

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;

use crate::registry::RegistryError;

/// HTTP 层错误包装：handler 返回 `Result<_, AppError>`。
pub struct AppError(pub RegistryError);

impl From<RegistryError> for AppError {
    fn from(e: RegistryError) -> Self {
        Self(e)
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        // message 已含 service/instance 上下文，且不含内部实现细节，可直接外发
        let (status, code) = match &self.0 {
            RegistryError::InstanceNotFound { .. } => (StatusCode::NOT_FOUND, "instance_not_found"),
            RegistryError::ServiceNotFound(_) => (StatusCode::NOT_FOUND, "service_not_found"),
        };
        (
            status,
            Json(json!({ "code": code, "message": self.0.to_string() })),
        )
            .into_response()
    }
}
