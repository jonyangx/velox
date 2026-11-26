//! 无反馈运维报告通道（架构 §4.3.4/G2-4）。
//!
//! 纪律：**任何 writer/截断/队列内部报告都不得调用 tracing、不得发回 NonBlocking、
//! 不得写本地诊断文件**——因果图仅「业务事件 → 文件 writer → stderr」，
//! 无 stderr→subscriber 回边，业务停止后小阈值也不会自激轮转。
//! 轮转/删除/截断/规范化 → WARN；IO 失败/队列丢弃 → ERROR。
//! 行格式 `component=velox-log …`，路径/错误文本截断使整行 ≤1024 字节；
//! `writeln!(stderr.lock())` 并忽略写失败（不用会 panic 的 eprintln!）。
//! 限频：按类别 60s（时间源可注入），首次立即，被抑制次数累加、下次输出带 counts。
//! 只有启动线程与文件 worker 执行 stderr IO；业务线程与 LogGuard::drop 只更新原子计数。

use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// 报告限频窗口（生产 60s；测试经 `with_clock` 注入时间源，不等待真实 60s）
pub(crate) const REPORT_RATE_WINDOW: Duration = Duration::from_secs(60);
/// 单行报告上限（路径/错误文本被截断以满足）
const MAX_LINE_BYTES: usize = 1024;

/// 类型化运维事件（§4.3.4 ReportEvent）
#[derive(Debug, Clone)]
pub(crate) enum ReportEvent {
    /// 完成一次轮转链（WARN）
    RotateCompleted { stem: String },
    /// 删除最旧归档 / 超保留序号旧文件（WARN）
    OldestRemoved { stem: String, path: PathBuf },
    /// 旧文件超限收敛 set_len(M)（WARN，含原长度/新长度/路径）
    Truncated {
        stem: String,
        path: PathBuf,
        old_len: u64,
        new_len: u64,
    },
    /// 遗留副本清理（如 `<stem>.stderr.log`，WARN）
    LegacyNormalized { stem: String, path: PathBuf },
    /// IO 失败（ERROR，含阶段与路径；文本截断）
    IoFailed {
        stem: String,
        stage: &'static str,
        kind: String,
        path: PathBuf,
    },
    /// 队列丢弃（ERROR；lossy NonBlocking error_counter 采样差值）
    QueueDropped { stem: String, dropped: u64 },
}

impl ReportEvent {
    fn category(&self) -> Category {
        match self {
            ReportEvent::RotateCompleted { .. } => Category::Rotate,
            ReportEvent::OldestRemoved { .. } => Category::OldestRemoved,
            ReportEvent::Truncated { .. } => Category::Truncated,
            ReportEvent::LegacyNormalized { .. } => Category::Legacy,
            ReportEvent::IoFailed { .. } => Category::Io,
            ReportEvent::QueueDropped { .. } => Category::Queue,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Category {
    Rotate,
    OldestRemoved,
    Truncated,
    Legacy,
    Io,
    Queue,
}

/// 固定数量原子计数快照（§4.3.4：生产不存事件列表，只有计数）
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ReportCounts {
    pub rotate: u64,
    pub oldest_removed: u64,
    pub truncated: u64,
    pub legacy: u64,
    pub io_failed: u64,
    pub queue_dropped: u64,
    /// 因限频被抑制而未输出的报告条数
    pub suppressed: u64,
}

/// 报告 sink（生产：stderr + 限频；测试：有界事件记录）
pub(crate) trait ReportSink: Send + Sync + 'static {
    fn record(&self, event: ReportEvent);
    /// guard 关停标记：closing 后抑制常规输出（最终汇总由 worker 尽力执行）
    fn mark_closing(&self);
    /// 计数快照（测试断言用；生产实现维护固定原子计数）
    #[allow(dead_code)]
    fn counts(&self) -> ReportCounts {
        ReportCounts::default()
    }
}

/// 生产实现：stderr 直达 + 按类别限频 + 固定原子计数。
/// stderr 写失败忽略（终端断开/管道关闭不影响业务）。
pub(crate) struct StdReportSink {
    counters: Counters,
    closing: AtomicBool,
    rate: Mutex<[RateState; 6]>,
    window: Duration,
    now: Box<dyn Fn() -> Instant + Send + Sync>,
}

#[derive(Default)]
struct Counters {
    rotate: AtomicU64,
    oldest_removed: AtomicU64,
    truncated: AtomicU64,
    legacy: AtomicU64,
    io_failed: AtomicU64,
    queue_dropped: AtomicU64,
    suppressed: AtomicU64,
}

#[derive(Default)]
struct RateState {
    last_emit: Option<Instant>,
    suppressed: u64,
}

impl StdReportSink {
    pub(crate) fn shared() -> Arc<Self> {
        Arc::new(Self::with_clock(REPORT_RATE_WINDOW, Box::new(Instant::now)))
    }

    /// 测试注入：时间源 + 窗口（不需要真实等待 60s 测限频）
    pub(crate) fn with_clock(
        window: Duration,
        now: Box<dyn Fn() -> Instant + Send + Sync>,
    ) -> Self {
        Self {
            counters: Counters::default(),
            closing: AtomicBool::new(false),
            rate: Mutex::new(Default::default()),
            window,
            now,
        }
    }

    fn category_index(c: Category) -> usize {
        match c {
            Category::Rotate => 0,
            Category::OldestRemoved => 1,
            Category::Truncated => 2,
            Category::Legacy => 3,
            Category::Io => 4,
            Category::Queue => 5,
        }
    }

    fn snapshot(&self) -> ReportCounts {
        ReportCounts {
            rotate: self.counters.rotate.load(Ordering::Relaxed),
            oldest_removed: self.counters.oldest_removed.load(Ordering::Relaxed),
            truncated: self.counters.truncated.load(Ordering::Relaxed),
            legacy: self.counters.legacy.load(Ordering::Relaxed),
            io_failed: self.counters.io_failed.load(Ordering::Relaxed),
            queue_dropped: self.counters.queue_dropped.load(Ordering::Relaxed),
            suppressed: self.counters.suppressed.load(Ordering::Relaxed),
        }
    }
}

impl ReportSink for StdReportSink {
    fn record(&self, event: ReportEvent) {
        // 计数无条件更新（closing 后仍累计，仅抑制输出）
        match &event {
            ReportEvent::RotateCompleted { .. } => {
                self.counters.rotate.fetch_add(1, Ordering::Relaxed)
            }
            ReportEvent::OldestRemoved { .. } => {
                self.counters.oldest_removed.fetch_add(1, Ordering::Relaxed)
            }
            ReportEvent::Truncated { .. } => {
                self.counters.truncated.fetch_add(1, Ordering::Relaxed)
            }
            ReportEvent::LegacyNormalized { .. } => {
                self.counters.legacy.fetch_add(1, Ordering::Relaxed)
            }
            ReportEvent::IoFailed { .. } => self.counters.io_failed.fetch_add(1, Ordering::Relaxed),
            ReportEvent::QueueDropped { .. } => {
                self.counters.queue_dropped.fetch_add(1, Ordering::Relaxed)
            }
        };
        if self.closing.load(Ordering::Relaxed) {
            return;
        }
        let idx = Self::category_index(event.category());
        let mut rate = self.rate.lock().unwrap_or_else(|e| e.into_inner());
        let state = &mut rate[idx];
        let now = (self.now)();
        let due = match state.last_emit {
            None => true,
            Some(last) => now.duration_since(last) >= self.window,
        };
        if !due {
            state.suppressed += 1;
            self.counters.suppressed.fetch_add(1, Ordering::Relaxed);
            return;
        }
        let suppressed = state.suppressed;
        state.suppressed = 0;
        state.last_emit = Some(now);
        drop(rate);
        let counts = self.snapshot();
        let line = format_event(&event, suppressed, &counts);
        // stderr 写失败忽略（调用端不可达时报告尽力而为）
        let mut err = std::io::stderr().lock();
        let _ = writeln!(err, "{line}");
    }

    fn mark_closing(&self) {
        self.closing.store(true, Ordering::Relaxed);
    }

    fn counts(&self) -> ReportCounts {
        self.snapshot()
    }
}

/// 单行组装：≤1024 字节（路径/错误文本截断），含 stage/stem/累计计数/限频抑制数
fn format_event(event: &ReportEvent, suppressed: u64, counts: &ReportCounts) -> String {
    let counts_str = format!(
        "rotate={} removed={} truncated={} legacy={} io={} queue_drop={}",
        counts.rotate,
        counts.oldest_removed,
        counts.truncated,
        counts.legacy,
        counts.io_failed,
        counts.queue_dropped
    );
    let (level, stage, stem, detail) = match event {
        ReportEvent::RotateCompleted { stem } => ("WARN", "rotate_completed", stem, String::new()),
        ReportEvent::OldestRemoved { stem, path } => (
            "WARN",
            "oldest_removed",
            stem,
            format!(" path={}", clip(&path.display().to_string(), 200)),
        ),
        ReportEvent::Truncated {
            stem,
            path,
            old_len,
            new_len,
        } => (
            "WARN",
            "truncated_to_limit",
            stem,
            format!(
                " path={} old_len={old_len} new_len={new_len}",
                clip(&path.display().to_string(), 200)
            ),
        ),
        ReportEvent::LegacyNormalized { stem, path } => (
            "WARN",
            "legacy_copy_removed",
            stem,
            format!(" path={}", clip(&path.display().to_string(), 200)),
        ),
        ReportEvent::IoFailed {
            stem,
            stage,
            kind,
            path,
        } => (
            "ERROR",
            "io_failed",
            stem,
            format!(
                " io_stage={stage} kind={} path={}",
                clip(kind, 120),
                clip(&path.display().to_string(), 200)
            ),
        ),
        ReportEvent::QueueDropped { stem, dropped } => (
            "ERROR",
            "queue_dropped",
            stem,
            format!(" dropped_lines={dropped}"),
        ),
    };
    let suffix = if suppressed > 0 {
        format!(" suppressed={suppressed}")
    } else {
        String::new()
    };
    let prefix = format!("component=velox-log level={level} stage={stage} stem={stem}");
    // 总行长 ≤1024：超出时截断 detail
    let budget = MAX_LINE_BYTES
        .saturating_sub(prefix.len())
        .saturating_sub(counts_str.len())
        .saturating_sub(suffix.len())
        .saturating_sub(3);
    let detail_clipped = if detail.len() > budget {
        format!(" {}", clip(detail.trim_start(), budget.saturating_sub(1)))
    } else {
        detail
    };
    format!("{prefix}{detail_clipped} counts={counts_str}{suffix}")
}

/// 按字节截断并标注（不切断 UTF-8 多字节序列——按字节边界回退）
fn clip(s: &str, max_bytes: usize) -> String {
    if s.len() <= max_bytes {
        return s.to_string();
    }
    let mut cut = max_bytes.saturating_sub(3);
    while cut > 0 && !s.is_char_boundary(cut) {
        cut -= 1;
    }
    format!("{}…", &s[..cut])
}

#[cfg(test)]
pub(crate) struct TestReportSink {
    events: Mutex<Vec<ReportEvent>>,
    counters: Mutex<ReportCounts>,
    closing: AtomicBool,
}

#[cfg(test)]
impl TestReportSink {
    pub(crate) fn shared() -> Arc<Self> {
        Arc::new(Self {
            events: Mutex::new(Vec::new()),
            counters: Mutex::new(ReportCounts::default()),
            closing: AtomicBool::new(false),
        })
    }

    pub(crate) fn events(&self) -> Vec<ReportEvent> {
        self.events
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
}

#[cfg(test)]
impl ReportSink for TestReportSink {
    fn record(&self, event: ReportEvent) {
        let mut counters = self.counters.lock().unwrap_or_else(|e| e.into_inner());
        match &event {
            ReportEvent::RotateCompleted { .. } => counters.rotate += 1,
            ReportEvent::OldestRemoved { .. } => counters.oldest_removed += 1,
            ReportEvent::Truncated { .. } => counters.truncated += 1,
            ReportEvent::LegacyNormalized { .. } => counters.legacy += 1,
            ReportEvent::IoFailed { .. } => counters.io_failed += 1,
            ReportEvent::QueueDropped { .. } => counters.queue_dropped += 1,
        }
        let mut events = self.events.lock().unwrap_or_else(|e| e.into_inner());
        // 有界记录（避免故障风暴撑爆测试内存）：保留最近 1024 条
        if events.len() >= 1024 {
            events.remove(0);
        }
        events.push(event);
    }

    fn mark_closing(&self) {
        self.closing.store(true, Ordering::Relaxed);
    }

    fn counts(&self) -> ReportCounts {
        *self.counters.lock().unwrap_or_else(|e| e.into_inner())
    }
}
