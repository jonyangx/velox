//! 配置存储：内存索引 + 写穿落盘（`{data_dir}/{namespace}.json`）。
//!
//! 并发说明：`std::sync::RwLock` 仅保护短暂读写；落盘为小文件同步写入，
//! 数据量小，v0.1 接受锁内 IO（见 subsystems.md 演进路线）。

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::RwLock;

use serde_json::Value;

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
    data: RwLock<HashMap<String, NamespaceData>>,
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
            data_dir,
        })
    }

    /// 写入（写穿落盘）。
    pub fn put(&self, namespace: &str, key: &str, value: Value) -> Result<(), StoreError> {
        if !valid_namespace(namespace) {
            return Err(StoreError::InvalidNamespace(namespace.to_string()));
        }
        let mut data = self.data.write().unwrap();
        let map = data.entry(namespace.to_string()).or_default();
        let existed = map.insert(key.to_string(), value).is_some();
        self.flush_locked(namespace, map)?;
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

    /// 删除单项；返回是否确有删除。
    pub fn delete(&self, namespace: &str, key: &str) -> Result<bool, StoreError> {
        let mut data = self.data.write().unwrap();
        let Some(map) = data.get_mut(namespace) else {
            return Ok(false);
        };
        let removed = map.remove(key).is_some();
        if removed {
            self.flush_locked(namespace, map)?;
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

    /// 持有写锁期间落盘整个 namespace（调用方必须已持锁）。
    fn flush_locked(&self, namespace: &str, map: &NamespaceData) -> Result<(), StoreError> {
        let path = self.data_dir.join(format!("{namespace}.json"));
        let raw = serde_json::to_string_pretty(map)?;
        std::fs::write(path, raw).map_err(StoreError::Io)
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

    #[test]
    fn put_get_delete_roundtrip() {
        let store = temp_store();
        store
            .put("svc-a", "governance", json!({"max_retries": 2}))
            .unwrap();
        assert_eq!(
            store.get("svc-a", "governance").unwrap(),
            json!({"max_retries": 2})
        );
        assert!(store.delete("svc-a", "governance").unwrap());
        assert!(!store.delete("svc-a", "governance").unwrap());
        assert!(matches!(
            store.get("svc-a", "governance"),
            Err(StoreError::NotFound { .. })
        ));
    }

    #[test]
    fn rejects_invalid_namespace() {
        let store = temp_store();
        assert!(matches!(
            store.put("../evil", "k", json!(1)),
            Err(StoreError::InvalidNamespace(_))
        ));
        assert!(matches!(
            store.put("", "k", json!(1)),
            Err(StoreError::InvalidNamespace(_))
        ));
    }

    #[test]
    fn persists_across_restart() {
        let dir = std::env::temp_dir().join(format!("velox-config-test-{}", Uuid::new_v4()));
        {
            let store = Store::open(&dir).unwrap();
            store.put("ns1", "k1", json!({"v": 42})).unwrap();
        }
        let store2 = Store::open(&dir).unwrap();
        assert_eq!(store2.get("ns1", "k1").unwrap(), json!({"v": 42}));
        std::fs::remove_dir_all(&dir).ok();
    }
}
