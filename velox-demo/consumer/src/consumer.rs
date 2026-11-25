//! 领域逻辑：经 velox-mesh 代理调用 demo-provider。
//!
//! 出口流量统一走 mesh（`{mesh_endpoint}/proxy/demo-provider/...`），
//! 由 mesh 负责服务发现、负载均衡与故障转移；本模块只负责协议与解析。

use crate::error::MeshCallError;

/// demo-provider 问候语响应（与 provider 的 `/api/v1/greeting/{name}` 对齐）。
#[derive(Debug, Clone, serde::Deserialize)]
pub struct ProviderGreeting {
    pub greeting: String,
    pub name: String,
    pub template_source: String,
    pub instance_id: Option<String>,
}

/// mesh 调用器：持有 mesh 入口地址与共享 HTTP 客户端。
pub struct Caller {
    mesh_endpoint: String,
    http: reqwest::Client,
}

impl Caller {
    pub fn new(mesh_endpoint: impl Into<String>, http: reqwest::Client) -> Self {
        Self {
            mesh_endpoint: mesh_endpoint.into(),
            http,
        }
    }

    pub fn mesh_endpoint(&self) -> &str {
        &self.mesh_endpoint
    }

    /// 经 mesh 调用 demo-provider 的问候语接口。
    ///
    /// 请求超时由共享客户端统一施加（见 main.rs 装配，`mesh.request_timeout_secs`）；
    /// 非 2xx / 网络失败 / 响应体非法分别映射为 MeshCallError 的不同变体。
    pub async fn fetch_greeting(&self, name: &str) -> Result<ProviderGreeting, MeshCallError> {
        let url = format!(
            "{}/proxy/demo-provider/api/v1/greeting/{}",
            self.mesh_endpoint, name
        );
        let resp =
            self.http
                .get(&url)
                .send()
                .await
                .map_err(|source| MeshCallError::MeshUnreachable {
                    endpoint: self.mesh_endpoint.clone(),
                    source,
                })?;
        if !resp.status().is_success() {
            return Err(MeshCallError::ProviderUnreachable {
                url: url.clone(),
                status: resp.status().as_u16(),
            });
        }
        resp.json::<ProviderGreeting>()
            .await
            .map_err(|source| MeshCallError::InvalidBody { url, source })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::extract::Path;
    use axum::routing::get;
    use axum::{Json, Router};
    use serde_json::{json, Value};

    /// 在 127.0.0.1 随机端口起一个"假 provider 经 mesh 代理"的服务，
    /// 返回与 demo-provider 契约一致的 canned JSON。
    /// 临时端口（bind 0 由内核分配）不违反"测试不占真实端口"规范。
    async fn spawn_canned_mesh(status: u16) -> String {
        async fn canned(Path(name): Path<String>) -> Json<Value> {
            Json(json!({
                "greeting": format!("hello, {name}! (from config center)"),
                "name": name,
                "template_source": "config-center",
                "instance_id": "0198abcd-0000-7000-8000-000000000001",
            }))
        }
        let err = || async {
            (
                axum::http::StatusCode::NOT_FOUND,
                Json(json!({ "code": "instance_not_found" })),
            )
        };
        let app = if status == 200 {
            Router::new().route("/proxy/demo-provider/api/v1/greeting/{name}", get(canned))
        } else {
            Router::new().route("/proxy/demo-provider/api/v1/greeting/{name}", get(err))
        };
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("绑定临时端口失败");
        let addr = listener.local_addr().expect("读取临时端口失败");
        tokio::spawn(async move {
            axum::serve(listener, app).await.expect("测试服务退出异常");
        });
        format!("http://{addr}")
    }

    fn test_caller(endpoint: String) -> Caller {
        Caller::new(
            endpoint,
            reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(2))
                .build()
                .expect("构建测试客户端失败"),
        )
    }

    #[tokio::test]
    async fn fetch_greeting_parses_provider_response_via_loopback() {
        let endpoint = spawn_canned_mesh(200).await;
        let caller = test_caller(endpoint);
        let g = caller
            .fetch_greeting("velox")
            .await
            .expect("回环调用应成功");
        assert_eq!(g.greeting, "hello, velox! (from config center)");
        assert_eq!(g.name, "velox");
        assert_eq!(g.template_source, "config-center");
        assert_eq!(
            g.instance_id,
            Some("0198abcd-0000-7000-8000-000000000001".to_string())
        );
    }

    #[tokio::test]
    async fn non_2xx_maps_to_provider_unreachable() {
        let endpoint = spawn_canned_mesh(404).await;
        let caller = test_caller(endpoint);
        let err = caller
            .fetch_greeting("velox")
            .await
            .expect_err("非 2xx 应报错");
        assert!(matches!(
            err,
            MeshCallError::ProviderUnreachable { status: 404, .. }
        ));
    }

    #[tokio::test]
    async fn unreachable_mesh_maps_to_mesh_unreachable() {
        // 端口 1 上没有服务，连接必然失败且立即返回
        let caller = test_caller("http://127.0.0.1:1".to_string());
        let err = caller
            .fetch_greeting("velox")
            .await
            .expect_err("不可达 mesh 应报错");
        assert!(matches!(err, MeshCallError::MeshUnreachable { .. }));
    }

    #[tokio::test]
    async fn invalid_body_maps_to_invalid_body() {
        // mesh 在但返回的 JSON 缺字段 → 反序列化失败
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("绑定临时端口失败");
        let addr = listener.local_addr().expect("读取临时端口失败");
        let app: Router = Router::new().route(
            "/proxy/demo-provider/api/v1/greeting/{name}",
            get(|| async { Json(json!({ "unexpected": true })) }),
        );
        tokio::spawn(async move {
            axum::serve(listener, app).await.expect("测试服务退出异常");
        });
        let caller = test_caller(format!("http://{addr}"));
        let err = caller
            .fetch_greeting("velox")
            .await
            .expect_err("缺字段响应应报错");
        assert!(matches!(err, MeshCallError::InvalidBody { .. }));
    }
}
