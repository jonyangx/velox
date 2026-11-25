//! 配置存储：SQLite 单文件 KV 存储（`{data_dir}/velox-config.db`）+
//! namespace 快照 TTL 读缓存。
//!
//! 架构：不再维护内存索引，SQLite 是唯一真相；读路径走 cache-aside
//! （快照缓存由 [`crate::cache::NamespaceCache`] 提供，写路径提交后
//! 主动驱逐），对外 HTTP 语义与 v0.1 完全一致。
//!
//! 并发说明（开发规范第 3.3 节「异步与并发」）：
//! - 单个 `Connection` 由 `std::sync::Mutex` 保护（rusqlite 的 Connection
//!   是 Send 但不是 Sync，包一层 Mutex 即可跨线程共享）；临界区内只有
//!   SQLite 调用，锁从不跨 `.await`（guard 始终待在阻塞线程上）；
//! - 所有 DB 访问经 [`Store::with_conn`] 进入 `tokio::task::spawn_blocking`：
//!   SQLite 是阻塞 IO，不得占用 tokio worker 线程，worker 上只有一次
//!   非阻塞等待；
//! - WAL 模式下的读写并发由 SQLite 串行化；写路径在 DB 提交成功后立即
//!   驱逐缓存，保证「读己之写」（缓存一致性协议见 `cache.rs` 的代数守卫）。
//!
//! 行为差异（相对 v0.1 的 `{namespace}.json` 写穿实现）：KV 表里 namespace
//! 的存在性由行定义——删光最后一个 key 后 namespace 随之消失（`list`
//! 返回 `Ok(None)`），旧实现会留下空的 namespace 文件。v0.1 的旧格式文件
//! 在 `open` 时一次性迁移（见 [`migrate_json_files`]）。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rusqlite::{params, Connection, OptionalExtension};
use serde_json::Value;

use crate::cache::NamespaceCache;

/// SQLite 库文件名（位于 `{data_dir}` 下；同目录会出现 `-wal` / `-shm` 副文件）
const DB_FILE_NAME: &str = "velox-config.db";
/// SQLite busy 超时：journal_mode 切换需要短暂的排它锁；同时容忍外部工具
/// （如 sqlite3 CLI）短暂持有锁，避免立刻报 `SQLITE_BUSY`
const SQLITE_BUSY_TIMEOUT: Duration = Duration::from_secs(5);

/// 单个 namespace 的全部配置（key 排序，与列表接口的展示顺序一致）
pub type NamespaceData = BTreeMap<String, Value>;

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("invalid namespace '{0}': only [a-zA-Z0-9_-] allowed")]
    InvalidNamespace(String),
    #[error("config '{key}' not found in namespace '{namespace}'")]
    NotFound { namespace: String, key: String },
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
}

pub struct Store {
    /// SQLite 连接：唯一真相。`Mutex` 让非 `Sync` 的 Connection 可跨线程
    /// 共享；所有访问都经由 [`Store::with_conn`]（spawn_blocking + 锁中毒
    /// 恢复），请求路径不出现 unwrap。
    conn: Arc<Mutex<Connection>>,
    /// namespace 快照读缓存（cache-aside），写路径提交后主动驱逐
    cache: NamespaceCache,
}

/// namespace 同时用作表的主键与旧格式文件名，必须限制字符集（防路径注入）
fn valid_namespace(ns: &str) -> bool {
    !ns.is_empty()
        && ns
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// 当前 epoch 毫秒（写入 `updated_at` 列；为 v0.2 的 etag / 长轮询预留）。
/// 系统时钟早于 1970 时退化为 0（不可信环境下的尽力而为值）。
fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

/// PRAGMA 与建表。PRAGMA 顺序有讲究：
/// 1. `busy_timeout` 必须最先设置——journal_mode 切换需要短暂的排它锁，
///    没有超时参数时并发场景下会直接报 `SQLITE_BUSY`；
/// 2. journal_mode 返回一行（切换结果），必须用 `query_row` 执行并消费，
///    execute_batch 只接受不返回行的语句；
/// 3. synchronous=NORMAL 配合 WAL：崩溃时已提交事务可能还留在 WAL 中
///    而非 db 主文件，但原子性与一致性由 SQLite 保证，是速度与耐久度的
///    合理折衷。
fn init_db(conn: &Connection) -> Result<(), StoreError> {
    conn.busy_timeout(SQLITE_BUSY_TIMEOUT)?;
    let mode: String = conn.query_row("PRAGMA journal_mode = WAL", [], |row| row.get(0))?;
    if !mode.eq_ignore_ascii_case("wal") {
        tracing::warn!(mode = %mode, "failed to enable WAL journal mode");
    }
    conn.execute_batch("PRAGMA synchronous = NORMAL;")?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS config (
            namespace  TEXT NOT NULL,
            key        TEXT NOT NULL,
            value      TEXT NOT NULL,      -- JSON 序列化文本（UTF-8）
            updated_at INTEGER NOT NULL,   -- epoch 毫秒，为 v0.2 etag/长轮询预留
            PRIMARY KEY (namespace, key)
        ) WITHOUT ROWID;",
    )?;
    Ok(())
}

/// v0.1 旧格式（`{data_dir}/{namespace}.json`，内容为 key→JSON 的 map）
/// 一次性迁移。仅当 `config` 表为空时执行（已有数据即视为迁移已完成或
/// 全新库，直接跳过）；每个文件独立事务导入，成功后改名为
/// `{ns}.json.migrated` 防止重复导入。
/// 损坏 / 不可读的文件只告警跳过，不阻断启动（沿用 v0.1 `open` 的容错
/// 语义：宁可少迁一个文件，不让配置中心起不来）。
fn migrate_json_files(conn: &mut Connection, data_dir: &Path) -> Result<(), StoreError> {
    let count: i64 = conn.query_row("SELECT COUNT(*) FROM config", [], |row| row.get(0))?;
    if count != 0 {
        return Ok(());
    }
    let imported_at = now_millis();
    for entry in std::fs::read_dir(data_dir)? {
        let path = entry?.path();
        // 只处理 *.json；已迁移的 `{ns}.json.migrated` 扩展名不是 json，
        // 天然被跳过（SQLite 的 -wal/-shm 副文件同理）
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let Some(ns) = path.file_stem().map(|s| s.to_string_lossy().to_string()) else {
            continue;
        };
        if !valid_namespace(&ns) {
            tracing::warn!(file = %path.display(), "skip legacy file with invalid namespace name");
            continue;
        }
        let map: NamespaceData = match std::fs::read(&path)
            .map_err(StoreError::Io)
            .and_then(|bytes| serde_json::from_slice(&bytes).map_err(StoreError::Json))
        {
            Ok(map) => map,
            Err(e) => {
                tracing::warn!(file = %path.display(), error = %e, "skip corrupted legacy file");
                continue;
            }
        };
        // 每个文件一个事务：中途失败整体回滚，不留半份数据
        let tx = conn.transaction()?;
        {
            let mut stmt = tx.prepare(
                "INSERT INTO config (namespace, key, value, updated_at) VALUES (?1, ?2, ?3, ?4)",
            )?;
            for (key, value) in &map {
                let raw = serde_json::to_string(value)?;
                stmt.execute(params![ns, key, raw, imported_at])?;
            }
        }
        tx.commit()?;
        // 改名失败只告警：数据已入库（表已非空），下次启动 COUNT != 0 不会重复导入
        let migrated = path.with_extension("json.migrated");
        if let Err(e) = std::fs::rename(&path, &migrated) {
            tracing::warn!(file = %path.display(), error = %e, "legacy file imported but rename failed");
        } else {
            tracing::info!(namespace = %ns, keys = map.len(), "legacy namespace file migrated");
        }
    }
    Ok(())
}

impl Store {
    /// 打开（或创建）SQLite 库并初始化；v0.1 的 `{namespace}.json` 文件
    /// 一次性迁移。
    ///
    /// 这里保持同步 IO 是有意的：启动期一次性执行发生在 HTTP 服务开始
    /// 接客之前，不在 tokio runtime 的请求路径上，不存在「阻塞 worker
    /// 线程拖慢并发请求」的问题，为它引入 async 只会增加复杂度而没有收益。
    pub fn open(data_dir: impl Into<PathBuf>, cache_ttl: Duration) -> anyhow::Result<Self> {
        let data_dir = data_dir.into();
        std::fs::create_dir_all(&data_dir)?;
        let mut conn = Connection::open(data_dir.join(DB_FILE_NAME))?;
        init_db(&conn)?;
        migrate_json_files(&mut conn, &data_dir)?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
            cache: NamespaceCache::new(cache_ttl),
        })
    }

    /// 所有 DB 访问的唯一入口：克隆连接的 Arc → `spawn_blocking`（SQLite
    /// 是阻塞 IO，不得占用 tokio worker 线程）→ 在阻塞线程上锁连接并执行。
    ///
    /// `JoinError` 仅在 runtime 正在关停等异常场景出现，统一按 IO 错误上报，
    /// 不为此给 `StoreError` 增加新变体。
    async fn with_conn<T, F>(&self, f: F) -> Result<T, StoreError>
    where
        F: FnOnce(&mut Connection) -> Result<T, StoreError> + Send + 'static,
        T: Send + 'static,
    {
        let conn = Arc::clone(&self.conn);
        let executed = tokio::task::spawn_blocking(move || {
            // 锁中毒恢复：持有连接的线程 panic 时，SQLite 事务语义保证库
            // 文件不会处于半提交状态，取回连接继续服务优于把 panic 传染
            // 给请求路径
            let mut guard = conn.lock().unwrap_or_else(|e| e.into_inner());
            f(&mut guard)
        })
        .await
        .map_err(|e| StoreError::Io(std::io::Error::other(e.to_string())))?;
        executed
    }

    /// 写入（单行 upsert）。序列化在当前线程完成（纯 CPU、数据量小），
    /// 阻塞的 DB 调用进 `spawn_blocking`；提交成功后驱逐该 namespace 的
    /// 缓存快照，保证「读己之写」。
    pub async fn put(&self, namespace: &str, key: &str, value: Value) -> Result<(), StoreError> {
        if !valid_namespace(namespace) {
            return Err(StoreError::InvalidNamespace(namespace.to_string()));
        }
        let raw = serde_json::to_string(&value)?;
        let ns = namespace.to_string();
        let k = key.to_string();
        let now = now_millis();
        // 先 SELECT 只是为了日志里的 existed 字段：与 upsert 不在同一事务，
        // 并发写交错时该字段仅尽力而为，不影响正确性
        let existed = self
            .with_conn(move |conn| {
                let existed: Option<i64> = conn
                    .query_row(
                        "SELECT 1 FROM config WHERE namespace = ?1 AND key = ?2",
                        params![ns, k],
                        |row| row.get(0),
                    )
                    .optional()?;
                conn.execute(
                    "INSERT INTO config (namespace, key, value, updated_at)
                     VALUES (?1, ?2, ?3, ?4)
                     ON CONFLICT (namespace, key) DO UPDATE
                     SET value = excluded.value, updated_at = excluded.updated_at",
                    params![ns, k, raw, now],
                )?;
                Ok(existed.is_some())
            })
            .await?;
        self.cache.invalidate(namespace);
        tracing::info!(namespace, key, existed, "config written");
        Ok(())
    }

    /// 读取单项：先走缓存快速路径（只克隆单个 Value），未命中再整
    /// namespace 回源；namespace 或 key 不存在报 [`StoreError::NotFound`]。
    pub async fn get(&self, namespace: &str, key: &str) -> Result<Value, StoreError> {
        if let Some(value) = self.cache.get_key(namespace, key) {
            return Ok(value);
        }
        let snapshot = self.load_namespace(namespace).await?;
        snapshot
            .and_then(|mut data| data.remove(key))
            .ok_or_else(|| StoreError::NotFound {
                namespace: namespace.to_string(),
                key: key.to_string(),
            })
    }

    /// 删除单项；返回是否确有删除（`rows_affected > 0` 即命中）。
    /// 未命中时无数据变化，不驱逐缓存。
    pub async fn delete(&self, namespace: &str, key: &str) -> Result<bool, StoreError> {
        let ns = namespace.to_string();
        let k = key.to_string();
        let removed = self
            .with_conn(move |conn| {
                let affected = conn.execute(
                    "DELETE FROM config WHERE namespace = ?1 AND key = ?2",
                    params![ns, k],
                )?;
                Ok(affected > 0)
            })
            .await?;
        if removed {
            self.cache.invalidate(namespace);
            tracing::info!(namespace, key, "config deleted");
        }
        Ok(removed)
    }

    /// 列举 namespace 下全部配置；`Ok(None)` 表示 namespace 不存在。
    pub async fn list(&self, namespace: &str) -> Result<Option<NamespaceData>, StoreError> {
        self.load_namespace(namespace).await
    }

    /// 已有 namespace 列表（排序；管理面低频路径，不走缓存）。
    pub async fn list_namespaces(&self) -> Result<Vec<String>, StoreError> {
        self.with_conn(|conn| {
            let mut stmt =
                conn.prepare("SELECT DISTINCT namespace FROM config ORDER BY namespace")?;
            let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
            let mut namespaces = Vec::new();
            for row in rows {
                namespaces.push(row?);
            }
            Ok(namespaces)
        })
        .await
    }

    /// cache-aside 读路径：命中直接返回；未命中 / 过期则先记代数、再查
    /// DB、最后条件回填。回填携带查询前的代数（`store_if_unchanged`）：
    /// 查 DB 期间发生过写入的话，陈旧快照进不了缓存；即便回填被拦截，
    /// 本次请求仍返回刚从 DB 读到的数据（不额外报错）。
    async fn load_namespace(&self, namespace: &str) -> Result<Option<NamespaceData>, StoreError> {
        if let Some(snapshot) = self.cache.get(namespace) {
            return Ok(Some(snapshot));
        }
        let generation = self.cache.read_generation();
        let ns = namespace.to_string();
        let snapshot = self
            .with_conn(move |conn| {
                let mut stmt = conn
                    .prepare("SELECT key, value FROM config WHERE namespace = ?1 ORDER BY key")?;
                let rows = stmt.query_map([ns], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })?;
                let mut data = NamespaceData::new();
                for row in rows {
                    let (key, raw) = row?;
                    data.insert(key, serde_json::from_str(&raw)?);
                }
                // 空结果 = namespace 不存在：KV 表里 namespace 的存在性由行
                // 定义（与 v0.1 「删光最后一个 key 后留下空文件」的行为差异
                // 见模块文档）
                Ok(if data.is_empty() { None } else { Some(data) })
            })
            .await?;
        self.cache
            .store_if_unchanged(generation, namespace, snapshot.clone());
        Ok(snapshot)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tokio::task::JoinSet;
    use uuid::Uuid;

    /// 常规测试用的缓存 TTL：足够长，覆盖「写后立即读」场景
    const TEST_TTL: Duration = Duration::from_secs(60);

    fn temp_data_dir() -> PathBuf {
        std::env::temp_dir().join(format!("velox-config-test-{}", Uuid::new_v4()))
    }

    fn temp_store() -> Store {
        Store::open(temp_data_dir(), TEST_TTL).unwrap()
    }

    #[tokio::test]
    async fn put_get_delete_roundtrip() {
        let store = temp_store();
        store
            .put("svc-a", "governance", json!({"max_retries": 2}))
            .await
            .unwrap();
        assert_eq!(
            store.get("svc-a", "governance").await.unwrap(),
            json!({"max_retries": 2})
        );
        assert!(store.delete("svc-a", "governance").await.unwrap());
        assert!(!store.delete("svc-a", "governance").await.unwrap());
        assert!(matches!(
            store.get("svc-a", "governance").await,
            Err(StoreError::NotFound { .. })
        ));
        // 新语义：namespace 的存在性由行定义，删光最后一个 key 后 namespace 消失
        assert!(store.list("svc-a").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn rejects_invalid_namespace() {
        let store = temp_store();
        assert!(matches!(
            store.put("../evil", "k", json!(1)).await,
            Err(StoreError::InvalidNamespace(_))
        ));
        assert!(matches!(
            store.put("", "k", json!(1)).await,
            Err(StoreError::InvalidNamespace(_))
        ));
    }

    #[tokio::test]
    async fn persists_across_restart() {
        let dir = temp_data_dir();
        {
            let store = Store::open(&dir, TEST_TTL).unwrap();
            store.put("ns1", "k1", json!({"v": 42})).await.unwrap();
        }
        // drop 后重新打开同一目录：SQLite 是唯一真相，数据仍在
        let store2 = Store::open(&dir, TEST_TTL).unwrap();
        assert_eq!(store2.get("ns1", "k1").await.unwrap(), json!({"v": 42}));
        std::fs::remove_dir_all(&dir).ok();
    }

    // multi_thread runtime 让 spawn_blocking 的阻塞线程与 async 任务真正并行，
    // 检验 Mutex<Connection> 串行化下并发写不丢
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn concurrent_puts_all_landed() {
        let store = Arc::new(temp_store());
        let mut tasks = JoinSet::new();
        for i in 0..32 {
            let store = Arc::clone(&store);
            tasks.spawn(async move {
                let key = format!("key-{i}");
                store.put("svc-busy", &key, json!({ "i": i })).await
            });
        }
        while let Some(result) = tasks.join_next().await {
            result.unwrap().unwrap();
        }
        let items = store.list("svc-busy").await.unwrap().unwrap();
        assert_eq!(items.len(), 32);
    }

    #[tokio::test]
    async fn migrates_legacy_json_files() {
        let dir = temp_data_dir();
        std::fs::create_dir_all(&dir).unwrap();
        // v0.1 旧格式：{ns}.json，内容为 key → JSON 的 map
        std::fs::write(
            dir.join("legacy-a.json"),
            r#"{"k1": {"v": 1}, "k2": "str"}"#,
        )
        .unwrap();
        std::fs::write(dir.join("legacy-b.json"), r#"{"k": [1, 2]}"#).unwrap();

        let store = Store::open(&dir, TEST_TTL).unwrap();
        assert_eq!(store.get("legacy-a", "k1").await.unwrap(), json!({"v": 1}));
        assert_eq!(store.get("legacy-a", "k2").await.unwrap(), json!("str"));
        assert_eq!(store.get("legacy-b", "k").await.unwrap(), json!([1, 2]));
        // 原文件被改名为 .json.migrated，原文件不复存在
        assert!(dir.join("legacy-a.json.migrated").exists());
        assert!(!dir.join("legacy-a.json").exists());
        assert!(dir.join("legacy-b.json.migrated").exists());
        // namespace 列表包含迁移来的两个
        let namespaces = store.list_namespaces().await.unwrap();
        assert!(namespaces.contains(&"legacy-a".to_string()));
        assert!(namespaces.contains(&"legacy-b".to_string()));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn corrupted_legacy_file_is_skipped() {
        let dir = temp_data_dir();
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("bad.json"), "{ not valid json").unwrap();
        // 完好文件与损坏文件并存：损坏的跳过，不阻断其余迁移
        std::fs::write(dir.join("good.json"), r#"{"k": 1}"#).unwrap();

        let store = Store::open(&dir, TEST_TTL).unwrap();
        // 损坏文件：不导入、原样保留（未被改名）
        assert!(matches!(
            store.get("bad", "k").await,
            Err(StoreError::NotFound { .. })
        ));
        assert!(dir.join("bad.json").exists());
        assert!(!dir.join("bad.json.migrated").exists());
        // 完好文件正常导入
        assert_eq!(store.get("good", "k").await.unwrap(), json!(1));
        std::fs::remove_dir_all(&dir).ok();
    }

    // 证明 TTL 内读的是缓存：绕过 Store 直连 DB 删行（不经过 invalidate），
    // get / list 仍返回旧快照
    #[tokio::test]
    async fn cache_serves_snapshot_within_ttl() {
        let dir = temp_data_dir();
        let store = Store::open(&dir, Duration::from_secs(5)).unwrap();
        store.put("svc-c", "k1", json!({"v": 1})).await.unwrap();
        // 读一次，回填缓存
        assert_eq!(store.get("svc-c", "k1").await.unwrap(), json!({"v": 1}));

        let conn = Connection::open(dir.join(DB_FILE_NAME)).unwrap();
        conn.execute("DELETE FROM config WHERE namespace = ?1", params!["svc-c"])
            .unwrap();
        drop(conn);

        // TTL 未过期：仍命中缓存里的旧快照
        assert_eq!(store.get("svc-c", "k1").await.unwrap(), json!({"v": 1}));
        let items = store.list("svc-c").await.unwrap().unwrap();
        assert_eq!(items.len(), 1);
        std::fs::remove_dir_all(&dir).ok();
    }

    // sleep 至少超过 TTL（机器慢只会更过期，不会抖动）；过期后回源 DB
    #[tokio::test]
    async fn cache_expires_after_ttl() {
        let dir = temp_data_dir();
        let store = Store::open(&dir, Duration::from_millis(50)).unwrap();
        store.put("svc-e", "k1", json!({"v": 1})).await.unwrap();
        assert_eq!(store.get("svc-e", "k1").await.unwrap(), json!({"v": 1}));

        tokio::time::sleep(Duration::from_millis(200)).await;

        let conn = Connection::open(dir.join(DB_FILE_NAME)).unwrap();
        conn.execute("DELETE FROM config WHERE namespace = ?1", params!["svc-e"])
            .unwrap();
        drop(conn);

        // 过期回源：行已被直连删除 → NotFound
        assert!(matches!(
            store.get("svc-e", "k1").await,
            Err(StoreError::NotFound { .. })
        ));
        std::fs::remove_dir_all(&dir).ok();
    }

    // 写驱逐：put / delete 提交后缓存里的旧快照必须失效
    #[tokio::test]
    async fn write_invalidates_cache() {
        let store = temp_store();
        store.put("svc-w", "k", json!("v1")).await.unwrap();
        assert_eq!(store.get("svc-w", "k").await.unwrap(), json!("v1")); // 回填
        store.put("svc-w", "k", json!("v2")).await.unwrap();
        assert_eq!(store.get("svc-w", "k").await.unwrap(), json!("v2"));
        assert!(store.delete("svc-w", "k").await.unwrap());
        assert!(matches!(
            store.get("svc-w", "k").await,
            Err(StoreError::NotFound { .. })
        ));
    }
}
