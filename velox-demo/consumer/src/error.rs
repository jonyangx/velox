//! 领域错误类型（thiserror）：注册生命周期与经 velox-mesh 调用上游。
//!
//! LifecycleError 发生在后台任务中，由任务记录日志并重试，不进入 HTTP 响应路径；
//! MeshCallError 是请求处理路径的错误，实现 IntoResponse 统一映射为 502。

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;
use thiserror::Error;

/// 与 velox-registry 交互失败的领域错误。
#[derive(Debug, Error)]
pub enum LifecycleError {
    #[error("registry {endpoint} 不可达：{source}")]
    RegistryUnreachable {
        endpoint: String,
        #[source]
        source: reqwest::Error,
    },
    #[error("registry 返回异常响应（{url}）：status {status}")]
    RegistryRejected { url: String, status: u16 },
}

/// 经 velox-mesh 调用 demo-provider 失败的领域错误。
///
/// 无论 mesh 不可达、上游异常还是响应体非法，对外统一呈现为
/// 502 + `provider_unreachable`：调用方只需知道"这次上游调用没成"。
#[derive(Debug, Error)]
pub enum MeshCallError {
    #[error("velox-mesh {endpoint} 不可达：{source}")]
    MeshUnreachable {
        endpoint: String,
        #[source]
        source: reqwest::Error,
    },
    #[error("经 velox-mesh 调用 demo-provider 失败（{url}）：status {status}")]
    ProviderUnreachable { url: String, status: u16 },
    #[error("上游响应体解析失败（{url}）：{source}")]
    InvalidBody {
        url: String,
        #[source]
        source: reqwest::Error,
    },
}

impl IntoResponse for MeshCallError {
    fn into_response(self) -> Response {
        let message = match &self {
            MeshCallError::MeshUnreachable { .. } => {
                "velox-mesh 入口不可达，请检查 mesh 是否运行及 mesh.endpoint 配置".to_string()
            }
            MeshCallError::ProviderUnreachable { status, .. } => {
                format!("经 velox-mesh 调用 demo-provider 失败：上游返回 status {status}，请检查 demo-provider 是否健康")
            }
            MeshCallError::InvalidBody { .. } => {
                "demo-provider 响应体解析失败，请检查上游版本兼容性".to_string()
            }
        };
        tracing::warn!(error = %self, "mesh 调用失败，返回 502");
        (
            StatusCode::BAD_GATEWAY,
            Json(json!({ "code": "provider_unreachable", "message": message })),
        )
            .into_response()
    }
}
