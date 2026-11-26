//! velox-log：velox 共享日志库（BRD 001-addlog / 架构 02 定稿）。
//!
//! 职责：五个运行时二进制统一的日志初始化——
//! - 等级链解析：`RUST_LOG(非空) > VELOX_*__LOG__LEVEL(已设置) > 文件 [log].level(显式) > warn`，
//!   先选来源、只校验生效字符串（`filter.rs`）；
//! - 本地文件 rotate：固定 `stem.log / .log.1 / .log.2` 三文件，单文件硬字节上限 M
//!   （生产 200 MiB），0 超限容差；超长单事件截断（R=min(M,64 KiB)，UTF-8 完整前缀 +
//!   ` [truncated]\n`）；旧超限文件启动规范化收敛（`rotate.rs`）；
//! - 异步落盘：tracing-appender 0.2.4 lossy 队列（1024 条）+ 单文件 worker，
//!   文件 IO 不占 tokio worker（`bounded.rs` + appender）；
//! - 无反馈运维报告：轮转/删除/截断/IO → stderr 直达 + 固定原子计数，
//!   **不调用 tracing、不回流队列**（`report.rs`）。
//!
//! 生产无轮转参数配置（RQ-1）：`RotationLimits::injected` 仅库内部测试使用。

pub mod bounded;
pub mod filter;
pub mod report;
pub mod rotate;

#[cfg(test)]
mod tests;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Context;
use serde::Deserialize;
use tracing_subscriber::{
    layer::{Layer as _, SubscriberExt},
    EnvFilter,
};

use crate::bounded::BoundedMakeWriter;
use crate::report::{ReportSink, StdReportSink};
use crate::rotate::{LogPaths, QueueDropState, RotationLimits, SizeRotatingWriter, StdFs};

/// 单文件字节上限（BRD：200 MiB；生产固定，不开放配置）
pub const MAX_FILE_BYTES: u64 = 200 * 1024 * 1024;
/// 保留文件数（含活跃共 3：Q1 定案）
pub const KEEP_FILES: usize = 3;
/// 单事件落盘上限 R=min(M,64 KiB) 的 64 KiB 侧（生产 M >> 64KiB）
pub const MAX_EVENT_BYTES: usize = 64 * 1024;
/// lossy 队列容量（§4.5：不用上游 128000 默认）
pub const QUEUE_EVENTS: usize = 1024;

/// `[log]` 配置节（各子系统 Config 持有）。
///
/// serde 经私有 [`RawLogConfig`] 转换：`dir`/`level` 缺省取内置默认，
/// 并记录 `level` 是否在文件显式出现（§4.4：文件来源判定；
/// 空 `level = ""` 保留为显式空串，属于非法值而非缺省）。
#[derive(Debug, Clone, Deserialize)]
#[serde(from = "RawLogConfig")]
pub struct LogConfig {
    /// 日志目录；默认 `logs`（相对进程 cwd 解析），`VELOX_*__LOG__DIR` 可覆盖
    pub dir: String,
    /// 文件显式等级；默认 `warn`。保留文件原值（未被 env 改写）；
    /// 是否生效由等级链决定
    pub level: String,
    /// 文件是否显式提供 `[log].level`（非 serde 字段，RawLogConfig 转换时记录）
    file_level_present: bool,
}

impl Default for LogConfig {
    fn default() -> Self {
        Self {
            dir: "logs".to_string(),
            level: "warn".to_string(),
            file_level_present: false,
        }
    }
}

#[derive(Debug, Deserialize)]
struct RawLogConfig {
    dir: Option<String>,
    level: Option<String>,
}

impl From<RawLogConfig> for LogConfig {
    fn from(raw: RawLogConfig) -> Self {
        let level = raw.level;
        Self {
            dir: raw.dir.unwrap_or_else(|| "logs".to_string()),
            file_level_present: level.is_some(),
            level: level.unwrap_or_else(|| "warn".to_string()),
        }
    }
}

/// 等级生效来源（§4.4；值与来源绑定，较低来源不冒充生效来源）
#[derive(Debug, Clone, PartialEq)]
pub enum LevelSource {
    /// 内置默认 warn
    Default,
    /// 配置文件显式 `[log].level`（绝对路径）
    File(PathBuf),
    /// 环境变量（`RUST_LOG` 或 `VELOX_<PREFIX>__LOG__LEVEL`）
    Env(String),
}

/// 解析结果：生效值 + 来源 + 已构造过滤器（init 直接使用，不再读取环境）
#[derive(Debug, Clone)]
pub struct ResolvedLevel {
    pub value: String,
    pub source: LevelSource,
    filter: EnvFilter,
}

impl Default for ResolvedLevel {
    /// Default = warn/Default（§4.4 例表：文件缺省 + 无 env → warn/Default）
    fn default() -> Self {
        Self {
            value: "warn".to_string(),
            source: LevelSource::Default,
            filter: EnvFilter::new("warn"),
        }
    }
}

impl ResolvedLevel {
    pub fn filter(&self) -> &EnvFilter {
        &self.filter
    }

    pub fn source_display(&self) -> String {
        match &self.source {
            LevelSource::Default => "default".to_string(),
            LevelSource::File(p) => format!("file:{}", p.display()),
            LevelSource::Env(name) => format!("env:{name}"),
        }
    }
}

/// 相对路径以 cwd 拼接为绝对上下文（配置错误信息用；current_dir 失败回退原值）
pub fn absolutize(p: &Path) -> PathBuf {
    if p.is_absolute() {
        return p.to_path_buf();
    }
    match std::env::current_dir() {
        Ok(cwd) => cwd.join(p),
        Err(_) => p.to_path_buf(),
    }
}

/// 等级链解析（生产入口）：读真实进程环境。
/// `path` 为本次 `Config::load` 的真实配置文件路径（需绝对上下文；
/// 未提供配置文件时传 None——此时文件来源不可能生效）。
pub fn resolve_level(
    log: &LogConfig,
    path: Option<&Path>,
    prefix: &str,
) -> anyhow::Result<ResolvedLevel> {
    let env = filter::EnvInputs::from_std_env();
    let sel = filter::select_source(log, path, prefix, &env)?;
    filter::build_resolved(sel)
}

/// init 复用的装配参数（私有：生产用固定 Limits/StdReportSink/StdFs；
/// 测试注入小阈值/FaultFs/TestReportSink，不暴露配置/启动参数/环境变量）
pub(crate) struct BuildOptions<F: rotate::FileOps> {
    pub dir: PathBuf,
    pub stem: String,
    pub filter: EnvFilter,
    pub limits: RotationLimits,
    pub queue_events: usize,
    pub report: Arc<dyn ReportSink>,
    pub fs: F,
}

/// 内部装配（不安装全局 subscriber；§6.1.2 测试经局部 Dispatch 注入）：
/// 规范化+打开活跃 → NonBlocking（lossy 队列）→ BoundedMakeWriter → 双 fmt 层。
pub(crate) fn build_dispatch<F: rotate::FileOps>(
    opts: BuildOptions<F>,
) -> anyhow::Result<(tracing::Dispatch, LogGuard)> {
    let paths = LogPaths::new(&opts.dir, &opts.stem);
    let queue_state = Arc::new(QueueDropState::new());
    let mut writer =
        SizeRotatingWriter::new(opts.fs, paths, opts.limits, Arc::clone(&opts.report))?;
    writer.set_queue_state(Arc::clone(&queue_state));

    // lossy 队列：显式容量 1024、丢弃计数外显、独立 worker 线程（§4.5）
    let (non_blocking, worker_guard) =
        tracing_appender::non_blocking::NonBlockingBuilder::default()
            .buffered_lines_limit(opts.queue_events)
            .lossy(true)
            .thread_name("velox-log")
            .finish(writer);
    queue_state.set(non_blocking.error_counter());

    // 事件限长容量 R=min(M, 64 KiB)（生产 M=200MiB → R=64KiB；测试注入小阈值取 min）
    let bounded = BoundedMakeWriter::new(
        non_blocking,
        rotate::record_cap(opts.limits.max_bytes),
        Arc::new(std::sync::atomic::AtomicU64::new(0)),
    );

    // 文件 fmt：ANSI off + 限长 writer + 同一过滤器
    let file_layer = tracing_subscriber::fmt::layer()
        .with_ansi(false)
        .with_writer(bounded)
        .with_filter(opts.filter.clone());
    // stdout fmt：保留裸进程终端输出（Q6）；off 时两 sink 均无业务日志
    let stdout_layer = tracing_subscriber::fmt::layer()
        .with_writer(std::io::stdout)
        .with_filter(opts.filter);

    let subscriber = tracing_subscriber::registry()
        .with(file_layer)
        .with(stdout_layer);
    let dispatch = tracing::Dispatch::new(subscriber);
    let guard = LogGuard::new(worker_guard, opts.report, queue_state);
    Ok((dispatch, guard))
}

/// 日志 guard：worker 生命周期 + 报告 sink + 队列丢弃计数。
/// drop 仅做最后一次队列采样与 closing 标记，再 drop WorkerGuard
/// （appender 握手有界：发送 Shutdown ≤100ms、等待完成 ≤1000ms，不无限 join）。
pub struct LogGuard {
    inner: Option<GuardInner>,
}

struct GuardInner {
    worker_guard: tracing_appender::non_blocking::WorkerGuard,
    report: Arc<dyn ReportSink>,
    queue: Arc<QueueDropState>,
}

impl LogGuard {
    fn new(
        worker_guard: tracing_appender::non_blocking::WorkerGuard,
        report: Arc<dyn ReportSink>,
        queue: Arc<QueueDropState>,
    ) -> Self {
        Self {
            inner: Some(GuardInner {
                worker_guard,
                report,
                queue,
            }),
        }
    }
}

impl Drop for LogGuard {
    fn drop(&mut self) {
        if let Some(inner) = self.inner.take() {
            // 最后一次队列丢弃采样（计数落盘后由 worker 尽力输出/汇总）
            if let Some(dropped) = inner.queue.sample() {
                if dropped > 0 {
                    inner.report.record(report::ReportEvent::QueueDropped {
                        stem: String::new(),
                        dropped,
                    });
                }
            }
            inner.report.mark_closing();
            drop(inner.worker_guard); // 有界等待（尽力排空已入队记录）
        }
    }
}

/// 生产初始化（五个入口统一调用）：
/// mkdir（fail-fast，E1）→ 启动规范化 → 打开活跃 → 装配 dispatch → 安装全局 subscriber
/// → INFO 记录日志有效配置（TC-20：source/value/dir/M/KEEP/cap/队列；默认 warn 静默）。
pub fn init(crate_name: &str, log: &LogConfig, level: &ResolvedLevel) -> anyhow::Result<LogGuard> {
    let dir = PathBuf::from(&log.dir);
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("创建日志目录 {} 失败", dir.display()))?;
    let (dispatch, guard) = build_dispatch(BuildOptions {
        dir: dir.clone(),
        stem: crate_name.to_string(),
        filter: level.filter().clone(),
        limits: RotationLimits::production(),
        queue_events: QUEUE_EVENTS,
        report: StdReportSink::shared(),
        fs: StdFs,
    })?;
    tracing::dispatcher::set_global_default(dispatch)
        .map_err(|e| anyhow::anyhow!("安装全局日志 subscriber 失败（进程内重复 init？）：{e}"))?;

    // 启动有效配置行（经新 subscriber 落文件+stdout；默认 warn 下静默，显式 info 可查）
    let abs_dir = std::fs::canonicalize(&dir).unwrap_or(dir);
    tracing::info!(
        source = %level.source_display(),
        value = %level.value,
        dir = %abs_dir.display(),
        max_file_bytes = MAX_FILE_BYTES,
        keep_files = KEEP_FILES,
        event_cap_bytes = rotate::record_cap(MAX_FILE_BYTES),
        queue_events = QUEUE_EVENTS,
        "日志初始化完成"
    );
    Ok(guard)
}
