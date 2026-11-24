//! 领域错误类型（thiserror）：注册生命周期与配置拉取。
//!
//! 这些错误发生在后台任务中，由任务记录日志并重试，不进入 HTTP 响应路径。

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

/// 与 velox-config 交互失败的领域错误。
#[derive(Debug, Error)]
pub enum ConfigFetchError {
    #[error("config center {endpoint} 不可达：{source}")]
    Unreachable {
        endpoint: String,
        #[source]
        source: reqwest::Error,
    },
    #[error("config center 返回异常响应（{url}）：status {status}")]
    Rejected { url: String, status: u16 },
    #[error("config center 响应体解析失败（{url}）：{source}")]
    InvalidBody {
        url: String,
        #[source]
        source: reqwest::Error,
    },
}
