//! 按事件限长的 MakeWriter（架构 §4.2.2/G2-2）。
//!
//! 每次事件（一次 `make_writer`）创建独立缓冲，容量 R=min(M,64 KiB)；
//! fmt 的多次 `write` 只向该缓冲追加，超出部分计数丢弃并返回输入已消费；
//! 事件缓冲 drop 时**只提交一次**：完整事件原样提交；超长事件保留 UTF-8 完整前缀、
//! 在 R 内预留 ASCII 后缀 ` [truncated]\n`，最终长度 ≤R。**不会创建第二个无限缓冲
//! 或第二条队列记录**。业务线程只更新截断事件计数（原子量），不做 stderr IO。

use std::io;
use std::io::Write as _;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use tracing_appender::non_blocking::NonBlocking;

/// 截断标记（ASCII，预留于 R 内）
pub(crate) const TRUNCATION_MARKER: &[u8] = b" [truncated]\n";

/// 截断组装：保留 UTF-8 完整前缀 + 标记，最终长度 ≤ cap。
/// （`truncated=false` 时原样返回。）
pub(crate) fn cap_with_marker(buf: &[u8], cap: usize, truncated: bool) -> Vec<u8> {
    if !truncated {
        return buf.to_vec();
    }
    let budget = cap.saturating_sub(TRUNCATION_MARKER.len());
    let mut cut = buf.len().min(budget);
    // 剔除残缺 UTF-8 字节：切点为尾随连续字节（0b10xxxxxx）时回退
    while cut > 0 && (buf[cut] & 0xC0) == 0x80 {
        cut -= 1;
    }
    let mut out = Vec::with_capacity(cut + TRUNCATION_MARKER.len());
    out.extend_from_slice(&buf[..cut]);
    out.extend_from_slice(TRUNCATION_MARKER);
    debug_assert!(out.len() <= cap, "截断结果不得超过 cap");
    out
}

/// 包裹 NonBlocking 的事件限长层（文件 fmt 的 MakeWriter）
pub(crate) struct BoundedMakeWriter {
    inner: NonBlocking,
    cap: usize,
    /// 截断事件计数（仅业务线程更新的原子量；stderr 报告由 worker 采样输出）
    truncated_events: Arc<AtomicU64>,
}

impl BoundedMakeWriter {
    pub(crate) fn new(inner: NonBlocking, cap: usize, truncated_events: Arc<AtomicU64>) -> Self {
        Self {
            inner,
            cap,
            truncated_events,
        }
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for BoundedMakeWriter {
    type Writer = EventBuffer;

    fn make_writer(&'a self) -> Self::Writer {
        EventBuffer {
            sink: self.inner.make_writer(),
            buf: Vec::with_capacity(256),
            cap: self.cap,
            dropped_bytes: 0,
            truncated_events: Arc::clone(&self.truncated_events),
        }
    }
}

/// 单事件缓冲：跨多次 write 聚合，drop 时一次性提交（空事件不提交）
pub(crate) struct EventBuffer {
    sink: NonBlocking,
    buf: Vec<u8>,
    cap: usize,
    dropped_bytes: usize,
    truncated_events: Arc<AtomicU64>,
}

impl io::Write for EventBuffer {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let space = self.cap.saturating_sub(self.buf.len());
        let keep = space.min(buf.len());
        if keep > 0 {
            self.buf.extend_from_slice(&buf[..keep]);
        }
        if buf.len() > keep {
            self.dropped_bytes += buf.len() - keep;
        }
        // 超出部分主动丢弃并返回输入已消费（不向队列提交超长内容）
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Drop for EventBuffer {
    fn drop(&mut self) {
        if self.buf.is_empty() && self.dropped_bytes == 0 {
            return; // 空记录无操作
        }
        let payload = if self.dropped_bytes > 0 {
            self.truncated_events.fetch_add(1, Ordering::Relaxed);
            cap_with_marker(&self.buf, self.cap, true)
        } else {
            std::mem::take(&mut self.buf)
        };
        // 只提交一次；lossy 队列满即丢（由 error_counter 采样为 QueueDropped）
        let _ = self.sink.write_all(&payload);
        let _ = self.sink.flush();
    }
}
