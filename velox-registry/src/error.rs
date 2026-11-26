//! 领域错误到 HTTP 响应的映射（统一错误结构见开发规范第 4 节）。

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;

use crate::registry::RegistryError;

/// HTTP 层错误包装：handler 返回 `Result<_, AppError>`。
pub enum AppError {
    /// 既有：404 `service_not_found` / `instance_not_found`，映射不变（FR-11）
    Registry(RegistryError),
    /// 落盘失败：500 + `persist_failed`（Q3/QA-6 定案）。
    /// message 不含内部路径/IO 细节；细节由调用点 `error!` 日志外显（FR-7/NFR-4），
    /// 变体携带的字符串仅用于日志。
    Persist(String),
    /// 400 + `invalid_parameter`。两个来源：watch 参数范围校验（FR-10）
    /// 与 `QueryRejection` 解析失败（仅 watch 新端点捕获映射，G2-2；
    /// 既有六端点错误行为不变）。
    InvalidParam(String),
}

impl From<RegistryError> for AppError {
    fn from(e: RegistryError) -> Self {
        Self::Registry(e)
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        match self {
            AppError::Registry(e) => {
                // message 已含 service/instance 上下文，且不含内部实现细节，可直接外发
                let (status, code) = match &e {
                    RegistryError::InstanceNotFound { .. } => {
                        (StatusCode::NOT_FOUND, "instance_not_found")
                    }
                    RegistryError::ServiceNotFound(_) => {
                        (StatusCode::NOT_FOUND, "service_not_found")
                    }
                };
                (
                    status,
                    Json(json!({ "code": code, "message": e.to_string() })),
                )
                    .into_response()
            }
            AppError::Persist(_) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({
                    "code": "persist_failed",
                    "message": "registry state persist failed"
                })),
            )
                .into_response(),
            AppError::InvalidParam(message) => (
                StatusCode::BAD_REQUEST,
                Json(json!({ "code": "invalid_parameter", "message": message })),
            )
                .into_response(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    /// 消费错误，返回 (状态码, 响应 JSON)
    async fn respond(err: AppError) -> (StatusCode, serde_json::Value) {
        let res = err.into_response();
        let status = res.status();
        let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap();
        (status, serde_json::from_slice(&bytes).unwrap())
    }

    #[tokio::test]
    async fn registry_errors_keep_v01_mapping() {
        let (status, v) = respond(AppError::Registry(RegistryError::ServiceNotFound(
            "nope".into(),
        )))
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(v["code"], "service_not_found");

        let (status, _) = respond(AppError::Registry(RegistryError::InstanceNotFound {
            service: "svc".into(),
            instance: Uuid::new_v4().to_string(),
        }))
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn persist_error_maps_500_without_internal_detail() {
        let (status, v) = respond(AppError::Persist("io detail /internal/path".to_string())).await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(v["code"], "persist_failed");
        assert_eq!(v["message"], "registry state persist failed");
        let msg = v["message"].as_str().unwrap();
        assert!(!msg.contains("/internal/path"), "响应不泄露内部细节");
    }

    #[tokio::test]
    async fn invalid_param_maps_400() {
        let (status, v) = respond(AppError::InvalidParam(
            "timeout_secs=0（要求 1..=300）".to_string(),
        ))
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(v["code"], "invalid_parameter");
        assert!(!v["message"].as_str().unwrap().is_empty());
    }
}
