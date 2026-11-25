//! 实例级并发上限：超限立即 429，不排队（自保护手段，非业务限流）。

use std::sync::Arc;

use axum::extract::{Request, State};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use tokio::sync::Semaphore;

use crate::error::TooManyRequests;

/// 全局并发名额持有者；Clone 共享同一信号量（`from_fn_with_state` 的 state）。
#[derive(Clone)]
pub struct ConcurrencyLimit {
    semaphore: Arc<Semaphore>,
    max: usize,
}

impl ConcurrencyLimit {
    pub fn new(max: usize) -> Self {
        Self {
            semaphore: Arc::new(Semaphore::new(max)),
            max,
        }
    }

    pub fn max(&self) -> usize {
        self.max
    }
}

/// axum 中间件：`try_acquire_owned` 非阻塞取名额，失败立即 429（NFR-1、FR-3）。
/// 名额由 `OwnedSemaphorePermit` RAII 释放：`next.run()` 返回（响应构造完成，
/// 含 handler 出错路径）即归还，无手工增减计数，天然满足 NFR-3 不超卖与 AC-4。
pub(crate) async fn guard(
    State(limit): State<ConcurrencyLimit>,
    req: Request,
    next: Next,
) -> Response {
    match limit.semaphore.clone().try_acquire_owned() {
        Ok(_permit) => next.run(req).await, // _permit 在响应构造完成后 Drop → 归还名额
        Err(_) => {
            // NFR-2 / AC-7：每次拒绝一条 warn，含上限与路径
            tracing::warn!(
                limit = limit.max(),
                path = %req.uri().path(),
                "request rejected: instance concurrency limit reached"
            );
            TooManyRequests {
                limit: limit.max(),
            }
            .into_response()
        }
    }
}

#[cfg(test)]
mod tests {
    use axum::extract::State;
    use axum::http::header::CONTENT_TYPE;
    use axum::http::{Request as HttpRequest, StatusCode};
    use axum::routing::{get, put};
    use axum::{Json, Router};
    use serde_json::{json, Value};
    use tokio::sync::{mpsc, watch};
    use tower::ServiceExt; // oneshot

    use crate::api::limit_routes;

    /// 门闩 handler 所需共享状态：started 信号 + 放行门闩。
    #[derive(Clone)]
    struct Latch {
        started_tx: mpsc::Sender<()>,
        release_rx: watch::Receiver<bool>,
    }

    /// 可暂停 handler：进入后**先发 started 信号**（证明已通过限流层、名额已占用），
    /// 再等待放行——测试收到 started 后才发超限请求，避免 flaky。
    async fn latched_handler(State(latch): State<Latch>) -> Json<Value> {
        latch.started_tx.send(()).await.ok();
        let mut release = latch.release_rx.clone();
        while !*release.borrow_and_update() {
            release.changed().await.ok();
        }
        Json(json!({ "ok": true }))
    }

    async fn plain_ok() -> Json<Value> {
        Json(json!({ "status": "ok" }))
    }

    fn request(method: &str, uri: &str) -> HttpRequest<axum::body::Body> {
        HttpRequest::builder()
            .method(method)
            .uri(uri)
            .body(axum::body::Body::empty())
            .unwrap()
    }

    async fn body_json(res: axum::response::Response) -> Value {
        let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    /// AC-1 + AC-4：limit=2，两个请求在途时第 3 个被 429；放行后前两个 200，
    /// 第 4 个正常受理（被拒者从未占名额，无副作用）。
    #[tokio::test]
    async fn third_rejected_while_two_in_flight() {
        let (started_tx, mut started_rx) = mpsc::channel(8);
        let (release_tx, release_rx) = watch::channel(false);
        let latch = Latch {
            started_tx,
            release_rx,
        };
        let app = limit_routes(
            super::ConcurrencyLimit::new(2),
            Router::new().route("/a", get(latched_handler)),
        )
        .route("/healthz", get(plain_ok))
        .with_state(latch);

        // 两个并发请求被门闩按住
        let h1 = tokio::spawn(app.clone().oneshot(request("GET", "/a")));
        let h2 = tokio::spawn(app.clone().oneshot(request("GET", "/a")));

        // 时序要点：先等两个请求都通过限流层（started 信号），名额确已占用
        started_rx.recv().await.unwrap();
        started_rx.recv().await.unwrap();

        // 第 3 个请求：立即 429
        let res = app.clone().oneshot(request("GET", "/a")).await.unwrap();
        assert_eq!(res.status(), StatusCode::TOO_MANY_REQUESTS);

        // 放行后，前两个请求正常 200
        release_tx.send_replace(true);
        assert_eq!(h1.await.unwrap().unwrap().status(), StatusCode::OK);
        assert_eq!(h2.await.unwrap().unwrap().status(), StatusCode::OK);

        // 名额已归还：第 4 个请求正常受理（AC-4）
        let res = app.clone().oneshot(request("GET", "/a")).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
    }

    /// AC-2：跨路由、跨方法共享同一名额池——GET /a 与 PUT /b 在途合计达 2，
    /// 第 3 个请求即被拒（非每路由独立计数）。
    #[tokio::test]
    async fn shared_counter_across_routes_methods() {
        let (started_tx, mut started_rx) = mpsc::channel(8);
        let (release_tx, release_rx) = watch::channel(false);
        let latch = Latch {
            started_tx,
            release_rx,
        };
        let app = limit_routes(
            super::ConcurrencyLimit::new(2),
            Router::new()
                .route("/a", get(latched_handler))
                .route("/b", put(latched_handler)),
        )
        .with_state(latch);

        let g1 = tokio::spawn(app.clone().oneshot(request("GET", "/a")));
        let g2 = tokio::spawn(app.clone().oneshot(request("PUT", "/b")));
        started_rx.recv().await.unwrap();
        started_rx.recv().await.unwrap();

        // 第 3 个请求（任意已注册路由/方法）被拒
        let res = app.clone().oneshot(request("GET", "/b")).await.unwrap();
        assert_eq!(res.status(), StatusCode::TOO_MANY_REQUESTS);

        release_tx.send_replace(true);
        assert_eq!(g1.await.unwrap().unwrap().status(), StatusCode::OK);
        assert_eq!(g2.await.unwrap().unwrap().status(), StatusCode::OK);
    }

    /// AC-3：429 响应格式——application/json、code=too_many_requests、
    /// message 含上限数值、Retry-After: 1。
    #[tokio::test]
    async fn reject_body_shape() {
        let (started_tx, mut started_rx) = mpsc::channel(8);
        let (release_tx, release_rx) = watch::channel(false);
        let latch = Latch {
            started_tx,
            release_rx,
        };
        let app = limit_routes(
            super::ConcurrencyLimit::new(3),
            Router::new().route("/a", get(latched_handler)),
        )
        .with_state(latch);

        // 饱和 3 个名额
        let mut holders = Vec::new();
        for _ in 0..3 {
            holders.push(tokio::spawn(app.clone().oneshot(request("GET", "/a"))));
            started_rx.recv().await.unwrap();
        }

        let res = app.clone().oneshot(request("GET", "/a")).await.unwrap();
        assert_eq!(res.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(
            res.headers()
                .get(CONTENT_TYPE)
                .and_then(|v| v.to_str().ok()),
            Some("application/json")
        );
        assert_eq!(res.headers().get("retry-after").unwrap(), "1");
        let body = body_json(res).await;
        assert_eq!(body["code"], "too_many_requests");
        assert!(
            body["message"].as_str().unwrap().contains("3"),
            "message 应含上限数值，实际：{}",
            body["message"]
        );

        release_tx.send_replace(true);
        for h in holders {
            assert_eq!(h.await.unwrap().unwrap().status(), StatusCode::OK);
        }
    }

    /// Q1（结构验证）：业务路由被按满时，层外注册的豁免路由仍可用。
    #[tokio::test]
    async fn exempt_route_stays_available() {
        let (started_tx, mut started_rx) = mpsc::channel(8);
        let (release_tx, release_rx) = watch::channel(false);
        let latch = Latch {
            started_tx,
            release_rx,
        };
        let app = limit_routes(
            super::ConcurrencyLimit::new(1),
            Router::new().route("/a", get(latched_handler)),
        )
        .route("/healthz", get(plain_ok))
        .with_state(latch);

        let holder = tokio::spawn(app.clone().oneshot(request("GET", "/a")));
        started_rx.recv().await.unwrap();

        // 名额已占满，但豁免路由（注册在限流层之外）正常返回
        let res = app.clone().oneshot(request("GET", "/healthz")).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);

        // 对照：业务路由同刻被拒
        let res = app.clone().oneshot(request("GET", "/a")).await.unwrap();
        assert_eq!(res.status(), StatusCode::TOO_MANY_REQUESTS);

        release_tx.send_replace(true);
        assert_eq!(holder.await.unwrap().unwrap().status(), StatusCode::OK);
    }

    /// route_layer 语义：未命中路由（404）不经过限流层，不占名额——
    /// 名额仅 1 且被按满时，404 路径仍正常返回。
    #[tokio::test]
    async fn unmatched_route_bypasses_limit() {
        let (started_tx, mut started_rx) = mpsc::channel(8);
        let (release_tx, release_rx) = watch::channel(false);
        let latch = Latch {
            started_tx,
            release_rx,
        };
        let app = limit_routes(
            super::ConcurrencyLimit::new(1),
            Router::new().route("/a", get(latched_handler)),
        )
        .with_state(latch);

        let holder = tokio::spawn(app.clone().oneshot(request("GET", "/a")));
        started_rx.recv().await.unwrap();

        let res = app
            .clone()
            .oneshot(request("GET", "/not-exist"))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::NOT_FOUND);

        release_tx.send_replace(true);
        assert_eq!(holder.await.unwrap().unwrap().status(), StatusCode::OK);
    }
}
