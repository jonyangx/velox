//! 领域错误到 HTTP 响应的映射（统一错误结构见开发规范第 4 节）。

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;

use crate::store::StoreError;

/// HTTP 层错误包装：handler 返回 `Result<_, AppError>`。
pub struct AppError(pub StoreError);

impl From<StoreError> for AppError {
    fn from(e: StoreError) -> Self {
        Self(e)
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, code) = match &self.0 {
            StoreError::InvalidNamespace(_) => (StatusCode::BAD_REQUEST, "invalid_namespace"),
            StoreError::NotFound { .. } => (StatusCode::NOT_FOUND, "config_not_found"),
            // 存储层内部故障：对外收敛为统一 message，细节留在日志
            StoreError::Io(_) | StoreError::Json(_) => {
                tracing::error!(error = %self.0, "config store internal error");
                (StatusCode::INTERNAL_SERVER_ERROR, "storage_internal_error")
            }
        };
        (
            status,
            Json(json!({ "code": code, "message": self.0.to_string() })),
        )
            .into_response()
    }
}
