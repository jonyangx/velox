//! velox-config HTTP 契约集成测试。
//!
//! 本测试起真实二进制打真实 HTTP：spawn `velox-config` 子进程 + reqwest 客户端，
//! 验证存储层从「内存 + 写穿 JSON 文件」替换为「SQLite KV + TTL 读缓存」后，
//! 对外 HTTP 契约完全不变。
//!
//! 契约来源：
//! - `src/api.rs`：路由与成功响应格式（`{"ok": true}` / JSON 值原样返回 / 列表）；
//! - `src/error.rs`：错误响应统一为 `{"code": "...", "message": "..."}`；
//! - `velox-mono/docs/subsystems.md`：velox-config API 概览。
//!
//! 依赖说明：velox-config 是 binary crate（无 lib 目标），本测试不引用其内部模块，
//! 只使用普通依赖（tokio / serde_json）与 dev-dependencies（reqwest / uuid）。

use std::net::TcpListener;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use reqwest::{Client, Method, StatusCode};
use serde_json::{json, Value};
use uuid::Uuid;

/// 健康检查总超时与轮询间隔。
const HEALTH_TIMEOUT: Duration = Duration::from_secs(10);
const HEALTH_POLL_INTERVAL: Duration = Duration::from_millis(50);
/// HTTP 客户端单请求超时。
const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
/// 端口竞态导致的启动失败重试次数（每次换新端口）。
const START_ATTEMPTS: usize = 3;

/// 子进程守卫：Drop 时 kill 子进程并清理临时目录，
/// 保证测试结束（含 panic 路径）时子进程与临时文件被回收。
struct ServerGuard {
    child: Child,
    temp_dir: PathBuf,
}

impl Drop for ServerGuard {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.temp_dir);
    }
}

/// 占一个临时端口：bind(:0) 取端口号后立刻释放。
/// 存在极小竞态窗口（释放后、服务 bind 前被他人抢占），由外层重试兜底。
fn pick_free_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind 127.0.0.1:0 失败");
    listener.local_addr().expect("获取 local_addr 失败").port()
}

/// 起真实服务并等健康检查通过。
///
/// 命令行第一个参数为 TOML 配置文件路径（见 `src/main.rs` / `src/config.rs`）。
/// 配置里带上 `[cache]` 段：这是存储层新配置（TTL 读缓存）；旧版 Config
/// 未定义该字段时 serde 会忽略未知字段，不会导致解析失败。
async fn try_start_server(client: &Client, port: u16) -> Result<(String, ServerGuard), String> {
    let temp_dir = std::env::temp_dir().join(format!("velox-config-e2e-{}", Uuid::new_v4()));
    std::fs::create_dir_all(&temp_dir)
        .map_err(|e| format!("创建临时目录 {} 失败：{e}", temp_dir.display()))?;

    let data_dir = temp_dir.join("data");
    let config_path = temp_dir.join("config.toml");
    let config_toml = format!(
        "[server]\naddr = \"127.0.0.1:{port}\"\n\n[storage]\ndata_dir = \"{}\"\n\n[cache]\nttl_secs = 300\n",
        data_dir.display(),
    );
    std::fs::write(&config_path, &config_toml)
        .map_err(|e| format!("写配置文件 {} 失败：{e}", config_path.display()))?;

    let bin = env!("CARGO_BIN_EXE_velox-config");
    let child = Command::new(bin)
        .arg(&config_path)
        // 环境变量覆盖（VELOX_CONFIG__*）会盖掉配置文件取值，显式清掉保证确定性
        .env_remove("VELOX_CONFIG__SERVER__ADDR")
        .env_remove("VELOX_CONFIG__STORAGE__DATA_DIR")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("启动二进制 {bin} 失败：{e}"))?;
    // 从这里起任何失败路径都由 guard 的 Drop 负责回收子进程与临时目录
    let mut guard = ServerGuard { child, temp_dir };

    let addr = format!("127.0.0.1:{port}");
    let health_url = format!("http://{addr}/healthz");
    let deadline = Instant::now() + HEALTH_TIMEOUT;
    loop {
        // 子进程提前退出（如端口被抢占、配置解析失败）时立刻报错，不再傻等超时
        if let Ok(Some(status)) = guard.child.try_wait() {
            return Err(format!(
                "子进程提前退出（status={status}）：bin={bin}, addr={addr}, config={}",
                config_path.display()
            ));
        }
        if let Ok(resp) = client.get(&health_url).send().await {
            if resp.status() == StatusCode::OK {
                return Ok((format!("http://{addr}"), guard));
            }
        }
        if Instant::now() >= deadline {
            return Err(format!(
                "健康检查超时（{HEALTH_TIMEOUT:?}）：bin={bin}, addr={addr}, config={}",
                config_path.display()
            ));
        }
        tokio::time::sleep(HEALTH_POLL_INTERVAL).await;
    }
}

/// 起服务：最多重试 [`START_ATTEMPTS`] 次（每次重新选端口），规避临时端口被抢占的竞态。
async fn start_server(client: &Client) -> (String, ServerGuard) {
    let mut last_err = String::new();
    for _ in 0..START_ATTEMPTS {
        let port = pick_free_port();
        match try_start_server(client, port).await {
            Ok(server) => return server,
            Err(e) => last_err = e,
        }
    }
    panic!("连续 {START_ATTEMPTS} 次启动 velox-config 失败，最后一次错误：{last_err}");
}

/// 发请求并返回 (状态码, 响应 JSON)；带 body 时自动加 `Content-Type: application/json`。
/// 响应体不是合法 JSON 时 panic 并附原文，便于定位。
async fn send_json(
    client: &Client,
    method: Method,
    url: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut request = client.request(method, url);
    if let Some(value) = body {
        request = request.json(&value);
    }
    let resp = request
        .send()
        .await
        .unwrap_or_else(|e| panic!("请求 {url} 失败：{e}"));
    let status = resp.status();
    let text = resp
        .text()
        .await
        .unwrap_or_else(|e| panic!("读取 {url} 响应体失败：{e}"));
    let value: Value = serde_json::from_str(&text)
        .unwrap_or_else(|e| panic!("{url} 响应不是合法 JSON（{e}）：{text}"));
    (status, value)
}

/// 起真实二进制打真实 HTTP，验证存储层替换后对外契约未变。
#[tokio::test]
async fn http_contract_unchanged() {
    // no_proxy：避免开发机的 HTTP_PROXY 等环境变量把发往 127.0.0.1 的请求劫走
    let client = Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .no_proxy()
        .build()
        .expect("构建 reqwest 客户端失败");

    let (base, _guard) = start_server(&client).await;
    let item_url = format!("{base}/api/v1/config/ns1/k1");
    let ns_url = format!("{base}/api/v1/config/ns1");
    let all_url = format!("{base}/api/v1/config");

    // ---- 写入：PUT /api/v1/config/{ns}/{key}，body 为任意 JSON 值 ----
    let (status, body) = send_json(&client, Method::PUT, &item_url, Some(json!({"a": 1}))).await;
    assert_eq!(status, 200, "PUT 应返回 200，实际 {status}，body={body}");
    assert_eq!(
        body,
        json!({"ok": true}),
        "PUT 成功响应应为 {{\"ok\": true}}"
    );

    // ---- 读取单项：原样返回写入的 JSON 值 ----
    let (status, body) = send_json(&client, Method::GET, &item_url, None).await;
    assert_eq!(
        status, 200,
        "GET 已写入项应返回 200，实际 {status}，body={body}"
    );
    assert_eq!(body, json!({"a": 1}), "GET 单项应原样返回写入值");

    // ---- 列举 namespace 下全部配置 ----
    let (status, body) = send_json(&client, Method::GET, &ns_url, None).await;
    assert_eq!(
        status, 200,
        "GET namespace 列表应返回 200，实际 {status}，body={body}"
    );
    assert_eq!(
        body,
        json!({"k1": {"a": 1}}),
        "namespace 列举应包含全部 key"
    );

    // ---- namespace 列表（排序） ----
    let (status, body) = send_json(&client, Method::GET, &all_url, None).await;
    assert_eq!(
        status, 200,
        "GET namespace 总表应返回 200，实际 {status}，body={body}"
    );
    assert_eq!(body, json!(["ns1"]), "namespace 总表应为排序后的名字数组");

    // ---- 删除 ----
    let (status, body) = send_json(&client, Method::DELETE, &item_url, None).await;
    assert_eq!(status, 200, "DELETE 应返回 200，实际 {status}，body={body}");
    assert_eq!(
        body,
        json!({"ok": true}),
        "DELETE 成功响应应为 {{\"ok\": true}}"
    );

    // ---- 删除后读取 → 404 config_not_found ----
    let (status, body) = send_json(&client, Method::GET, &item_url, None).await;
    assert_eq!(
        status, 404,
        "GET 已删除项应返回 404，实际 {status}，body={body}"
    );
    assert_eq!(
        body["code"],
        json!("config_not_found"),
        "错误响应 code 应为 config_not_found，body={body}"
    );

    // ---- 重复删除 → 404（与 GET 语义对齐） ----
    let (status, body) = send_json(&client, Method::DELETE, &item_url, None).await;
    assert_eq!(
        status, 404,
        "重复 DELETE 应返回 404，实际 {status}，body={body}"
    );
    assert_eq!(
        body["code"],
        json!("config_not_found"),
        "重复 DELETE 的错误响应 code 应为 config_not_found，body={body}"
    );

    // ---- 删光后 namespace 本身消失 → 404 ----
    let (status, body) = send_json(&client, Method::GET, &ns_url, None).await;
    assert_eq!(
        status, 404,
        "namespace 下配置删光后应整体消失（404），实际 {status}，body={body}"
    );
    assert_eq!(
        body["code"],
        json!("config_not_found"),
        "namespace 不存在的错误响应 code 应为 config_not_found，body={body}"
    );

    // ---- 非法 namespace → 400 invalid_namespace ----
    // `$` 是 URI path 的合法字符（RFC 3986 sub-delims），reqwest 不会对其百分号编码，
    // 请求能穿过 axum 路由到达 handler，由存储层校验 `[a-zA-Z0-9_-]` 后拒绝。
    // 若实现把此类字符拦在路由层（404），此处需放宽为「非 200 且 code 非空」。
    let (status, body) = send_json(
        &client,
        Method::PUT,
        &format!("{base}/api/v1/config/bad$ns/k1"),
        Some(json!(1)),
    )
    .await;
    assert_eq!(
        status, 400,
        "非法 namespace 应返回 400，实际 {status}，body={body}"
    );
    assert_eq!(
        body["code"],
        json!("invalid_namespace"),
        "非法 namespace 的错误响应 code 应为 invalid_namespace，body={body}"
    );
}
