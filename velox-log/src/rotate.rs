//! 硬边界轮转 writer（架构 §4.2/§4.3，G2-1/G2-2/G2-5）。
//!
//! 硬不变量：初始化成功后只持有 `stem.log` / `stem.log.1` / `stem.log.2`；
//! 任一受管文件逻辑长度 ≤ M，任一写入不越过 M，总和 ≤ 3M（0 超限容差；
//! 整块提前轮转的未用空间 < 本块长度 ≤ R）。
//! 启动规范化先于 subscriber 安装：超限旧文件 `set_len(M)` 收敛、
//! 遗留 `<stem>.stderr.log` 与 `.log.N`（N≥3）精确清理，失败 fail-fast。
//! 运行期按 §4.3.2 失败表工作：phase 仅在成功后推进；`written: Option<u64>`
//! None=未知、严禁旧计数透过 reopen；每条非空写入触发一次有界恢复链尝试。

use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::{Arc, OnceLock};

use anyhow::Context;

use crate::report::{ReportEvent, ReportSink};
use crate::{bounded::cap_with_marker, MAX_EVENT_BYTES};

/// 单事件上限（= min(M, 64 KiB) 生产口径；测试注入小阈值时取 min(M, 64 KiB)）
pub(crate) fn record_cap(max_bytes: u64) -> usize {
    max_bytes.min(MAX_EVENT_BYTES as u64) as usize
}

/// 轮转参数（§4.2.2：只供库内部测试注入小阈值；生产固定常量，不开放配置）
#[derive(Debug, Clone, Copy)]
pub(crate) struct RotationLimits {
    pub max_bytes: u64,
    /// 保留文件数（固定 3；由构造校验约束，运行期轮转链隐式按三文件布局执行）
    #[allow(dead_code)]
    pub keep_files: usize,
}

impl RotationLimits {
    pub const fn production() -> Self {
        Self {
            max_bytes: crate::MAX_FILE_BYTES,
            keep_files: crate::KEEP_FILES,
        }
    }

    /// 注入构造：max_bytes ≥32、keep_files 固定 3（§4.2.2 约束；仅测试路径使用）
    #[allow(dead_code)]
    pub fn injected(max_bytes: u64, keep_files: usize) -> anyhow::Result<Self> {
        if max_bytes < 32 {
            anyhow::bail!("注入轮转参数非法：max_bytes={max_bytes}（要求 ≥32）");
        }
        if keep_files != 3 {
            anyhow::bail!("注入轮转参数非法：keep_files={keep_files}（固定 3）");
        }
        Ok(Self {
            max_bytes,
            keep_files,
        })
    }
}

/// 受管路径集合（仅三个 + 供遗留清理的扫描名）
#[derive(Debug, Clone)]
pub(crate) struct LogPaths {
    pub dir: PathBuf,
    pub stem: String,
    pub active: PathBuf,
    pub archive1: PathBuf,
    pub archive2: PathBuf,
}

impl LogPaths {
    pub fn new(dir: &Path, stem: &str) -> Self {
        Self {
            dir: dir.to_path_buf(),
            stem: stem.to_string(),
            active: dir.join(format!("{stem}.log")),
            archive1: dir.join(format!("{stem}.log.1")),
            archive2: dir.join(format!("{stem}.log.2")),
        }
    }

    fn managed_name(&self, name: &str) -> Option<ManagedSlot> {
        if name == self.active_name() {
            Some(ManagedSlot::Active)
        } else if name == self.archive1_name() {
            Some(ManagedSlot::Archive1)
        } else if name == self.archive2_name() {
            Some(ManagedSlot::Archive2)
        } else {
            None
        }
    }

    fn active_name(&self) -> String {
        format!("{}.log", self.stem)
    }
    fn archive1_name(&self) -> String {
        format!("{}.log.1", self.stem)
    }
    fn archive2_name(&self) -> String {
        format!("{}.log.2", self.stem)
    }

    /// 遗留副本名（前版脚本设计）：`<stem>.stderr.log`
    fn stderr_copy_name(&self) -> String {
        format!("{}.stderr.log", self.stem)
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum ManagedSlot {
    Active,
    Archive1,
    Archive2,
}

/// 文件标识（Unix；NFR-7 不承诺 Windows）
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Meta {
    pub len: u64,
    pub dev: u64,
    pub ino: u64,
    pub nlink: u64,
}

/// 文件操作适配器：生产 StdFs；FaultFs（cfg(test)）支持每操作第 N 次失败、
/// 短写、WriteZero、Interrupted、flush 失败与 worker 阻塞门。
pub(crate) trait FileOps: Send + 'static {
    type Handle: Send;

    fn read_dir(&self, dir: &Path) -> io::Result<Vec<PathBuf>>;
    fn metadata_path(&self, path: &Path) -> io::Result<Meta>;
    fn metadata_handle(&self, handle: &mut Self::Handle) -> io::Result<Meta>;
    fn open_append_create(&self, path: &Path) -> io::Result<Self::Handle>;
    fn set_len(&self, handle: &mut Self::Handle, len: u64) -> io::Result<()>;
    fn remove(&self, path: &Path) -> io::Result<()>;
    fn rename(&self, from: &Path, to: &Path) -> io::Result<()>;
    /// 单次底层写：返回实际写入 n（可能短写）；Err(Interrupted) 未写任何字节
    fn write(&self, handle: &mut Self::Handle, bytes: &[u8]) -> io::Result<usize>;
    fn flush(&self, handle: &mut Self::Handle) -> io::Result<()>;
}

/// 生产实现：标准库文件（内部不套 BufWriter）
pub(crate) struct StdFs;

impl FileOps for StdFs {
    type Handle = std::fs::File;

    fn read_dir(&self, dir: &Path) -> io::Result<Vec<PathBuf>> {
        let mut out = Vec::new();
        for entry in std::fs::read_dir(dir)? {
            out.push(entry?.path());
        }
        Ok(out)
    }

    fn metadata_path(&self, path: &Path) -> io::Result<Meta> {
        Ok(Meta::from_std(&std::fs::metadata(path)?))
    }

    fn metadata_handle(&self, handle: &mut Self::Handle) -> io::Result<Meta> {
        Ok(Meta::from_std(&handle.metadata()?))
    }

    fn open_append_create(&self, path: &Path) -> io::Result<Self::Handle> {
        std::fs::OpenOptions::new()
            .append(true)
            .create(true)
            .open(path)
    }

    fn set_len(&self, handle: &mut Self::Handle, len: u64) -> io::Result<()> {
        handle.set_len(len)
    }

    fn remove(&self, path: &Path) -> io::Result<()> {
        std::fs::remove_file(path)
    }

    fn rename(&self, from: &Path, to: &Path) -> io::Result<()> {
        std::fs::rename(from, to)
    }

    fn write(&self, handle: &mut Self::Handle, bytes: &[u8]) -> io::Result<usize> {
        handle.write(bytes)
    }

    fn flush(&self, handle: &mut Self::Handle) -> io::Result<()> {
        handle.flush()
    }
}

impl Meta {
    #[cfg(unix)]
    fn from_std(m: &std::fs::Metadata) -> Self {
        use std::os::unix::fs::MetadataExt;
        Self {
            len: m.len(),
            dev: m.dev(),
            ino: m.ino(),
            nlink: m.nlink(),
        }
    }

    #[cfg(not(unix))]
    fn from_std(_m: &std::fs::Metadata) -> Self {
        panic!("velox-log 仅支持 Unix 平台（NFR-7）")
    }
}

// 非 Unix 平台直接拒绝编译（NFR-7：Linux/macOS，不承诺 Windows）
#[cfg(not(unix))]
compile_error!("velox-log 仅支持 Unix 平台（NFR-7：Linux/macOS），不承诺 Windows");

/// 队列丢弃计数共享状态：NonBlocking 创建完成后 set ErrorCounter，
/// worker 在每次 write/flush 采样差值 → QueueDropped 事件（§4.3.4）
#[derive(Default)]
pub(crate) struct QueueDropState {
    counter: OnceLock<tracing_appender::non_blocking::ErrorCounter>,
    // ErrorCounter::dropped_lines() 为 usize（AtomicUsize）
    last_seen: std::sync::atomic::AtomicUsize,
}

impl QueueDropState {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn set(&self, counter: tracing_appender::non_blocking::ErrorCounter) {
        let _ = self.counter.set(counter);
    }

    /// 距上次采样的新增丢弃行数（usize → u64 供报告事件使用）
    pub(crate) fn sample(&self) -> Option<u64> {
        let counter = self.counter.get()?;
        let now = counter.dropped_lines();
        let prev = self.last_seen.swap(now, Ordering::Relaxed);
        Some(u64::try_from(now.saturating_sub(prev)).unwrap_or(0))
    }
}

/// 恢复阶段（§4.3.1）：Ready 不变量 = current=Some、written=Some(实际长度≤M)；
/// 其余 phase 均 current=None、written=None，严禁旧计数透过 reopen。
#[derive(Debug, Clone, Copy, PartialEq)]
enum Phase {
    Ready,
    RemoveOldest,
    MoveOne,
    MoveActive,
    OpenFresh,
    Reopen,
}

/// 单 worker 独占的串行 writer（无锁；文件 IO 只发生在 tracing-appender 专用线程）
pub(crate) struct SizeRotatingWriter<F: FileOps> {
    fs: F,
    paths: LogPaths,
    limits: RotationLimits,
    current: Option<F::Handle>,
    written: Option<u64>,
    phase: Phase,
    report: Arc<dyn ReportSink>,
    queue: Arc<QueueDropState>,
}

impl<F: FileOps> SizeRotatingWriter<F> {
    /// 构造 = 启动规范化 + append 打开活跃（§4.2.3；目录须已存在——mkdir 是 init 的职责）。
    /// 任何规范化失败 fail-fast（错误含路径与阶段），进程不得接客。
    pub fn new(
        fs: F,
        paths: LogPaths,
        limits: RotationLimits,
        report: Arc<dyn ReportSink>,
    ) -> anyhow::Result<Self> {
        Self::normalize(&fs, &paths, limits.max_bytes, &report)
            .context("日志文件启动规范化失败")?;
        let mut writer = Self {
            fs,
            paths,
            limits,
            current: None,
            written: None,
            phase: Phase::OpenFresh,
            report,
            queue: Arc::new(QueueDropState::new()),
        };
        writer.step_open_active().context("打开活跃日志文件失败")?;
        Ok(writer)
    }

    pub(crate) fn set_queue_state(&mut self, queue: Arc<QueueDropState>) {
        self.queue = queue;
    }

    /// 启动规范化（§4.2.3）：
    /// - 受管三文件长度 ≤M 原样保留；>M 以可写句柄 set_len(M) 保留前 M 字节并 WARN
    ///   （不用复制/临时文件，避免收敛期间增加容量；末行可能截断，明确披露）；
    /// - 精确 `.log.N`（N≥3）删除（WARN）；精确 `<stem>.stderr.log` 删除（WARN）；
    ///   不匹配/不删除其他包文件或任意 glob；
    /// - 活跃规范化后 len=M 不在构造期滚动：首条非空记录写入前按正常链轮转。
    fn normalize(
        fs: &F,
        paths: &LogPaths,
        max_bytes: u64,
        report: &Arc<dyn ReportSink>,
    ) -> anyhow::Result<()> {
        let entries = fs
            .read_dir(&paths.dir)
            .with_context(|| format!("遍历日志目录失败：{}", paths.dir.display()))?;
        for path in entries {
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            let stem = paths.stem.as_str();
            // 受管三文件优先判定（.log/.log.1/.log.2 不能被序号分支吞掉）
            if paths.managed_name(name).is_some() {
                let meta = fs
                    .metadata_path(&path)
                    .with_context(|| format!("读取日志文件元数据失败：{}", path.display()))?;
                if meta.len > max_bytes {
                    // set_len(M) 保留前 M 字节、删除超限尾部
                    let mut handle = fs
                        .open_append_create(&path)
                        .with_context(|| format!("打开超限日志文件失败：{}", path.display()))?;
                    fs.set_len(&mut handle, max_bytes)
                        .with_context(|| format!("收敛超限日志文件失败：{}", path.display()))?;
                    report.record(ReportEvent::Truncated {
                        stem: stem.to_string(),
                        path: path.clone(),
                        old_len: meta.len,
                        new_len: max_bytes,
                    });
                }
                continue;
            }
            if name == paths.stderr_copy_name() {
                // 前版设计的无界副本：精确清理（G2-1/§4.2.3）
                remove_reporting(fs, &path, stem, report, |stem, path| {
                    ReportEvent::LegacyNormalized { stem, path }
                })?;
                continue;
            }
            if let Some(n) = parse_extra_numbered(name, stem) {
                if n >= 3 {
                    remove_reporting(fs, &path, stem, report, |stem, path| {
                        ReportEvent::OldestRemoved { stem, path }
                    })?;
                }
                continue;
            }
            // 其他文件（含别的包日志）：不匹配、不动
        }
        Ok(())
    }

    /// OpenFresh/Reopen 共用：打开活跃（append+create）→ 读 metadata →
    /// 绝不沿用旧 written；>M 先 set_len(M) 并 WARN，成功才 Ready。
    /// open 成功但 metadata 失败 → 立即 drop 句柄，保持 None/None。
    fn step_open_active(&mut self) -> io::Result<()> {
        let path = self.paths.active.clone();
        let mut handle = match self.fs.open_append_create(&path) {
            Ok(h) => h,
            Err(e) => {
                self.report_io("open_active", &path, &e);
                return Err(e);
            }
        };
        let meta = match self.fs.metadata_handle(&mut handle) {
            Ok(m) => m,
            Err(e) => {
                drop(handle); // 立即 drop：metadata 失败不进入 Ready
                self.report_io("open_meta", &path, &e);
                return Err(e);
            }
        };
        let mut len = meta.len;
        if len > self.limits.max_bytes {
            if let Err(e) = self.fs.set_len(&mut handle, self.limits.max_bytes) {
                drop(handle);
                self.report_io("reopen_truncate", &path, &e);
                return Err(e);
            }
            self.report.record(ReportEvent::Truncated {
                stem: self.paths.stem.clone(),
                path: path.clone(),
                old_len: len,
                new_len: self.limits.max_bytes,
            });
            len = self.limits.max_bytes;
        }
        self.current = Some(handle);
        self.written = Some(len);
        self.phase = Phase::Ready;
        Ok(())
    }

    fn step_remove_oldest(&mut self) -> io::Result<()> {
        let path = self.paths.archive2.clone();
        match self.fs.remove(&path) {
            Ok(()) => {}
            // NotFound 对删除视为已完成（容许历史空洞）
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => {
                self.report_io("remove_oldest", &path, &e);
                return Err(e); // phase 保持 RemoveOldest，原链不动
            }
        }
        self.phase = Phase::MoveOne;
        Ok(())
    }

    fn step_move_one(&mut self) -> io::Result<()> {
        let (from, to) = (self.paths.archive1.clone(), self.paths.archive2.clone());
        match self.fs.rename(&from, &to) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {} // 源缺失：容许空洞
            Err(e) => {
                self.report_io("move_one", &from, &e);
                return Err(e); // phase 保持 MoveOne（.2 已删、.1 原位，仅重试本步）
            }
        }
        self.phase = Phase::MoveActive;
        Ok(())
    }

    fn step_move_active(&mut self) -> io::Result<()> {
        let (from, to) = (self.paths.active.clone(), self.paths.archive1.clone());
        match self.fs.rename(&from, &to) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {} // 源缺失：容许空洞
            Err(e) => {
                self.report_io("move_active", &from, &e);
                return Err(e); // phase 保持 MoveActive（.1 已到 .2、活跃仍在）
            }
        }
        self.phase = Phase::OpenFresh;
        Ok(())
    }

    /// 正常轮转链（整块）：remove .2 → .1→.2 → 活跃→.1 → 打开新活跃。
    /// 只完成有限的一条链，不做递归；每步成功才推进；失败停留当前 phase，
    /// 本条记录丢弃（ERROR 已报告），下一条从同一 phase 继续。
    fn rotate_chain(&mut self) -> io::Result<()> {
        // 前置：先 drop current、清空 written、置 RemoveOldest
        self.current = None;
        self.written = None;
        self.phase = Phase::RemoveOldest;
        self.step_remove_oldest()?;
        self.step_move_one()?;
        self.step_move_active()?;
        self.step_open_active()?;
        self.report.record(ReportEvent::RotateCompleted {
            stem: self.paths.stem.clone(),
        });
        Ok(())
    }

    /// 单步恢复（§4.3.2：「后续每条非空写入触发一次有界的恢复链尝试」）：
    /// 只推进当前 phase 一步；RemoveOldest 失败重试同一 .2、MoveOne 失败仅重试本步、
    /// MoveActive 失败不能把 .2 再前移；OpenFresh/Reopen = open+metadata。
    fn step_recovery(&mut self) -> io::Result<()> {
        match self.phase {
            Phase::RemoveOldest => self.step_remove_oldest(),
            Phase::MoveOne => self.step_move_one(),
            Phase::MoveActive => self.step_move_active(),
            Phase::OpenFresh | Phase::Reopen => self.step_open_active(),
            Phase::Ready => Ok(()),
        }
    }

    /// 写失败/身份失效统一转入 Reopen（drop current、written=None），先报告再返回
    fn fail_to_reopen(&mut self, stage: &'static str, path: &Path, kind: &str) {
        self.current = None;
        self.written = None;
        self.phase = Phase::Reopen;
        self.report.record(ReportEvent::IoFailed {
            stem: self.paths.stem.clone(),
            stage,
            kind: kind.to_string(),
            path: path.to_path_buf(),
        });
    }

    fn report_io(&self, stage: &'static str, path: &Path, e: &io::Error) {
        self.report.record(ReportEvent::IoFailed {
            stem: self.paths.stem.clone(),
            stage,
            kind: e.to_string(),
            path: path.to_path_buf(),
        });
    }

    /// Ready 写前身份检查（§4.3.2）：活跃路径缺失 / nlink=0 / dev-ino 不符 →
    /// 关闭旧 fd、Reopen；检查成功同时以句柄实际长度刷新 written（权威值）。
    fn identity_check(&mut self) -> io::Result<()> {
        let path = self.paths.active.clone();
        let path_meta = match self.fs.metadata_path(&path) {
            Ok(m) => m,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                // 活跃被外部 unlink：不往不可见 inode 持续写
                self.fail_to_reopen("identity_path_missing", &path, "NotFound");
                return Ok(());
            }
            Err(e) => {
                self.fail_to_reopen("identity_path_meta", &path, &e.to_string());
                return Err(e);
            }
        };
        let handle_meta = match self
            .fs
            .metadata_handle(self.current.as_mut().expect("Ready 不变量：current=Some"))
        {
            Ok(m) => m,
            Err(e) => {
                self.fail_to_reopen("identity_handle_meta", &path, &e.to_string());
                return Err(e);
            }
        };
        if handle_meta.nlink == 0
            || path_meta.dev != handle_meta.dev
            || path_meta.ino != handle_meta.ino
        {
            self.fail_to_reopen("identity_mismatch", &path, "dev/ino/nlink 不一致");
            return Ok(());
        }
        self.written = Some(handle_meta.len);
        Ok(())
    }

    /// 单条完整记录写入（含轮转预检、短写循环、Interrupted ≤3 次、错误转 Reopen）。
    /// 返回 Ok 表示已提交或按纪律丢弃（当前记录绝不从头重放）。
    fn write_record(&mut self, bytes: &[u8]) -> io::Result<()> {
        if bytes.is_empty() {
            return Ok(()); // 空记录无操作
        }
        self.sample_queue_drops();
        if self.phase == Phase::Ready {
            // 身份检查失败转 Reopen 后，本条记录继续尝试 reopen、成功则仍写入本条
            self.identity_check()?;
        }
        if self.phase != Phase::Ready {
            // 一次有界恢复尝试；成功进入 Ready 则本条可写，否则本条丢弃
            self.step_recovery()?;
            if self.phase != Phase::Ready {
                return Ok(());
            }
        }
        let len = bytes.len() as u64;
        let written = self.written.expect("Ready 不变量：written=Some");
        if written + len > self.limits.max_bytes {
            // 先归档再整条写入新活跃（相等时恰好填满，下一非空块触发轮转）；
            // 减法比较，禁止加法溢出
            self.rotate_chain()?;
        }
        // 短写循环：按实际 n 更新 written；全程不轮转（剩余长度已预检可容纳）
        let mut total = self.written.expect("Ready 不变量：written=Some");
        let mut interrupted = 0u32;
        let mut offset = 0usize;
        let path = self.paths.active.clone();
        while offset < bytes.len() {
            match self.fs.write(
                self.current.as_mut().expect("Ready 不变量"),
                &bytes[offset..],
            ) {
                Ok(0) => {
                    // write 返回 0（非空输入）：按错误处理，已写前缀保留、丢未写尾部
                    self.fail_to_reopen("write_zero", &path, "write returned 0");
                    return Err(io::Error::new(io::ErrorKind::WriteZero, "write returned 0"));
                }
                Ok(n) => {
                    offset += n;
                    total += n as u64;
                    self.written = Some(total);
                }
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {
                    interrupted += 1;
                    if interrupted > 3 {
                        // 超次数按写失败转 Reopen、报告 ERROR 并丢未写尾部
                        self.fail_to_reopen("write_interrupted", &path, &e.to_string());
                        return Err(e);
                    }
                }
                Err(e) => {
                    self.fail_to_reopen("write_error", &path, &e.to_string());
                    return Err(e);
                }
            }
        }
        Ok(())
    }

    /// flush（§4.3.3）：Ready 时刷当前句柄（失败转 Reopen、已写字节不回滚）；
    /// 其余 phase 无新数据可刷，直接 Ok，不主动执行 rename/open。
    fn flush_record(&mut self) -> io::Result<()> {
        self.sample_queue_drops();
        if self.phase == Phase::Ready {
            let path = self.paths.active.clone();
            if let Err(e) = self.fs.flush(self.current.as_mut().expect("Ready 不变量")) {
                self.fail_to_reopen("flush", &path, &e.to_string());
                return Err(e);
            }
        }
        Ok(())
    }

    /// worker 侧队列丢弃采样：error_counter 差值 → QueueDropped（ERROR）
    fn sample_queue_drops(&mut self) {
        if let Some(dropped) = self.queue.sample() {
            if dropped > 0 {
                self.report.record(ReportEvent::QueueDropped {
                    stem: self.paths.stem.clone(),
                    dropped,
                });
            }
        }
    }
}

impl<F: FileOps> Write for SizeRotatingWriter<F> {
    /// 外层 write：执行完整 record 的内部短写循环后返回原输入 len
    /// （截断属于主动消费策略；错误经 ReportSink 报告后向 worker 返回 Err，
    /// tracing-appender worker 忽略并继续，不重放）。
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        // 二次防御（§4.2.2）：单次输入 > R 用同样的前缀+标记规则，
        // 保护直接 Write 测试/未来调用；不允许一条任意大记录撑破单文件。
        let cap = record_cap(self.limits.max_bytes);
        let truncated = buf.len() > cap;
        let payload = if truncated {
            cap_with_marker(buf, cap, true)
        } else {
            buf.to_vec()
        };
        self.write_record(&payload)?;
        Ok(buf.len())
    }

    /// override write_all：调用同一 record 路径，禁止对截断残部自动再次提交
    fn write_all(&mut self, buf: &[u8]) -> io::Result<()> {
        self.write(buf).map(|_| ())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.flush_record()
    }
}

/// 解析精确遗留序号名 `<stem>.log.N`（N 为纯数字且 ≥1；N≥3 才清理）
fn parse_extra_numbered(name: &str, stem: &str) -> Option<u64> {
    let suffix = name.strip_prefix(stem)?.strip_prefix(".log.")?;
    if suffix.is_empty() || !suffix.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    suffix.parse::<u64>().ok()
}

/// 删除并按事件类型报告；NotFound 视为已完成
fn remove_reporting(
    fs: &impl FileOps,
    path: &Path,
    stem: &str,
    report: &Arc<dyn ReportSink>,
    make_event: fn(String, PathBuf) -> ReportEvent,
) -> anyhow::Result<()> {
    match fs.remove(path) {
        Ok(()) => {
            report.record(make_event(stem.to_string(), path.to_path_buf()));
            Ok(())
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => {
            Err(anyhow::Error::new(e).context(format!("清理遗留日志文件失败：{}", path.display())))
        }
    }
}
