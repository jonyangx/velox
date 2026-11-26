//! 快照持久化：`{data_dir}/registry/registry-state.json` 单文件全量快照。
//!
//! 布局（架构 §4.2/G2-5）：快照与临时文件都放在 data_dir 下的 registry 专属子目录，
//! 避开 velox-config `Store::open` 对 data_dir **顶层** `*.json` 的非递归 namespace 扫描
//! （`registry-state` 是合法 namespace 名，放顶层会被 config 装载并可能被 put/delete 覆盖）。
//!
//! 并发说明（开发规范第 3.3 节 + 架构 §4.8/G2-1）：
//! - [`SnapshotStore::flush`] 由 `tokio::sync::Mutex`（flush_lock）串行化；
//!   **自读快照闭包起持续持有，直到 `spawn_blocking` 写文件（含 rename）完成才释放**——
//!   覆盖范围 =「读快照 + 完整阻塞 IO」，保证并发落盘时后执行的 flush 读到的状态
//!   永不早于先执行的 flush，文件不会被旧快照回退；
//! - 领域锁（registry.rs 的 `std::sync::RwLock`）只在快照闭包内短暂克隆，不与 IO 共持；
//! - 原子写 = temp + `sync_all` + `rename`：temp 文件内容在 rename 前落盘；
//!   未对父目录 fsync，极端掉电下目录项持久化不作承诺（已知边界，架构 §4.2）。

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// 文件格式版本；恢复时不识别即按损坏整文件跳过（FR-6，演进预留）
const FORMAT_VERSION: u32 = 1;
/// 隔离子目录名（相对 data_dir）
const SUBDIR: &str = "registry";
/// 快照文件名（子目录内）
const SNAPSHOT_FILE: &str = "registry-state.json";
/// 原子写临时文件名：固定名（flush_lock 已串行化单进程写入，避免 tmp 堆积；open 时清理崩溃残留）
const SNAPSHOT_TMP: &str = "registry-state.json.tmp";

/// 落盘文件契约 DTO（本模块持有；`metadata` 为 HashMap，遍历顺序不保证字节稳定，
/// 只承诺内容可恢复——架构 §9 非阻塞纠正）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RegistrySnapshot {
    pub format_version: u32,
    /// 信息性字段，恢复逻辑不依赖（FR-1：实例时间以 `last_heartbeat_epoch` 为准）
    pub saved_at_epoch: u64,
    /// 服务名排序（BTreeMap）；实例按 (address, port, id) 排序
    pub services: BTreeMap<String, Vec<PersistedInstance>>,
}

/// 单实例落盘表示：心跳为绝对时间戳 epoch 秒（FR-1；内存中的 `Instant` 不可序列化）
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PersistedInstance {
    pub id: Uuid,
    pub address: String,
    pub port: u16,
    pub metadata: HashMap<String, String>,
    pub last_heartbeat_epoch: u64,
}

/// 启动恢复结果（仅启动期同步调用，不在请求路径）。
#[derive(Debug)]
pub enum LoadOutcome {
    /// 合法快照
    Restored(RegistrySnapshot),
    /// JSON 非法 / 必填字段缺失 / `format_version` 不识别 / 读取失败——整文件跳过（FR-6）
    Corrupted(String),
    /// 快照文件不存在（首次启动；不视为异常）
    Missing,
}

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

/// registry 快照存储：目录管理 + 启动恢复读取 + 串行化落盘。
#[derive(Debug)]
pub struct SnapshotStore {
    data_dir: PathBuf,
    /// `{data_dir}/registry`（隔离子目录，G2-5）
    dir: PathBuf,
    /// 落盘串行锁：锁覆盖「读快照闭包 + 完整阻塞 IO」（G2-1，见模块注释）
    flush_lock: tokio::sync::Mutex<()>,
}

impl SnapshotStore {
    /// 打开存储：递归创建 `<data_dir>/registry/`（data_dir 与子目录一并创建；
    /// 失败 → Err 含路径，B-3/TC-15 fail fast），并尽力清理崩溃残留的 tmp
    /// （清理发生在 open、先于任何写入；恢复只依赖旧正式文件）。
    pub fn open(data_dir: impl Into<PathBuf>) -> anyhow::Result<Self> {
        let data_dir = data_dir.into();
        let dir = data_dir.join(SUBDIR);
        std::fs::create_dir_all(&dir)
            .map_err(|e| anyhow::anyhow!("创建持久化目录 {} 失败：{e}", dir.display()))?;
        let tmp = dir.join(SNAPSHOT_TMP);
        if tmp.exists() {
            if let Err(e) = std::fs::remove_file(&tmp) {
                tracing::warn!(path = %tmp.display(), error = %e, "清理残留临时快照失败");
            }
        }
        Ok(Self {
            data_dir,
            dir,
            flush_lock: tokio::sync::Mutex::new(()),
        })
    }

    /// 快照文件绝对（相对 cwd）路径：`{data_dir}/registry/registry-state.json`。
    pub fn snapshot_path(&self) -> PathBuf {
        self.dir.join(SNAPSHOT_FILE)
    }

    pub fn data_dir(&self) -> &std::path::Path {
        &self.data_dir
    }

    /// 同步读取快照（仅启动期调用，FR-3：恢复在接客之前）。
    /// 损坏判定（FR-6）：读失败、JSON 解析失败、必填字段缺失、`format_version` 不识别
    /// ——统一 `Corrupted(reason)`，不做部分恢复。
    pub fn load(&self) -> LoadOutcome {
        let path = self.snapshot_path();
        let raw = match std::fs::read(&path) {
            Ok(raw) => raw,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return LoadOutcome::Missing,
            Err(e) => return LoadOutcome::Corrupted(format!("读取失败：{e}")),
        };
        let snapshot: RegistrySnapshot = match serde_json::from_slice(&raw) {
            Ok(s) => s,
            Err(e) => return LoadOutcome::Corrupted(format!("解析失败：{e}")),
        };
        if snapshot.format_version != FORMAT_VERSION {
            return LoadOutcome::Corrupted(format!(
                "format_version={} 不识别（当前支持 {FORMAT_VERSION}）",
                snapshot.format_version
            ));
        }
        LoadOutcome::Restored(snapshot)
    }

    /// 串行化落盘（G2-1 纪律）：
    /// 获取 flush_lock → **锁内**调用 `snapshot_of`（纯内存闭包：短暂取领域读锁克隆后即释放）
    /// → 持锁 `await` `spawn_blocking`（写 `{file}.tmp` → `sync_all()` → `rename`）
    /// → IO 完成后才释放 flush_lock。覆盖范围 =「读快照 + 完整阻塞 IO」。
    pub async fn flush<F>(&self, snapshot_of: F) -> Result<(), StoreError>
    where
        F: FnOnce() -> RegistrySnapshot,
    {
        let _guard = self.flush_lock.lock().await;
        let snapshot = snapshot_of();
        let raw = serde_json::to_string_pretty(&snapshot)?;
        let tmp = self.dir.join(SNAPSHOT_TMP);
        let final_path = self.dir.join(SNAPSHOT_FILE);
        tokio::task::spawn_blocking(move || -> Result<(), StoreError> {
            write_atomic(&tmp, &final_path, raw.as_bytes())
        })
        .await
        // JoinError 仅在 runtime 正在关停等异常场景出现，统一按 IO 错误上报
        .map_err(|e| StoreError::Io(std::io::Error::other(e.to_string())))??;
        Ok(())
    }
}

/// 原子替换写（NFR-5）：temp + `sync_all` + `rename`。串行锁保证同一时刻只有一路写入，
/// 固定 tmp 名不会互相踩踏。
fn write_atomic(
    tmp: &std::path::Path,
    final_path: &std::path::Path,
    bytes: &[u8],
) -> Result<(), StoreError> {
    use std::io::Write;
    let mut file = std::fs::File::create(tmp)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    std::fs::rename(tmp, final_path)?;
    Ok(())
}

/// 共享句柄别名（api.rs AppState / main 装配使用）。
pub type SharedStore = Arc<SnapshotStore>;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn temp_dir() -> PathBuf {
        std::env::temp_dir().join(format!("velox-registry-store-{}", Uuid::new_v4()))
    }

    fn sample_snapshot(epoch: u64) -> RegistrySnapshot {
        let mut services = BTreeMap::new();
        services.insert(
            "svc-a".to_string(),
            vec![PersistedInstance {
                id: Uuid::new_v4(),
                address: "10.0.0.1".to_string(),
                port: 9000,
                metadata: HashMap::from([("version".to_string(), "0.1.0".to_string())]),
                last_heartbeat_epoch: epoch,
            }],
        );
        RegistrySnapshot {
            format_version: FORMAT_VERSION,
            saved_at_epoch: epoch,
            services,
        }
    }

    #[tokio::test]
    async fn flush_load_roundtrip() {
        let dir = temp_dir();
        let store = SnapshotStore::open(&dir).unwrap();
        let snap = sample_snapshot(1_737_000_000);
        store.flush(|| snap.clone()).await.unwrap();
        match store.load() {
            LoadOutcome::Restored(loaded) => assert_eq!(loaded, snap),
            other => panic!("期望 Restored，实际 {other:?}"),
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn load_missing_when_no_file() {
        let dir = temp_dir();
        let store = SnapshotStore::open(&dir).unwrap();
        assert!(matches!(store.load(), LoadOutcome::Missing));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn open_creates_nested_dirs() {
        let dir = temp_dir().join("a").join("b"); // 父目录均不存在（TC-14）
        let store = SnapshotStore::open(&dir).unwrap();
        assert!(dir.join(SUBDIR).is_dir());
        assert_eq!(store.snapshot_path(), dir.join(SUBDIR).join(SNAPSHOT_FILE));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn open_fails_when_path_component_is_file() {
        let base = temp_dir();
        std::fs::create_dir_all(&base).unwrap();
        let blocker = base.join("blocker");
        std::fs::write(&blocker, b"not a dir").unwrap();
        let err = SnapshotStore::open(blocker.join("registry")).unwrap_err();
        assert!(err.to_string().contains("创建持久化目录"), "实际：{err}");
        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn corrupted_variants_are_skipped() {
        let dir = temp_dir();
        let store = SnapshotStore::open(&dir).unwrap();
        let path = store.snapshot_path();

        // 空文件 / 截断 JSON / 缺字段 / 未知版本 → 整文件 Corrupted（TC-13）
        for (desc, content) in [
            ("空文件", ""),
            ("截断", "{\"format_version\": 1, \"saved_at_ep"),
            (
                "缺字段",
                json!({"format_version": 1, "saved_at_epoch": 1})
                    .to_string()
                    .as_str(),
            ),
            (
                "未知版本",
                json!({"format_version": 99, "saved_at_epoch": 1, "services": {}})
                    .to_string()
                    .as_str(),
            ),
        ] {
            std::fs::write(&path, content).unwrap();
            match store.load() {
                LoadOutcome::Corrupted(reason) => assert!(!reason.is_empty(), "{desc}"),
                other => panic!("{desc}：期望 Corrupted，实际 {other:?}"),
            }
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn open_removes_stale_tmp_and_keeps_formal_file() {
        let dir = temp_dir();
        let store = SnapshotStore::open(&dir).unwrap();
        let snap = sample_snapshot(42);
        store.flush(|| snap.clone()).await.unwrap();
        // 模拟崩溃残留的 tmp
        std::fs::write(dir.join(SUBDIR).join(SNAPSHOT_TMP), b"half-written").unwrap();

        // 重新 open：tmp 被清理，正式文件不受影响（TC-18a）
        let reopened = SnapshotStore::open(&dir).unwrap();
        assert!(!dir.join(SUBDIR).join(SNAPSHOT_TMP).exists());
        assert!(matches!(reopened.load(), LoadOutcome::Restored(_)));
        std::fs::remove_dir_all(&dir).ok();
    }

    /// QA-4/G2-1：flush_lock 覆盖「读快照 + 完整 IO」，并发 flush 串行、文件不回退、
    /// 最终文件为后执行 flush 的完整快照。
    #[tokio::test]
    async fn concurrent_flushes_serialize_and_last_wins() {
        let dir = temp_dir();
        let store = Arc::new(SnapshotStore::open(&dir).unwrap());
        let s_old = sample_snapshot(1_000);
        let s_mid = sample_snapshot(2_000);
        let s_new = sample_snapshot(3_000);

        // flush A：闭包内模拟慢速读快照（在锁内拖 150ms），先启动并持有锁
        let store_a = Arc::clone(&store);
        let a = tokio::spawn(async move {
            store_a
                .flush(|| {
                    std::thread::sleep(std::time::Duration::from_millis(150));
                    s_mid.clone()
                })
                .await
                .unwrap();
        });
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        // flush B：后启动（等待 A 释放锁），最终文件必须是 B 的快照
        let store_b = Arc::clone(&store);
        let expected = s_new.clone();
        let b = tokio::spawn(async move {
            store_b.flush(|| expected.clone()).await.unwrap();
        });
        let _ = tokio::join!(a, b);

        match store.load() {
            LoadOutcome::Restored(loaded) => {
                assert_eq!(loaded, s_new, "文件不得被先启动的慢 flush 回退")
            }
            other => panic!("期望 Restored，实际 {other:?}"),
        }
        assert!(
            !dir.join(SUBDIR).join(SNAPSHOT_TMP).exists(),
            "串行写完成后 tmp 应已 rename"
        );
        let _ = s_old; // 仅证明旧快照不残留：s_old != s_new 已由上一断言覆盖
        std::fs::remove_dir_all(&dir).ok();
    }
}
