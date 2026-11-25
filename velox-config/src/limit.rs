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

