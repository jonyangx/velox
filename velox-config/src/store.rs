//! 配置存储：内存索引 + 写穿落盘（`{data_dir}/{namespace}.json`）。
//!
//! 并发说明（开发规范第 3.3 节「异步与并发」）：
//! - `std::sync::RwLock` 只保护短暂内存读写，锁内不做任何文件 IO；
//! - 落盘由 `tokio::sync::Mutex` 串行化，且在串行锁内读取内存快照：
//!   内存修改先于落盘被写锁串行化提交，因此后执行的 flush 读到的状态
//!   永不早于先执行的 flush，并发写不会把文件回退到旧快照（写穿不丢写）；
//! - 真正的文件写入经 `tokio::task::spawn_blocking` 交给阻塞线程池，
//!   不阻塞 tokio worker 线程。

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::RwLock;

use serde_json::Value;
use tokio::sync::Mutex;

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
}

type NamespaceData = BTreeMap<String, Value>;

pub struct Store {
    /// 内存索引：锁内只有纯内存操作（开发规范第 3.3 节）
    data: RwLock<HashMap<String, NamespaceData>>,
    /// 落盘串行锁：flush 时点快照 + 串行落盘，保证文件不回退（见 `flush`）
    flush_lock: Mutex<()>,
    data_dir: PathBuf,
}

/// namespace 同时是落盘文件名，必须限制字符集以防路径注入。
fn valid_namespace(ns: &str) -> bool {
    !ns.is_empty()
        && ns
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

impl Store {
    /// 打开存储目录并恢复已落盘的 namespace 数据；损坏文件跳过并告警，不阻断启动。
    ///
    /// 这里保持同步 IO 是有意的：启动期一次性恢复发生在 HTTP 服务开始接客之前，
    /// 不在 tokio runtime 的请求路径上，不存在「阻塞 worker 线程拖慢并发请求」的问题，
    /// 为它引入 async 只会增加复杂度而没有收益。
    pub fn open(data_dir: impl Into<PathBuf>) -> anyhow::Result<Self> {
        let data_dir = data_dir.into();
        std::fs::create_dir_all(&data_dir)?;
        let mut loaded = HashMap::new();
        for entry in std::fs::read_dir(&data_dir)? {
            let path = entry?.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let Some(ns) = path.file_stem().map(|s| s.to_string_lossy().to_string()) else {
                continue;
            };
            if !valid_namespace(&ns) {
                tracing::warn!(file = %path.display(), "skip file with invalid namespace name");
                continue;
            }
            match std::fs::read(&path)
                .map_err(StoreError::Io)
                .and_then(|b| serde_json::from_slice::<NamespaceData>(&b).map_err(StoreError::Json))
            {
                Ok(map) => {
                    tracing::info!(namespace = %ns, keys = map.len(), "namespace restored");
                    loaded.insert(ns, map);
                }
                Err(e) => {
                    tracing::warn!(file = %path.display(), error = %e, "skip corrupted namespace file")
                }
            }
        }
        Ok(Self {
            data: RwLock::new(loaded),
            flush_lock: Mutex::new(()),
            data_dir,
        })
    }

    /// 写入（写穿落盘）。
    ///
    /// 写锁内只做内存修改（无 IO、无克隆），随后释放锁再异步落盘：
    /// 不跨 `.await` 持锁，也不在 tokio worker 线程上做阻塞文件 IO。
    /// 失败语义与旧实现一致：内存已更新但落盘失败时返回 `Err`。
    pub async fn put(&self, namespace: &str, key: &str, value: Value) -> Result<(), StoreError> {
        if !valid_namespace(namespace) {
            return Err(StoreError::InvalidNamespace(namespace.to_string()));
        }
        // 写锁临界区内只有纯内存插入，随后立刻释放锁
        let existed = {
            let mut data = self.data.write().unwrap();
            let map = data.entry(namespace.to_string()).or_default();
            map.insert(key.to_string(), value).is_some()
        };
        self.flush(namespace).await?;
        tracing::info!(namespace, key, existed, "config written");
        Ok(())
    }

    pub fn get(&self, namespace: &str, key: &str) -> Result<Value, StoreError> {
        let data = self.data.read().unwrap();
        data.get(namespace)
            .and_then(|m| m.get(key))
            .cloned()
            .ok_or_else(|| StoreError::NotFound {
                namespace: namespace.to_string(),
                key: key.to_string(),
            })
    }

    /// 删除单项；返回是否确有删除。落盘策略与 [`Store::put`] 相同：锁外异步落盘。
    pub async fn delete(&self, namespace: &str, key: &str) -> Result<bool, StoreError> {
        // 写锁临界区内只有纯内存删除，随后立刻释放锁
        let removed = {
            let mut data = self.data.write().unwrap();
            let Some(map) = data.get_mut(namespace) else {
                return Ok(false);
            };
            map.remove(key).is_some()
        };
        if removed {
            self.flush(namespace).await?;
            tracing::info!(namespace, key, "config deleted");
        }
        Ok(removed)
    }

    /// 列举 namespace 下全部配置；namespace 不存在返回 None。
    pub fn list(&self, namespace: &str) -> Option<NamespaceData> {
        self.data.read().unwrap().get(namespace).cloned()
    }

    /// 已有 namespace 列表（排序）。
    pub fn list_namespaces(&self) -> Vec<String> {
        let mut names: Vec<String> = self.data.read().unwrap().keys().cloned().collect();
        names.sort();
        names
    }

    /// 落盘整个 namespace 的当前内存快照。
    ///
    /// 正确性关键：快照必须在 `flush_lock` **锁内**读取，而不是在写锁内预克隆。
    /// 内存修改被 `std` 写锁串行化、落盘被 `flush_lock` 串行化，且前者先于后者，
    /// 因此后执行的 flush 读到的内存状态永不早于先执行的 flush——
    /// 并发写同一 namespace 时最后一笔落盘必含最新状态，文件不会回退。
    /// （若在写锁内克隆快照再锁外落盘，慢的那次 flush 可能携带旧快照覆盖新文件。）
    ///
    /// 代价：所有 namespace 的落盘共用一把串行锁。配置写入是低频操作，
    /// 换取顺序正确性是值得的（开发规范第 3.3 节「跨 await 持锁必须评估」：
    /// 此处持的是专为跨 await 设计的 tokio Mutex，持锁期间不做内存修改）。
    ///
    /// 序列化在当前线程完成（纯 CPU、配置数据量小）；真正的文件写入交给
    /// `spawn_blocking` 的阻塞线程池，tokio worker 线程只做一次非阻塞等待。
    async fn flush(&self, namespace: &str) -> Result<(), StoreError> {
        let _guard = self.flush_lock.lock().await;
        // flush 时点快照；namespace 不存在（仅 delete 未命中路径可达）则无可落盘
        let Some(snapshot) = self.data.read().unwrap().get(namespace).cloned() else {
            return Ok(());
        };
        let path = self.data_dir.join(format!("{namespace}.json"));
        let raw = serde_json::to_string_pretty(&snapshot)?;
        tokio::task::spawn_blocking(move || std::fs::write(path, raw))
            .await
            // JoinError 仅在 runtime 正在关停等异常场景出现，统一按 IO 错误上报，
            // 不为此给 StoreError 增加新变体
            .map_err(|e| StoreError::Io(std::io::Error::other(e.to_string())))?
            .map_err(StoreError::Io)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use uuid::Uuid;

    fn temp_store() -> Store {
        let dir = std::env::temp_dir().join(format!("velox-config-test-{}", Uuid::new_v4()));
        Store::open(dir).unwrap()
    }

    // put/delete 现为 async（锁外异步落盘），测试用 tokio runtime 驱动
    #[tokio::test]
    async fn put_get_delete_roundtrip() {
        let store = temp_store();
        store
            .put("svc-a", "governance", json!({"max_retries": 2}))
            .await
            .unwrap();
        assert_eq!(
            store.get("svc-a", "governance").unwrap(),
            json!({"max_retries": 2})
        );
        assert!(store.delete("svc-a", "governance").await.unwrap());
        assert!(!store.delete("svc-a", "governance").await.unwrap());
        assert!(matches!(
            store.get("svc-a", "governance"),
            Err(StoreError::NotFound { .. })
        ));
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
        let dir = std::env::temp_dir().join(format!("velox-config-test-{}", Uuid::new_v4()));
        {
            let store = Store::open(&dir).unwrap();
            store.put("ns1", "k1", json!({"v": 42})).await.unwrap();
        }
        let store2 = Store::open(&dir).unwrap();
        assert_eq!(store2.get("ns1", "k1").unwrap(), json!({"v": 42}));
        std::fs::remove_dir_all(&dir).ok();
    }
}
