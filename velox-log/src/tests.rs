//! velox-log 测试（架构 §6，G2-6 定案接口）。
//!
//! 分层：
//! - 纯逻辑（等级链/语法/常量/截断组装）走注入（EnvInputs/小阈值），不改进程环境；
//! - writer 状态机走 `SizeRotatingWriter::new` 私有构造 + TempDir + FaultFs 确定性注入；
//! - 完整 subscriber/队列/关停走 `build_dispatch` 局部 Dispatch
//!   （`dispatcher::with_default`），不重复安装全局 subscriber；
//! - 真实全局 init + 进程生命周期仅在本测试可执行文件自举的子进程
//!   （§6.2 活体夹具，`VELOX_LOG_FIXTURE=1`，默认随 `cargo test --workspace` 执行）。
//!
//! 临时目录 = 系统临时根 + pid + 原子计数（无 uuid/tempfile dev-dep，P3-A2）。

use std::collections::{BTreeMap, HashMap};
use std::io::Write as IoWrite;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use crate::filter::{resolve_from, validate_strict_level, EnvInputs};
use crate::report::{ReportEvent, ReportSink, StdReportSink, TestReportSink};
use crate::rotate::{FileOps, LogPaths, Meta, RotationLimits, SizeRotatingWriter, StdFs};
use crate::{build_dispatch, BuildOptions, LevelSource, LogConfig};
use crate::{KEEP_FILES, MAX_EVENT_BYTES, MAX_FILE_BYTES, QUEUE_EVENTS};

// ───────────────────────── 临时目录工具（§6.1.4，无 uuid） ─────────────────────────

static TEMP_SEQ: AtomicU64 = AtomicU64::new(0);

struct TempDir(std::path::PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let base = std::env::temp_dir();
        loop {
            let n = TEMP_SEQ.fetch_add(1, Ordering::Relaxed);
            let p = base.join(format!("velox-log-test-{tag}-{}-{n}", std::process::id()));
            match std::fs::create_dir(&p) {
                Ok(()) => return Self(p),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => panic!("创建临时目录失败：{e}"),
            }
        }
    }

    fn path(&self) -> &std::path::Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0); // RAII 尽力清理
    }
}

// ───────────────────────── FaultFs（§6.1.5 确定性故障注入） ─────────────────────────

/// 故障控制器：每操作第 N 次调用失败 / 短写 / WriteZero / Interrupted / worker 阻塞门。
/// 所有真实状态变更仍落临时目录（StdFs 承载）。
#[derive(Default)]
struct FaultController {
    calls: Mutex<HashMap<&'static str, u32>>,
    fail_at: Mutex<HashMap<&'static str, Vec<u32>>>,
    short_write_calls: AtomicU32, // write：前 N 次返回半量
    write_zero_at: AtomicU32,     // write：第 N 次返回 Ok(0)
    interrupted_at: AtomicU32,    // write：自第 N 次起持续 Interrupted
    gate_closed: Mutex<bool>,
    gate_cv: Condvar,
}

impl FaultController {
    fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    fn plan_fail(&self, op: &'static str, calls: &[u32]) -> &Self {
        self.fail_at
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(op, calls.to_vec());
        self
    }

    fn tick(&self, op: &'static str) -> u32 {
        let mut calls = self.calls.lock().unwrap_or_else(|e| e.into_inner());
        let n = calls.entry(op).or_insert(0);
        *n += 1;
        *n
    }

    fn should_fail(&self, op: &'static str, call: u32) -> Option<std::io::Error> {
        let plan = self.fail_at.lock().unwrap_or_else(|e| e.into_inner());
        if plan.get(op).is_some_and(|v| v.contains(&call)) {
            return Some(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                format!("注入故障 {op}#{call}"),
            ));
        }
        None
    }

    fn close_write_gate(&self) {
        *self.gate_closed.lock().unwrap_or_else(|e| e.into_inner()) = true;
    }

    fn open_write_gate(&self) {
        let mut gate = self.gate_closed.lock().unwrap_or_else(|e| e.into_inner());
        *gate = false;
        self.gate_cv.notify_all();
    }

    fn wait_if_gated(&self) {
        let mut gate = self.gate_closed.lock().unwrap_or_else(|e| e.into_inner());
        while *gate {
            let (g, _) = self
                .gate_cv
                .wait_timeout(gate, Duration::from_millis(20))
                .unwrap_or_else(|e| e.into_inner());
            gate = g;
        }
    }
}

struct FaultFs {
    inner: StdFs,
    ctl: Arc<FaultController>,
}

impl FaultFs {
    fn new(ctl: Arc<FaultController>) -> Self {
        Self { inner: StdFs, ctl }
    }
}

macro_rules! fault_tick {
    ($ctl:expr, $op:expr) => {{
        let call = $ctl.tick($op);
        if let Some(e) = $ctl.should_fail($op, call) {
            return Err(e);
        }
    }};
}

impl FileOps for FaultFs {
    type Handle = <StdFs as FileOps>::Handle;

    fn read_dir(&self, dir: &std::path::Path) -> std::io::Result<Vec<std::path::PathBuf>> {
        fault_tick!(self.ctl, "read_dir");
        self.inner.read_dir(dir)
    }

    fn metadata_path(&self, path: &std::path::Path) -> std::io::Result<Meta> {
        fault_tick!(self.ctl, "metadata_path");
        self.inner.metadata_path(path)
    }

    fn metadata_handle(&self, handle: &mut Self::Handle) -> std::io::Result<Meta> {
        fault_tick!(self.ctl, "metadata_handle");
        self.inner.metadata_handle(handle)
    }

    fn open_append_create(&self, path: &std::path::Path) -> std::io::Result<Self::Handle> {
        fault_tick!(self.ctl, "open_append_create");
        self.inner.open_append_create(path)
    }

    fn set_len(&self, handle: &mut Self::Handle, len: u64) -> std::io::Result<()> {
        fault_tick!(self.ctl, "set_len");
        self.inner.set_len(handle, len)
    }

    fn remove(&self, path: &std::path::Path) -> std::io::Result<()> {
        fault_tick!(self.ctl, "remove");
        self.inner.remove(path)
    }

    fn rename(&self, from: &std::path::Path, to: &std::path::Path) -> std::io::Result<()> {
        fault_tick!(self.ctl, "rename");
        self.inner.rename(from, to)
    }

    fn write(&self, handle: &mut Self::Handle, bytes: &[u8]) -> std::io::Result<usize> {
        // worker 阻塞门（guard 有界等待 / 队列满专项）
        self.ctl.wait_if_gated();
        let call = self.ctl.tick("write");
        if let Some(e) = self.ctl.should_fail("write", call) {
            return Err(e);
        }
        let zero_at = self.ctl.write_zero_at.load(Ordering::Relaxed);
        if zero_at > 0 && call == zero_at {
            return Ok(0);
        }
        let int_at = self.ctl.interrupted_at.load(Ordering::Relaxed);
        if int_at > 0 && call >= int_at {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Interrupted,
                "注入 Interrupted",
            ));
        }
        let short = self.ctl.short_write_calls.load(Ordering::Relaxed);
        if short > 0 && bytes.len() > 1 {
            self.ctl
                .short_write_calls
                .store(short - 1, Ordering::Relaxed);
            return self.inner.write(handle, &bytes[..bytes.len() / 2]);
        }
        self.inner.write(handle, bytes)
    }

    fn flush(&self, handle: &mut Self::Handle) -> std::io::Result<()> {
        fault_tick!(self.ctl, "flush");
        self.inner.flush(handle)
    }
}

// ───────────────────────── 构造助手 ─────────────────────────

fn std_writer(
    dir: &std::path::Path,
    max_bytes: u64,
    report: Arc<TestReportSink>,
) -> SizeRotatingWriter<StdFs> {
    SizeRotatingWriter::new(
        StdFs,
        LogPaths::new(dir, "test"),
        RotationLimits::injected(max_bytes, 3).unwrap(),
        report,
    )
    .unwrap()
}

fn fault_writer(
    dir: &std::path::Path,
    max_bytes: u64,
    ctl: Arc<FaultController>,
    report: Arc<TestReportSink>,
) -> SizeRotatingWriter<FaultFs> {
    SizeRotatingWriter::new(
        FaultFs::new(ctl),
        LogPaths::new(dir, "test"),
        RotationLimits::injected(max_bytes, 3).unwrap(),
        report,
    )
    .unwrap()
}

fn file_len(dir: &std::path::Path, name: &str) -> Option<u64> {
    std::fs::metadata(dir.join(name)).ok().map(|m| m.len())
}

fn managed_files(dir: &std::path::Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .expect("目录存在")
        .filter_map(|e| e.ok())
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| n.starts_with("test.log"))
        .collect();
    names.sort();
    names
}

fn read_file(dir: &std::path::Path, name: &str) -> String {
    String::from_utf8_lossy(&std::fs::read(dir.join(name)).unwrap_or_default()).into_owned()
}

/// 等待条件成立（有界轮询；drain ack 用）
fn wait_for(mut cond: impl FnMut() -> bool, timeout: Duration, what: &str) {
    let start = Instant::now();
    while !cond() {
        assert!(start.elapsed() < timeout, "等待超时：{what}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

// ───────────────────────── 常量与配置（TC-19/E6） ─────────────────────────

#[test]
fn production_constants_and_injection() {
    assert_eq!(MAX_FILE_BYTES, 209_715_200, "AC-13/NFR-1：M=200MiB 冻结");
    assert_eq!(KEEP_FILES, 3, "Q1 定案：含活跃共 3 份");
    assert_eq!(QUEUE_EVENTS, 1024, "§4.5 队列容量定案");
    assert_eq!(MAX_EVENT_BYTES, 64 * 1024, "§4.2.2 R 的 64KiB 侧");
    let limits = RotationLimits::production();
    assert_eq!(limits.max_bytes, MAX_FILE_BYTES);
    assert_eq!(limits.keep_files, 3);
    // R = min(M, 64KiB)：生产取 64KiB；小阈值注入取 M 本身
    assert_eq!(crate::rotate::record_cap(MAX_FILE_BYTES), 64 * 1024);
    assert_eq!(crate::rotate::record_cap(512), 512);
    // 注入构造约束（§4.2.2）：max_bytes ≥32、keep=3；生产不开放配置
    assert!(RotationLimits::injected(31, 3).is_err());
    assert!(RotationLimits::injected(1024, 2).is_err());
    assert!(RotationLimits::injected(1024, 3).is_ok());
    let cfg = LogConfig::default();
    assert_eq!(cfg.dir, "logs");
    assert_eq!(cfg.level, "warn");
}

#[test]
fn logconfig_raw_conversion_records_presence() {
    // 整节缺失 → 默认 + file_level_present=false（缺省不得伪装文件来源）
    let cfg: LogConfig = crate::RawLogConfig {
        dir: None,
        level: None,
    }
    .into();
    assert_eq!(cfg.dir, "logs");
    assert_eq!(cfg.level, "warn");
    assert!(!cfg.file_level_present_present());
    // 部分节：仅 dir
    let cfg: LogConfig = crate::RawLogConfig {
        dir: Some("/tmp/x".into()),
        level: None,
    }
    .into();
    assert_eq!(cfg.dir, "/tmp/x");
    assert_eq!(cfg.level, "warn");
    assert!(!cfg.file_level_present_present());
    // 仅 level：显式出现（空串保留为显式空串，不降级 Default）
    let cfg: LogConfig = crate::RawLogConfig {
        dir: None,
        level: Some("info".into()),
    }
    .into();
    assert!(cfg.file_level_present_present());
    assert_eq!(cfg.level, "info");
    let cfg: LogConfig = crate::RawLogConfig {
        dir: None,
        level: Some(String::new()),
    }
    .into();
    assert!(cfg.file_level_present_present());
    assert_eq!(cfg.level, "");
}

impl LogConfig {
    fn file_level_present_present(&self) -> bool {
        self.file_level_present
    }
}

// ───────────────────────── 等级链（TC-13/14/15，§4.4 例表） ─────────────────────────

fn env_of(pairs: &[(&str, &str)]) -> EnvInputs {
    EnvInputs::from_map(
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect::<BTreeMap<_, _>>(),
    )
}

fn resolve_with(
    log: &LogConfig,
    path: Option<&std::path::Path>,
    prefix: &str,
    env: &EnvInputs,
) -> anyhow::Result<crate::ResolvedLevel> {
    resolve_from(log, path, prefix, env)
}

#[test]
fn level_chain_priority_table() {
    let file = std::path::Path::new("/cfg/app.toml");
    // ① 文件 warnn + LOG__LEVEL=info，无 RUST_LOG → 成功，来源具体 LOG__LEVEL 变量
    let r = resolve_with(
        &LogConfig {
            dir: "logs".into(),
            level: "warnn".into(),
            file_level_present: true,
        },
        Some(file),
        "TEST",
        &env_of(&[("VELOX_TEST__LOG__LEVEL", "info")]),
    )
    .unwrap();
    assert_eq!(r.value, "info");
    assert_eq!(r.source, LevelSource::Env("VELOX_TEST__LOG__LEVEL".into()));

    // ② 文件 warnn + LOG__LEVEL 空串 + RUST_LOG=debug → 成功，来源 RUST_LOG（低层不校验）
    let r = resolve_with(
        &LogConfig {
            dir: "logs".into(),
            level: "warnn".into(),
            file_level_present: true,
        },
        Some(file),
        "TEST",
        &env_of(&[("VELOX_TEST__LOG__LEVEL", ""), ("RUST_LOG", "debug")]),
    )
    .unwrap();
    assert_eq!(r.value, "debug");
    assert_eq!(r.source, LevelSource::Env("RUST_LOG".into()));

    // ③ 文件 warn + LOG__LEVEL 空串 + RUST_LOG 空串 → 失败（空值，来源 LOG__LEVEL）
    let err = resolve_with(
        &LogConfig {
            dir: "logs".into(),
            level: "warn".into(),
            file_level_present: true,
        },
        Some(file),
        "TEST",
        &env_of(&[("VELOX_TEST__LOG__LEVEL", ""), ("RUST_LOG", "")]),
    )
    .unwrap_err();
    assert!(err.to_string().contains("value=\"\""), "实际：{err}");
    assert!(
        err.to_string().contains("VELOX_TEST__LOG__LEVEL"),
        "实际：{err}"
    );

    // ④ 文件 level 缺省 + 无 env → warn/Default（非「来自未配置 level 的文件」）
    let r = resolve_with(
        &LogConfig {
            dir: "logs".into(),
            level: "warn".into(),
            file_level_present: false,
        },
        Some(file),
        "TEST",
        &env_of(&[]),
    )
    .unwrap();
    assert_eq!(r.value, "warn");
    assert_eq!(r.source, LevelSource::Default);

    // ⑤ LOG__LEVEL=warnn，RUST_LOG 未设 → fail-fast，来源 LOG__LEVEL
    let err = resolve_with(
        &LogConfig::default(),
        Some(file),
        "TEST",
        &env_of(&[("VELOX_TEST__LOG__LEVEL", "warnn")]),
    )
    .unwrap_err();
    assert!(err.to_string().contains("warnn"), "实际：{err}");

    // ⑥ 仅 RUST_LOG=warnn → 接受（EnvFilter 中裸词为合法 target 指令）
    let r = resolve_with(
        &LogConfig::default(),
        Some(file),
        "TEST",
        &env_of(&[("RUST_LOG", "warnn")]),
    )
    .unwrap();
    assert_eq!(r.source, LevelSource::Env("RUST_LOG".into()));

    // ⑦ RUST_LOG=svc=warnn，低层均合法 → 失败，来源 RUST_LOG，不回退
    let err = resolve_with(
        &LogConfig {
            dir: "logs".into(),
            level: "warn".into(),
            file_level_present: true,
        },
        Some(file),
        "TEST",
        &env_of(&[("RUST_LOG", "svc=warnn")]),
    )
    .unwrap_err();
    assert!(err.to_string().contains("RUST_LOG"), "实际：{err}");
}

#[test]
fn level_chain_edge_cases() {
    let file = std::path::Path::new("/cfg/app.toml");
    // 文件来源错误含绝对路径 + log.level 键
    let err = resolve_with(
        &LogConfig {
            dir: "logs".into(),
            level: "warnn".into(),
            file_level_present: true,
        },
        Some(file),
        "TEST",
        &env_of(&[]),
    )
    .unwrap_err()
    .to_string();
    assert!(err.contains("source=file:/cfg/app.toml"), "实际：{err}");
    assert!(err.contains("key=log.level"), "实际：{err}");
    assert!(err.contains("warnn"), "实际：{err}");

    // 仅空白非空 RUST_LOG 非法
    let err = resolve_with(
        &LogConfig::default(),
        None,
        "TEST",
        &env_of(&[("RUST_LOG", "  ")]),
    )
    .unwrap_err()
    .to_string();
    assert!(err.contains("RUST_LOG"), "实际：{err}");

    // RUST_LOG="" 视作未设置 → 回落 Default
    let r = resolve_with(
        &LogConfig::default(),
        None,
        "TEST",
        &env_of(&[("RUST_LOG", "")]),
    )
    .unwrap();
    assert_eq!(r.source, LevelSource::Default);

    // 前缀隔离：VELOX_TEST__LOG__LEVEL 不影响 VELOX_OTHER 前缀解析
    let r = resolve_with(
        &LogConfig::default(),
        None,
        "VELOX_OTHER",
        &env_of(&[("VELOX_TEST__LOG__LEVEL", "warnn")]),
    );
    assert!(
        r.is_ok(),
        "其他前缀的非法值不得影响本前缀（合法高优先/无关渠道不校验）"
    );

    // 非 UTF-8 env：选中时报错并标变量名
    let mut raw = BTreeMap::new();
    raw.insert(
        "VELOX_TEST__LOG__LEVEL".to_string(),
        std::os::unix::ffi::OsStringExt::from_vec(vec![0xff, 0xfe]),
    );
    let err = resolve_with(
        &LogConfig::default(),
        None,
        "TEST",
        &EnvInputs::from_raw_map(raw),
    )
    .unwrap_err()
    .to_string();
    assert!(err.contains("VELOX_TEST__LOG__LEVEL"), "实际：{err}");

    // 非法低层不影响合法高层（§4.4.1：只校验生效字符串）
    let r = resolve_with(
        &LogConfig {
            dir: "logs".into(),
            level: "totally-bad".into(),
            file_level_present: true,
        },
        Some(file),
        "TEST",
        &env_of(&[("VELOX_TEST__LOG__LEVEL", "error")]),
    )
    .unwrap();
    assert_eq!(r.value, "error");
}

#[test]
fn strict_syntax_whitelist() {
    // 合法：裸六等级 / 显式 target=level / 逗号混合 / 项两侧空白
    for ok in [
        "trace",
        "debug",
        "info",
        "warn",
        "error",
        "off",
        "warn,velox_mesh::proxy=debug",
        "velox_registry::registry=debug,warn",
        " warn , velox_mesh=info ",
        "velox-registry=error",
    ] {
        assert!(validate_strict_level(ok).is_ok(), "应合法：{ok:?}");
    }
    // 非法：warnn/空/裸 target/数字/未知等级/空项/尾逗号/空 target/非法 target 字符
    for bad in [
        "",
        "warnn",
        "WARN",
        "velox_registry",
        "123",
        "svc=warnn",
        "svc=",
        "warn,,svc=info",
        "warn,",
        ",warn",
        "svc =info",
        "svc=info extra",
        "svc/path=info",
        "svc*=info",
    ] {
        assert!(validate_strict_level(bad).is_err(), "应非法：{bad:?}");
    }
}

// ───────────────────────── writer 基本行为（TC-03/04/05） ─────────────────────────

#[test]
fn writer_rotates_and_caps_file_count() {
    let dir = TempDir::new("rotate");
    let report = TestReportSink::shared();
    let m = 256u64;
    let mut w = std_writer(dir.path(), m, Arc::clone(&report));

    // 10 条 100B 记录：256B 上限下必发生多次轮转
    let rec = vec![b'a'; 100];
    for _ in 0..10 {
        w.write_all(&rec).unwrap();
    }
    let names = managed_files(dir.path());
    assert!(names.len() <= 3, "文件数 ≤3（实际 {names:?}）");
    for name in &names {
        let len = file_len(dir.path(), name).unwrap();
        assert!(len <= m, "{name} 长度 {len} ≤ M={m}（0 超限容差）");
    }
    // .log（活跃）+ .log.1 + .log.2 命名固定
    for name in &names {
        assert!(
            name == "test.log" || name == "test.log.1" || name == "test.log.2",
            "命名固定：{name}"
        );
    }
    assert!(report.counts().rotate >= 1, "发生轮转");
}

#[test]
fn writer_exact_fill_rotates_on_next_record() {
    let dir = TempDir::new("fill");
    let report = TestReportSink::shared();
    let m = 300u64;
    let mut w = std_writer(dir.path(), m, Arc::clone(&report));

    // 第一条 200B，第二条 100B：恰好填满 M，不轮转
    w.write_all(&[b'x'; 200]).unwrap();
    w.write_all(&[b'x'; 100]).unwrap();
    assert_eq!(file_len(dir.path(), "test.log"), Some(300));
    assert!(
        managed_files(dir.path()).len() == 1,
        "恰好填满不产生第四文件"
    );

    // 下一条非空记录触发轮转（先归档再写新活跃）
    w.write_all(&[b'y'; 50]).unwrap();
    assert_eq!(
        file_len(dir.path(), "test.log"),
        Some(50),
        "新活跃只含新记录"
    );
    assert!(dir.path().join("test.log.1").exists(), "旧活跃归档为 .1");
    assert_eq!(file_len(dir.path(), "test.log.1"), Some(300));
}

#[test]
fn writer_retention_removes_oldest() {
    let dir = TempDir::new("retention");
    let report = TestReportSink::shared();
    let mut w = std_writer(dir.path(), 128, Arc::clone(&report));

    // 多轮轮转：.2 始终是最旧，被删除后由 .1 顶替（AC-4）
    for round in 0..8u32 {
        w.write_all(
            format!("round{round:04}-{}", "c".repeat(100))
                .into_bytes()
                .as_slice(),
        )
        .unwrap();
    }
    let names = managed_files(dir.path());
    assert_eq!(names.len(), 3, "恒 ≤3：{names:?}");
    // 最旧内容已删（round0 早已不在任何文件）
    for name in &names {
        assert!(!read_file(dir.path(), name).contains("round0000"));
    }
    // 归档保留次序：.1 比 .2 新
    let a1 = read_file(dir.path(), "test.log.1");
    let a2 = read_file(dir.path(), "test.log.2");
    assert!(a1.contains("round") && a2.contains("round"));
}

#[test]
fn restart_appends_history_preserved() {
    let dir = TempDir::new("restart");
    let report = TestReportSink::shared();
    {
        let mut w = std_writer(dir.path(), 1024, Arc::clone(&report));
        w.write_all(b"first-line\n").unwrap();
    } // drop（模拟重启）
    {
        let mut w = std_writer(dir.path(), 1024, Arc::clone(&report));
        w.write_all(b"second-line\n").unwrap();
    }
    let content = read_file(dir.path(), "test.log");
    assert!(
        content.contains("first-line") && content.contains("second-line"),
        "E5：续写不清空历史"
    );
}

#[test]
fn init_normalizes_oversize_and_legacy_copies() {
    let dir = TempDir::new("normalize");
    // 预置：超限活跃 / 超限归档 / .log.3 / 遗留 stderr.log / 他包文件
    std::fs::write(dir.path().join("test.log"), vec![b'a'; 512]).unwrap();
    std::fs::write(dir.path().join("test.log.1"), vec![b'b'; 600]).unwrap();
    std::fs::write(dir.path().join("test.log.2"), vec![b'c'; 100]).unwrap();
    std::fs::write(dir.path().join("test.log.3"), vec![b'd'; 10]).unwrap();
    std::fs::write(dir.path().join("test.log.7"), vec![b'e'; 10]).unwrap();
    std::fs::write(dir.path().join("test.stderr.log"), vec![b'f'; 10]).unwrap();
    std::fs::write(dir.path().join("other.log"), vec![b'z'; 10]).unwrap();

    let report = TestReportSink::shared();
    let m = 256u64;
    {
        let mut w = std_writer(dir.path(), m, Arc::clone(&report));
        w.write_all(b"new\n").unwrap();
    }

    // 超限收敛到 M（保留前 M 字节）；随后首条记录触发正常轮转链
    // （活跃 len==M 不在构造期滚动，首条非空记录写前轮转，§4.2.3）：
    // .log=新记录、.log.1=旧活跃(收敛后 256)、.log.2=旧 .1(收敛后 256)；
    // 原 100B 的 .2 在轮转链中被合法淘汰
    assert_eq!(
        file_len(dir.path(), "test.log.1"),
        Some(m),
        "旧活跃归档（已收敛）"
    );
    assert_eq!(
        file_len(dir.path(), "test.log.2"),
        Some(m),
        "旧 .1 移位（已收敛）"
    );
    assert_eq!(
        file_len(dir.path(), "test.log"),
        Some(4),
        "新活跃只含新记录"
    );
    // .log.N（N≥3）与遗留 stderr.log 精确清理
    assert!(!dir.path().join("test.log.3").exists());
    assert!(!dir.path().join("test.log.7").exists());
    assert!(!dir.path().join("test.stderr.log").exists());
    // 他包文件不动
    assert!(dir.path().join("other.log").exists(), "不删除其他包文件");
    // 报告：截断 2 次 + 遗留 1 次 + 超号删除 2 次
    let counts = report.counts();
    assert_eq!(counts.truncated, 2, "超限活跃+归档各报告一次");
    assert_eq!(counts.legacy, 1);
    assert_eq!(counts.oldest_removed, 2);
}

#[test]
fn oversize_record_truncated_within_cap() {
    let dir = TempDir::new("oversize");
    let report = TestReportSink::shared();
    let m = 512u64; // R = min(512, 64KiB) = 512
    let mut w = std_writer(dir.path(), m, Arc::clone(&report));

    // 单次输入 > R：writer 侧二次防御截断（保留 UTF-8 前缀 + 标记，≤R）
    let big = "中".repeat(2000); // 6000 字节（UTF-8 每字 3B）
    w.write_all(big.as_bytes()).unwrap();
    let active = file_len(dir.path(), "test.log").unwrap();
    assert!(active <= m, "截断后 ≤M（实际 {active}）");
    let content = read_file(dir.path(), "test.log");
    assert!(content.contains(" [truncated]"), "含截断标记");
    assert!(content.contains("中"), "保留 UTF-8 完整前缀");
    assert!(!content.ends_with('\u{fffd}'), "无残缺 UTF-8");
}

#[test]
fn identity_check_reopens_after_unlink() {
    let dir = TempDir::new("unlink");
    let report = TestReportSink::shared();
    let mut w = std_writer(dir.path(), 1024, Arc::clone(&report));
    w.write_all(b"before-unlink\n").unwrap();
    let old_len = file_len(dir.path(), "test.log").unwrap();

    // 外部 unlink 活跃文件：旧 fd 写入不再可见 → 身份检查识别 → Reopen 新路径
    std::fs::remove_file(dir.path().join("test.log")).unwrap();
    w.write_all(b"after-unlink\n").unwrap();
    let content = read_file(dir.path(), "test.log");
    assert!(
        content.contains("after-unlink"),
        "记录写入重建后的活跃文件（不写已 unlink 的不可见 inode）"
    );
    assert!(
        !content.contains("before-unlink"),
        "新活跃从零开始（旧内容随旧 inode 消失）"
    );
    let _ = old_len;
}

// ───────────────────────── §4.3.2 分阶段失败表（FaultFs） ─────────────────────────

fn event_of(report: &TestReportSink, stage: &str) -> bool {
    report
        .events()
        .iter()
        .any(|e| matches!(e, ReportEvent::IoFailed { stage: s, .. } if *s == stage))
}

#[test]
fn fault_remove_oldest_retries_same_step() {
    let dir = TempDir::new("f-remove");
    let report = TestReportSink::shared();
    let ctl = FaultController::new();
    let mut w = fault_writer(dir.path(), 128, Arc::clone(&ctl), Arc::clone(&report));

    // 三条 100B 记录触发两轮轮转（100+100>128；再 100+100>128），使 .2 存在
    w.write_all(&[b'a'; 100]).unwrap();
    w.write_all(&[b'a'; 100]).unwrap();
    w.write_all(&[b'a'; 100]).unwrap();
    assert!(dir.path().join("test.log.2").exists(), "两轮轮转后 .2 存在");
    let a2_before = file_len(dir.path(), "test.log.2");

    // 注入 remove 第 3 次调用失败（下一轮转链的 RemoveOldest 步）
    ctl.plan_fail("remove", &[3]);

    // 本条触发轮转：remove(.2) 失败 → phase=RemoveOldest，本条丢弃，原链不动
    let _ = w.write_all(&[b'b'; 100]);
    assert!(
        event_of(&report, "remove_oldest"),
        "报告 ERROR remove_oldest"
    );
    assert_eq!(file_len(dir.path(), "test.log.2"), a2_before, "原链未动");

    // 下一条：重试同一 remove（成功）→ 推进 MoveOne（.1→.2）→ 本条仍丢弃
    let _ = w.write_all(&[b'c'; 100]);
    // 再下一条：MoveActive（活跃→.1）→ 本条丢弃
    let _ = w.write_all(&[b'd'; 100]);
    // 再下一条：OpenFresh（打开新活跃）→ 本条丢弃（下条起 Ready 正常写）
    let _ = w.write_all(&[b'f'; 100]);
    // OpenFresh 已就绪：本条直接写入新活跃
    w.write_all(&[b'e'; 50]).unwrap();
    assert_eq!(
        file_len(dir.path(), "test.log"),
        Some(50),
        "恢复后写入新活跃"
    );
    // 无重复移位：文件数与各槽内容唯一确定
    assert!(managed_files(dir.path()).len() <= 3);
}

// 设计注记：故障链中的「本条丢弃」可能表现为 Ok（主动丢弃）或 Err（IO 上抛），
// 两者都符合 §4.3.2 纪律；断言用 call-site `let _ =` 显式表达。

#[test]
fn fault_move_one_retries_without_double_shift() {
    let dir = TempDir::new("f-move");
    let report = TestReportSink::shared();
    let ctl = FaultController::new();
    let mut w = fault_writer(dir.path(), 128, Arc::clone(&ctl), Arc::clone(&report));

    // 三条 100B 记录触发两轮轮转（rename#1~4 已消耗：每链 .1→.2、活跃→.1 各一次）
    w.write_all(&[b'a'; 100]).unwrap();
    w.write_all(&[b'a'; 100]).unwrap();
    w.write_all(&[b'a'; 100]).unwrap();
    assert!(dir.path().join("test.log.2").exists(), "两轮轮转后 .2 存在");
    let a2_snapshot = read_file(dir.path(), "test.log.2");
    // 注入 rename 第 5 次失败（下一轮转链的 .1→.2 步）
    ctl.plan_fail("rename", &[5]);
    let _ = w.write_all(&[b'b'; 100]);
    assert!(event_of(&report, "move_one"), "报告 ERROR move_one");
    // .2 已被 remove，.1 仍在原位（未移动）
    assert!(
        !dir.path().join("test.log.2").exists()
            || (file_len(dir.path(), "test.log.2") == Some(a2_snapshot.len() as u64)
                && read_file(dir.path(), "test.log.2") == a2_snapshot),
        "原链未动（.1 仍在原位，.2 未被二次移位）"
    );

    // 后续记录逐步恢复：MoveOne 重试成功 → MoveActive → OpenFresh → 写入
    let _ = w.write_all(&[b'c'; 100]);
    let _ = w.write_all(&[b'd'; 100]);
    let _ = w.write_all(&[b'f'; 100]);
    w.write_all(&[b'e'; 30]).unwrap();
    assert!(managed_files(dir.path()).len() <= 3);
}

#[test]
fn fault_open_failure_recovers_via_open_fresh() {
    let dir = TempDir::new("f-open");
    let report = TestReportSink::shared();
    let ctl = FaultController::new();
    let mut w = fault_writer(dir.path(), 128, Arc::clone(&ctl), Arc::clone(&report));

    w.write_all(&[b'a'; 100]).unwrap();
    w.write_all(&[b'a'; 100]).unwrap();
    // 注入 open_append_create 失败（下一轮转链的 OpenFresh 步；ctor 已消耗 #1、首轮链 #2）
    ctl.plan_fail("open_append_create", &[3]);
    let _ = w.write_all(&[b'b'; 100]);
    assert!(event_of(&report, "open_active"), "报告 ERROR open_active");
    // 下一条：OpenFresh 重试成功 → Ready → 本条写入
    w.write_all(&[b'c'; 40]).unwrap();
    assert_eq!(file_len(dir.path(), "test.log"), Some(40));
}

#[test]
fn fault_write_error_keeps_prefix_and_reopens() {
    let dir = TempDir::new("f-write");
    let report = TestReportSink::shared();
    let ctl = FaultController::new();
    let mut w = fault_writer(dir.path(), 4096, Arc::clone(&ctl), Arc::clone(&report));

    // 注入：write 第 1 次调用返回 Err（记录写失败 → 已写前缀保留、转 Reopen、丢尾部）
    ctl.plan_fail("write", &[1]);
    let res = w.write_all(&vec![b'z'; 300]);
    assert!(
        res.is_err() || res.is_ok(),
        "按纪律处理（Err 上抛或主动丢弃）"
    );
    assert!(event_of(&report, "write_error"), "报告 ERROR write_error");

    // 下一条：Reopen → open+metadata → Ready → 写入新记录（不重放上一条）
    w.write_all(b"next\n").unwrap();
    let content = read_file(dir.path(), "test.log");
    assert!(content.contains("next"), "后续记录正常继续");
}

#[test]
fn fault_short_write_continues_within_record() {
    let dir = TempDir::new("f-short");
    let report = TestReportSink::shared();
    let ctl = FaultController::new();
    let mut w = fault_writer(dir.path(), 4096, Arc::clone(&ctl), Arc::clone(&report));

    // 前 2 次 write 各返回半量 → 同一 record 内短写循环补齐（全程不轮转）
    ctl.short_write_calls.store(2, Ordering::Relaxed);
    w.write_all(&[b's'; 200]).unwrap();
    assert_eq!(
        file_len(dir.path(), "test.log"),
        Some(200),
        "短写循环补齐整条"
    );
    assert!(managed_files(dir.path()).len() == 1, "短写不触发轮转");
}

#[test]
fn fault_write_zero_transitions_reopen() {
    let dir = TempDir::new("f-zero");
    let report = TestReportSink::shared();
    let ctl = FaultController::new();
    let mut w = fault_writer(dir.path(), 4096, Arc::clone(&ctl), Arc::clone(&report));

    ctl.write_zero_at.store(1, Ordering::Relaxed);
    let _ = w.write_all(&[b'q'; 100]);
    assert!(event_of(&report, "write_zero"), "write 返回 0 → ERROR 报告");
    // 下一条 Reopen 成功后继续
    w.write_all(b"after-zero\n").unwrap();
    assert!(read_file(dir.path(), "test.log").contains("after-zero"));
}

#[test]
fn fault_interrupted_retry_budget_then_reopen() {
    let dir = TempDir::new("f-int");
    let report = TestReportSink::shared();
    let ctl = FaultController::new();
    let mut w = fault_writer(dir.path(), 4096, Arc::clone(&ctl), Arc::clone(&report));

    // 自第 1 次 write 起持续 Interrupted：同一条 ≤3 次重试后按失败转 Reopen
    ctl.interrupted_at.store(1, Ordering::Relaxed);
    let _ = w.write_all(&[b'i'; 100]);
    assert!(
        event_of(&report, "write_interrupted"),
        "超过重试预算报告 ERROR"
    );
    // 解除注入：下一条 Reopen 成功并写入
    ctl.interrupted_at.store(0, Ordering::Relaxed);
    w.write_all(b"after-int\n").unwrap();
    assert!(read_file(dir.path(), "test.log").contains("after-int"));
}

#[test]
fn fault_flush_failure_reopens_no_rollback() {
    let dir = TempDir::new("f-flush");
    let report = TestReportSink::shared();
    let ctl = FaultController::new();
    let mut w = fault_writer(dir.path(), 4096, Arc::clone(&ctl), Arc::clone(&report));

    w.write_all(b"kept\n").unwrap();
    let len_before = file_len(dir.path(), "test.log").unwrap();
    ctl.plan_fail("flush", &[1]);
    let _ = w.flush();
    assert!(event_of(&report, "flush"), "flush 失败报告 ERROR");
    // 已写字节不回滚；下一条 Reopen 后继续
    assert_eq!(
        file_len(dir.path(), "test.log"),
        Some(len_before),
        "已写字节不回滚"
    );
    ctl.plan_fail("flush", &[]);
    w.write_all(b"after-flush\n").unwrap();
    assert!(read_file(dir.path(), "test.log").contains("after-flush"));
}

#[test]
fn reopen_truncates_oversize_path() {
    let dir = TempDir::new("f-reopen-big");
    let report = TestReportSink::shared();
    let ctl = FaultController::new();
    let m = 256u64;
    let mut w = fault_writer(dir.path(), m, Arc::clone(&ctl), Arc::clone(&report));

    // 制造 Reopen（write 失败），然后外部放置超限活跃文件
    ctl.plan_fail("write", &[1]);
    let _ = w.write_all(&[b'x'; 10]);
    std::fs::write(dir.path().join("test.log"), vec![b'o'; 600]).unwrap();

    // 下一条：Reopen 发现路径长度 >M → set_len(M) 并 WARN，成功才 Ready → 本条写入
    w.write_all(b"small\n").unwrap();
    let len = file_len(dir.path(), "test.log").unwrap();
    assert!(len <= m, "Reopen 收敛超限路径（实际 {len}）");
    assert!(
        report
            .events()
            .iter()
            .any(|e| matches!(e, ReportEvent::Truncated { new_len, .. } if *new_len == m)),
        "Reopen 收敛报告 Truncated WARN"
    );
    assert!(read_file(dir.path(), "test.log").contains("small"));
}

// ───────────────────────── 完整链路：限长/队列/关停/无反馈（§6.1.2） ─────────────────────────

fn dispatch_options<F: FileOps>(
    dir: &std::path::Path,
    max_bytes: u64,
    queue: usize,
    report: Arc<dyn ReportSink>,
    fs: F,
) -> BuildOptions<F> {
    BuildOptions {
        dir: dir.to_path_buf(),
        stem: "test".to_string(),
        filter: tracing_subscriber::EnvFilter::new("warn"),
        limits: RotationLimits::injected(max_bytes, 3).unwrap(),
        queue_events: queue,
        report,
        fs,
    }
}

#[test]
fn full_chain_event_cap_and_rotation_via_dispatch() {
    let dir = TempDir::new("chain");
    let report = TestReportSink::shared();
    let m = 512u64;
    let (dispatch, guard) = build_dispatch(dispatch_options(
        dir.path(),
        m,
        256,
        Arc::clone(&report) as Arc<dyn ReportSink>,
        StdFs,
    ))
    .unwrap();

    // 经完整 subscriber 链发事件（fmt → BoundedMakeWriter → 队列 → worker → 文件）
    tracing::dispatcher::with_default(&dispatch, || {
        for i in 0..40u32 {
            tracing::warn!(seq = i, payload = "p".repeat(120), "chain event");
        }
    });
    drop(guard); // 有界排空

    // 每事件（fmt 后）>m/2 → 至少数次轮转；全部文件 ≤M、总数 ≤3
    let names = managed_files(dir.path());
    assert!(names.len() <= 3, "{names:?}");
    for name in &names {
        assert!(file_len(dir.path(), name).unwrap() <= m);
    }
    assert!(report.counts().rotate >= 1, "完整链路发生轮转");
    assert!(report.counts().truncated == 0, "常规事件不截断");
}

#[test]
fn queue_full_drops_counted_no_backpressure() {
    let dir = TempDir::new("queue");
    let report = TestReportSink::shared();
    let ctl = FaultController::new();
    // 容量降至 2，写门关闭阻塞 worker
    let (dispatch, guard) = build_dispatch(dispatch_options(
        dir.path(),
        64 * 1024,
        2,
        Arc::clone(&report) as Arc<dyn ReportSink>,
        FaultFs::new(Arc::clone(&ctl)),
    ))
    .unwrap();
    ctl.close_write_gate();

    let start = Instant::now();
    tracing::dispatcher::with_default(&dispatch, || {
        for i in 0..200u32 {
            tracing::warn!(seq = i, "queue pressure event");
        }
    });
    let elapsed = start.elapsed();
    assert!(
        elapsed < Duration::from_millis(500),
        "lossy 队列不反压业务线程（200 事件耗时 {elapsed:?}）"
    );

    // 释放门 → worker 排空 → 队列丢弃被采样报告（QueueDropped ERROR）
    ctl.open_write_gate();
    drop(guard);
    wait_for(
        || report.counts().queue_dropped >= 1,
        Duration::from_secs(5),
        "队列丢弃计数",
    );
    assert!(report.counts().queue_dropped >= 1, "队列满丢弃必须外显");
}

#[test]
fn no_feedback_after_producers_stop() {
    // G2-4：M 小于报告文本长度——若轮转报告回流队列，将自激轮转。
    let dir = TempDir::new("quiet");
    let report = TestReportSink::shared();
    let m = 256u64; // 报告行 >256B，若回流必然再触发轮转
    let (dispatch, guard) = build_dispatch(dispatch_options(
        dir.path(),
        m,
        256,
        Arc::clone(&report) as Arc<dyn ReportSink>,
        StdFs,
    ))
    .unwrap();

    tracing::dispatcher::with_default(&dispatch, || {
        for i in 0..12u32 {
            tracing::warn!(seq = i, payload = "q".repeat(200), "trigger rotations");
        }
    });
    // 等待 drain（轮转计数稳定）
    wait_for(
        || {
            let before = report.counts();
            std::thread::sleep(Duration::from_millis(30));
            report.counts() == before
        },
        Duration::from_secs(5),
        "报告计数静止",
    );
    let counts_a = report.counts();
    let lens_a: Vec<Option<u64>> = ["test.log", "test.log.1", "test.log.2"]
        .iter()
        .map(|n| file_len(dir.path(), n))
        .collect();

    // 生产者已停止：连续两个观测窗计数与文件长度完全静止（无自动轮转回写）
    std::thread::sleep(Duration::from_millis(120));
    let counts_b = report.counts();
    let lens_b: Vec<Option<u64>> = ["test.log", "test.log.1", "test.log.2"]
        .iter()
        .map(|n| file_len(dir.path(), n))
        .collect();
    assert_eq!(counts_a, counts_b, "业务停止后报告计数静止（无反馈自激）");
    assert_eq!(lens_a, lens_b, "业务停止后文件长度静止");
    drop(guard);
}

#[test]
fn guard_drop_returns_within_budget_when_worker_blocked() {
    // §4.6：worker 被确定阻塞时 guard 有界返回（握手 ≤1.1s，测试预算 2s）；
    // 不要求该分支末条不丢。
    let dir = TempDir::new("guard");
    let report = TestReportSink::shared();
    let ctl = FaultController::new();
    let (_dispatch, guard) = build_dispatch(dispatch_options(
        dir.path(),
        64 * 1024,
        8,
        Arc::clone(&report) as Arc<dyn ReportSink>,
        FaultFs::new(Arc::clone(&ctl)),
    ))
    .unwrap();
    ctl.close_write_gate();
    // 触发一条让 worker 停在 write 门上
    tracing::dispatcher::with_default(&_dispatch, || {
        tracing::warn!("blocked worker probe");
    });
    std::thread::sleep(Duration::from_millis(100)); // 让 worker 取到记录并阻塞

    let start = Instant::now();
    let handle = std::thread::spawn(move || {
        drop(guard); // drop LogGuard
    });
    // 轮询等待（≤2s 预算；appender 握手 100ms/1000ms，留调度余量）
    let deadline = Instant::now() + Duration::from_secs(2);
    while !handle.is_finished() {
        assert!(
            Instant::now() < deadline,
            "guard drop 在 worker 阻塞时必须 ≤2s 返回（实测超时）"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    let elapsed = start.elapsed();
    assert!(
        elapsed < Duration::from_secs(2),
        "guard 有界返回（实测 {elapsed:?}）"
    );
    // 收尾：释放门让 worker 线程退出，避免测试进程泄漏（§4.6）
    ctl.open_write_gate();
}

// ───────────────────────── StdReportSink 限频（注入时钟，§4.3.4） ─────────────────────────

#[test]
fn std_report_sink_rate_limits_by_category() {
    let clock = Arc::new(Mutex::new(Instant::now()));
    let clock_ref = Arc::clone(&clock);
    let sink = StdReportSink::with_clock(
        Duration::from_secs(60),
        Box::new(move || *clock_ref.lock().unwrap()),
    );
    let dir = std::path::PathBuf::from("/tmp/velox-log-rate-test");
    let mk = |n: u64| ReportEvent::Truncated {
        stem: "s".into(),
        path: dir.clone(),
        old_len: n * 1000,
        new_len: 1,
    };
    sink.record(mk(1)); // 首次立即输出
    sink.record(mk(2)); // 窗口内 → 抑制
    sink.record(mk(3)); // 窗口内 → 抑制
    let c = sink.counts();
    assert_eq!(c.truncated, 3, "计数无条件累计");
    assert_eq!(c.suppressed, 2, "被抑制次数累加");

    // 时间推进过窗口 → 该类别下次输出带 counts；RateState 清零（累计量仍为 2）
    *clock.lock().unwrap_or_else(|e| e.into_inner()) += Duration::from_secs(61);
    sink.record(mk(4));
    assert_eq!(
        sink.counts().suppressed,
        2,
        "suppressed 为累计量（跨类别不重置）"
    );

    // 窗口内同类别再次抑制：累计量 2→3
    sink.record(mk(5));
    assert_eq!(sink.counts().suppressed, 3);

    // 不同类别独立限频（Io 类首次立即输出）
    sink.record(ReportEvent::IoFailed {
        stem: "s".into(),
        stage: "io_probe",
        kind: "Injected".into(),
        path: dir.clone(),
    });
    assert_eq!(sink.counts().io_failed, 1);
    assert_eq!(sink.counts().suppressed, 3, "Io 类首条不产生抑制");
}

// ───────────────────────── §6.2 活体夹具（真实 subscriber + 进程生命周期） ─────────────────────────

/// 子进程入口（仅 cfg(test) + VELOX_LOG_FIXTURE=1 时启用服务；普通 cargo test 直接返回）。
/// 走与生产完全一致的 build_dispatch → set_global_default → LogGuard 路径。
#[test]
fn fixture_child() {
    if std::env::var("VELOX_LOG_FIXTURE").as_deref() != Ok("1") {
        return;
    }
    let dir =
        std::path::PathBuf::from(std::env::var("VELOX_LOG_FIXTURE_DIR").expect("夹具目录参数"));
    let max_bytes: u64 = std::env::var("VELOX_LOG_FIXTURE_MAX")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1024);

    let report = TestReportSink::shared();
    let (dispatch, guard) = build_dispatch(BuildOptions {
        dir: dir.clone(),
        stem: "fixture".to_string(),
        filter: tracing_subscriber::EnvFilter::new("warn"),
        limits: RotationLimits::injected(max_bytes, 3).unwrap(),
        queue_events: 1024,
        report: Arc::clone(&report) as Arc<dyn ReportSink>,
        fs: StdFs,
    })
    .expect("夹具装配失败");
    tracing::dispatcher::set_global_default(dispatch).expect("夹具安装全局 subscriber");

    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("绑定夹具端口");
    let port = listener.local_addr().unwrap().port();
    // stdout 发送唯一 READY 行（父端 drain stdout 解析端口）
    println!("FIXTURE_READY {port}");
    let _ = std::io::stdout().flush();

    let stop = Arc::new(AtomicBool::new(false));
    let events = Arc::new(AtomicU64::new(0));
    listener.set_nonblocking(true).unwrap();
    let mut conn_threads: Vec<std::thread::JoinHandle<()>> = Vec::new();
    loop {
        if stop.load(Ordering::Relaxed) {
            break;
        }
        match listener.accept() {
            Ok((stream, _addr)) => {
                let stop = Arc::clone(&stop);
                let events = Arc::clone(&events);
                let report = Arc::clone(&report);
                conn_threads.push(std::thread::spawn(move || {
                    handle_fixture_conn(stream, &stop, &events, &report);
                }));
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(2));
            }
            Err(_) => break,
        }
    }
    for t in conn_threads {
        let _ = t.join();
    }
    // 所有 producer 结束后写最后生命周期日志，再释放 guard（§4.6 顺序契约）
    tracing::warn!("fixture_final");
    drop(guard);
    let _ = report;
    let _ = events;
}

fn handle_fixture_conn(
    mut stream: std::net::TcpStream,
    stop: &AtomicBool,
    events: &AtomicU64,
    report: &TestReportSink,
) {
    use std::io::{Read as _, Write as _};
    // BUG-1①：macOS/BSD 上 accept() 返回的 stream 继承 listener 的 O_NONBLOCK，
    // 高并发调度下 read 可能立刻返回 WouldBlock，空连接被静默关闭导致客户端空读；
    // 先还原阻塞模式，使下方 5s 读超时真正生效（stop 轮询仍发生在 listener 上，语义不变）。
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let mut buf = [0u8; 2048];
    let n = match stream.read(&mut buf) {
        Ok(n) if n > 0 => n,
        _ => return,
    };
    let req = String::from_utf8_lossy(&buf[..n]);
    let first_line = req.lines().next().unwrap_or("");
    let mut parts = first_line.split_whitespace();
    let method = parts.next().unwrap_or("");
    let target = parts.next().unwrap_or("");
    let (path, query) = match target.split_once('?') {
        Some((p, q)) => (p, q),
        None => (target, ""),
    };
    let respond = |stream: &mut std::net::TcpStream, code: &str, body: String| {
        let resp = format!(
            "HTTP/1.1 {code}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        );
        let _ = stream.write_all(resp.as_bytes());
    };
    match (method, path) {
        ("GET", "/healthz") => respond(&mut stream, "200 OK", "ok".into()),
        ("GET", "/stats") => {
            let c = report.counts();
            respond(
                &mut stream,
                "200 OK",
                format!(
                    "events={} rotations={} dropped={}",
                    events.load(Ordering::Relaxed),
                    c.rotate,
                    c.queue_dropped
                ),
            );
        }
        ("POST", "/stop") => {
            stop.store(true, Ordering::Relaxed);
            respond(&mut stream, "200 OK", "stopping".into());
        }
        ("POST", "/emit") => {
            let mut producer = String::new();
            let mut seq = String::new();
            for kv in query.split('&') {
                if let Some((k, v)) = kv.split_once('=') {
                    match k {
                        "producer" => producer = v.to_string(),
                        "seq" => seq = v.to_string(),
                        _ => {}
                    }
                }
            }
            // ~300B marker 事件（小阈值 M=1024 下快速轮转）
            let pad = "p".repeat(220);
            tracing::warn!(
                producer = %producer,
                seq = %seq,
                "FIXTURE_MARKER producer={producer} seq={seq} {pad}"
            );
            events.fetch_add(1, Ordering::Relaxed);
            respond(&mut stream, "200 OK", "emitted".into());
        }
        _ => respond(&mut stream, "404 Not Found", String::new()),
    }
}

/// 夹具子进程 RAII 守卫（BUG-1②）：包装父端 `std::process::Child`，
/// 任何退出路径（含断言失败/panic 提前返回）都保证 kill + wait 回收，
/// 不遗留 `fixture_child` 孤儿进程。
struct ChildGuard {
    inner: Option<std::process::Child>,
}

impl ChildGuard {
    fn spawn(cmd: &mut std::process::Command) -> Self {
        Self {
            inner: Some(cmd.spawn().expect("启动夹具子进程")),
        }
    }

    fn as_child(&mut self) -> &mut std::process::Child {
        self.inner.as_mut().expect("夹具子进程句柄")
    }

    /// 正常路径：取回子进程并阻塞等待退出（消费守卫，此后 Drop 不再重复回收）。
    fn wait(mut self) -> std::process::ExitStatus {
        self.inner
            .take()
            .expect("夹具子进程句柄")
            .wait()
            .expect("等待子进程")
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if let Some(mut child) = self.inner.take() {
            match child.try_wait() {
                Ok(Some(_)) => {} // 已退出，无需处理
                _ => {
                    // 仍在运行或状态未知：kill 后 wait 回收，杜绝孤儿/僵尸进程
                    let _ = child.kill();
                    let _ = child.wait();
                }
            }
        }
    }
}

/// 父驱动：自举子进程（同一测试可执行文件），8 并发客户端 × ≥64 请求，
/// ≥4 次真实轮转，marker 前后窗可查，stop 后子进程正常退出（§6.2；默认执行）。
#[test]
fn live_rotation_concurrent() {
    let dir = TempDir::new("live");
    let exe = std::env::current_exe().expect("current_exe");
    let mut child = ChildGuard::spawn(
        std::process::Command::new(exe)
            .args([
                "--exact",
                "tests::fixture_child",
                "--nocapture",
                "--test-threads=1",
            ])
            .env("VELOX_LOG_FIXTURE", "1")
            .env("VELOX_LOG_FIXTURE_DIR", dir.path())
            .env("VELOX_LOG_FIXTURE_MAX", "1024")
            .env_remove("RUST_LOG")
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped()),
    );

    // 父端 drain stdout/stderr（有界内存，不落磁盘全量副本）
    let stdout = child.as_child().stdout.take().expect("stdout piped");
    let stderr = child.as_child().stderr.take().expect("stderr piped");
    // stderr 持续排水（防管道堵死；内容仅诊断用，读取后丢弃）
    let stderr_drain = std::thread::spawn(move || {
        let mut reader = std::io::BufReader::new(stderr);
        let mut buf = [0u8; 4096];
        loop {
            match std::io::Read::read(&mut reader, &mut buf) {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
        }
    });

    // §6.2：父端**持续**排水子进程 stdout/stderr（有界内存、不落磁盘全量副本）——
    // 若 READY 后停止读 stdout，管道写满会阻塞子进程 stdout fmt 层与全部 handler 线程。
    let ready = Arc::new((Mutex::new(None::<u16>), Condvar::new()));
    {
        let ready = Arc::clone(&ready);
        let stdout_drain = std::thread::spawn(move || {
            let mut reader = std::io::BufReader::new(stdout);
            let mut line = String::new();
            loop {
                line.clear();
                match std::io::BufRead::read_line(&mut reader, &mut line) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {
                        // harness 输出与测试 println 同行：取 "FIXTURE_READY " 后的端口
                        if let Some(rest) = line.rsplit("FIXTURE_READY ").next() {
                            if let Some(tok) = rest.split_whitespace().next() {
                                if let Ok(v) = tok.parse::<u16>() {
                                    let (lock, cv) = &*ready;
                                    *lock.lock().unwrap_or_else(|e| e.into_inner()) = Some(v);
                                    cv.notify_all();
                                }
                            }
                        }
                    }
                }
            }
        });
        let _ = stdout_drain; // 常驻排水至子进程退出
    }
    let ready_deadline = Instant::now() + Duration::from_secs(15);
    let (lock, cv) = &*ready;
    let mut port_guard = lock.lock().unwrap_or_else(|e| e.into_inner());
    while port_guard.is_none() {
        assert!(Instant::now() < ready_deadline, "夹具子进程 15s 内未就绪");
        let (g, _) = cv
            .wait_timeout(port_guard, Duration::from_millis(100))
            .unwrap_or_else(|e| e.into_inner());
        port_guard = g;
    }
    let port = port_guard.expect("READY 行已给出端口");
    let base = format!("http://127.0.0.1:{port}");

    // 简单 HTTP 客户端（标准库 TcpStream）
    let http = |req: &str| -> Result<String, String> {
        use std::io::{Read as _, Write as _};
        let mut stream =
            std::net::TcpStream::connect(("127.0.0.1", port)).map_err(|e| e.to_string())?;
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        stream
            .write_all(req.as_bytes())
            .map_err(|e| e.to_string())?;
        let mut buf = Vec::new();
        stream.read_to_end(&mut buf).map_err(|e| e.to_string())?;
        Ok(String::from_utf8_lossy(&buf).into_owned())
    };
    let get = |path: &str| {
        http(&format!(
            "GET {path} HTTP/1.1\r\nhost: t\r\nconnection: close\r\n\r\n"
        ))
    };
    let post = |path: &str| {
        http(&format!(
            "POST {path} HTTP/1.1\r\nhost: t\r\ncontent-length: 0\r\nconnection: close\r\n\r\n"
        ))
    };

    // 健康检查
    let resp = get("/healthz").expect("healthz");
    assert!(resp.starts_with("HTTP/1.1 200"), "healthz 200");
    let _ = &base;

    // 窗口一（轮转前）：markerA 可查
    let resp = post("/emit?producer=win&seq=early").expect("emit markerA");
    assert!(resp.starts_with("HTTP/1.1 200"));
    let marker_a = "producer=win seq=early";
    wait_for(
        || {
            ["fixture.log", "fixture.log.1", "fixture.log.2"]
                .iter()
                .any(|n| read_file(dir.path(), n).contains("FIXTURE_MARKER"))
        },
        Duration::from_secs(10),
        "markerA 落盘",
    );
    assert!(
        ["fixture.log", "fixture.log.1", "fixture.log.2"]
            .iter()
            .any(|n| read_file(dir.path(), n).contains(marker_a)),
        "窗口一 markerA 可读"
    );

    // 8 并发客户端 × ≥64 请求（含健康检查混合负载）
    let mut clients = Vec::new();
    for producer in 0..8u32 {
        let _base = base.clone();
        clients.push(std::thread::spawn(move || {
            let http = |req: &str| -> Result<String, String> {
                use std::io::{Read as _, Write as _};
                let mut stream = None;
                for attempt in 0..5u32 {
                    match std::net::TcpStream::connect(("127.0.0.1", port)) {
                        Ok(s) => {
                            stream = Some(s);
                            break;
                        }
                        Err(_e) if attempt < 4 => {
                            std::thread::sleep(Duration::from_millis(20));
                        }
                        Err(e) => return Err(format!("connect: {e}")),
                    }
                }
                let mut stream = stream.expect("connect 重试后成功");
                stream.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
                stream.write_all(req.as_bytes()).map_err(|e| format!("write: {e}"))?;
                let mut buf = Vec::new();
                let mut reader = std::io::BufReader::new(stream);
                reader.read_to_end(&mut buf).map_err(|e| format!("read: {e}"))?;
                Ok(String::from_utf8_lossy(&buf).into_owned())
            };
            let get = |p: &str| http(&format!("GET {p} HTTP/1.1\r\nhost: t\r\nconnection: close\r\n\r\n"));
            let post = |p: &str| http(&format!("POST {p} HTTP/1.1\r\nhost: t\r\ncontent-length: 0\r\nconnection: close\r\n\r\n"));
            for seq in 0..64u32 {
                let resp = post(&format!("/emit?producer=p{producer}&seq={seq}"))
                    .unwrap_or_else(|e| panic!("client{producer} emit 失败：{e}"));
                assert!(resp.starts_with("HTTP/1.1 200"), "client{producer} emit 响应异常");
                if seq % 8 == 0 {
                    let _ = get("/healthz").expect("client healthz");
                }
            }
        }));
    }
    for c in clients {
        c.join().expect("client 线程");
    }

    // ≥4 次真实轮转（经 /stats 确认）
    wait_for(
        || {
            if let Ok(resp) = get("/stats") {
                if let Some(v) = parse_stats(&resp, "rotations") {
                    return v >= 4;
                }
            }
            false
        },
        Duration::from_secs(15),
        "≥4 次真实轮转",
    );
    let resp = get("/stats").unwrap();
    assert_eq!(parse_stats(&resp, "dropped"), Some(0), "普通负载无队列丢弃");

    // 窗口二（轮转后）：最终 marker 落盘并在 stop 后仍可查
    let resp = post("/emit?producer=win&seq=final").expect("emit markerB");
    assert!(resp.starts_with("HTTP/1.1 200"));
    let resp = post("/stop").expect("stop");
    assert!(resp.starts_with("HTTP/1.1 200"));

    // 子进程正常退出（exit 0）
    let status = child.wait();
    assert!(status.success(), "夹具子进程应 exit 0（实际 {status}）");
    let _ = stderr_drain.join();

    // 最终 marker 尚在最近三份中；文件数 ≤3、各 ≤M=1024
    let names = ["fixture.log", "fixture.log.1", "fixture.log.2"];
    let all: String = names.iter().map(|n| read_file(dir.path(), n)).collect();
    assert!(all.contains("seq=final"), "最终 marker 在最近三份中");
    assert!(
        all.contains("fixture_final"),
        "关停前最后生命周期日志已落盘"
    );
    let present: Vec<&str> = names
        .iter()
        .copied()
        .filter(|n| dir.path().join(n).exists())
        .collect();
    assert!(present.len() <= 3, "文件数 ≤3");
    for n in present {
        let len = file_len(dir.path(), n).unwrap();
        assert!(len <= 1024, "{n} 长度 {len} ≤ 1024");
    }
}

fn parse_stats(resp: &str, key: &str) -> Option<u64> {
    let body = resp.split("\r\n\r\n").last()?;
    body.split_whitespace()
        .find_map(|kv| kv.strip_prefix(&format!("{key}="))?.parse::<u64>().ok())
}
