//! namespace 快照 TTL 读缓存（cache-aside 模式）。
//!
//! 职责：为 [`crate::store::Store`] 的读路径提供整 namespace 快照缓存。
//! SQLite 是唯一真相，本缓存只做加速，不裁决数据是否存在。
//!
//! 核心不变量——代数（generation）守卫：
//! 读路径「缓存未命中 → 读 DB → 回填缓存」是三步非原子操作。若在「读 DB」
//! 期间发生了写入（写路径提交 DB 后会 bump 代数并驱逐缓存），读方拿到的
//! 就是不含该写入的**陈旧快照**；放任它回填的话，陈旧数据会在缓存里存活
//! 整个 TTL，违反「读己之写」。因此回填必须携带读 DB 前记下的代数：
//! [`NamespaceCache::store_if_unchanged`] 在加写锁前后各核对一次代数，
//! 代数已变即丢弃快照（读方本次仍返回刚从 DB 读到的数据，只是不污染缓存）。
//! 与之配合，[`NamespaceCache::invalidate`] 的顺序是**先** bump 代数、
//! **再**清条目，保证任何交叠下陈旧快照要么进不了缓存、要么随即被驱逐：
//! - 回填尚未拿到写锁 → 被第一次（或锁内第二次）代数检查拦截；
//! - 回填已越过两次检查并插入 → 它必然先于写入方拿到写锁，
//!   随后 invalidate 的 remove 会把这条目驱逐。

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::RwLock;
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::store::NamespaceData;

/// TTL 上限（10 年）：`Instant::now() + ttl` 溢出会在请求路径 panic，
/// 超过上限视为调用方配置错误，告警后夹紧。
const MAX_TTL: Duration = Duration::from_secs(10 * 365 * 24 * 3600);

/// 单个 namespace 的缓存条目：快照 + 过期时刻（惰性过期，不主动清除）。
struct Entry {
    data: NamespaceData,
    expires_at: Instant,
}

/// namespace 快照缓存。
///
/// 并发说明（开发规范第 3.3 节）：
/// - `RwLock` 只保护短暂内存读写，锁内不做任何 IO、不跨 `.await`；
/// - 锁中毒走恢复而非 unwrap：缓存数据不是真相，持有者 panic 后直接
///   复用锁内数据，比把 panic 传染给请求路径更安全。
pub struct NamespaceCache {
    /// 为 0 表示禁用：读永远未命中（条目立即过期）、写永远不回填
    ttl: Duration,
    entries: RwLock<HashMap<String, Entry>>,
    /// 写入代数：每次 invalidate 递增，用于拦截在途的陈旧回填（见模块文档）
    generation: AtomicU64,
}

impl NamespaceCache {
    pub fn new(ttl: Duration) -> Self {
        // Instant + ttl 溢出会在请求路径 panic，超上限直接夹紧
        let ttl = if ttl > MAX_TTL {
            tracing::warn!(ttl = ?ttl, max = ?MAX_TTL, "cache ttl exceeds upper bound, clamped");
            MAX_TTL
        } else {
            ttl
        };
        Self {
            ttl,
            entries: RwLock::new(HashMap::new()),
            generation: AtomicU64::new(0),
        }
    }

    /// 取整个 namespace 快照（克隆整个 map）；未缓存 / 过期返回 None。
    pub fn get(&self, namespace: &str) -> Option<NamespaceData> {
        self.with_entry(namespace, |entry| Some(entry.data.clone()))
    }

    /// 取单个配置值（只克隆目标 Value，避免整 map 克隆）；未命中返回 None。
    pub fn get_key(&self, namespace: &str, key: &str) -> Option<Value> {
        self.with_entry(namespace, |entry| entry.data.get(key).cloned())
    }

    /// 当前写入代数（读 DB 前记录，回填时核对）。
    pub fn read_generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }

    /// 条件回填：仅当从记录 `read_generation` 至今未发生过写入时才入缓存。
    ///
    /// `snapshot` 为 `None` 表示 namespace 不存在——不回填（不做负缓存），
    /// 否则「刚创建的 namespace」会被一条「不存在」的缓存挡住。
    pub fn store_if_unchanged(
        &self,
        read_generation: u64,
        namespace: &str,
        snapshot: Option<NamespaceData>,
    ) {
        let Some(data) = snapshot else { return };
        // 禁用缓存（ttl 为 0）：不回填
        if self.ttl.is_zero() {
            return;
        }
        // 第一次检查（锁外，快速失败）：读 DB 期间若发生过写入，直接丢弃
        if self.generation.load(Ordering::Acquire) != read_generation {
            return;
        }
        let mut entries = self.entries.write().unwrap_or_else(|e| e.into_inner());
        // 第二次检查（锁内）：覆盖「第一次检查之后、拿到写锁之前」被写入
        // 穿插的窗口。若此处代数仍未变，本条目要么在写入方拿锁之前已插入
        // （随后必被其 remove 驱逐），要么写入发生在插入之后（其 invalidate
        // 同样会驱逐）——两种情况陈旧数据都活不过 TTL。
        if self.generation.load(Ordering::Acquire) != read_generation {
            return;
        }
        entries.insert(
            namespace.to_string(),
            Entry {
                data,
                expires_at: Instant::now() + self.ttl,
            },
        );
    }

    /// 驱逐一个 namespace 的缓存条目，并 bump 写入代数。
    ///
    /// 顺序关键：**先** bump 代数、**再**拿写锁 remove——在途回填要么被
    /// （锁内的）第二次代数检查拦截，要么插入后被本次 remove 驱逐。
    pub fn invalidate(&self, namespace: &str) {
        self.generation.fetch_add(1, Ordering::AcqRel);
        self.entries
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .remove(namespace);
    }

    /// 在读锁存活期间对条目应用 `f`。不能把 guard 里的引用返回出去，
    /// 因此所有克隆都在 `f` 内完成；未缓存或已过期返回 None。
    fn with_entry<T>(&self, namespace: &str, f: impl FnOnce(&Entry) -> Option<T>) -> Option<T> {
        let entries = self.entries.read().unwrap_or_else(|e| e.into_inner());
        let entry = entries.get(namespace)?;
        // 惰性过期：过期视为未命中，条目留在原地等待覆盖或被 invalidate 驱逐
        if Instant::now() >= entry.expires_at {
            return None;
        }
        f(entry)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sample_data() -> NamespaceData {
        [("a", json!(1)), ("b", json!("x"))]
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect()
    }

    #[test]
    fn hit_and_get_key() {
        let cache = NamespaceCache::new(Duration::from_secs(60));
        let gen = cache.read_generation();
        cache.store_if_unchanged(gen, "ns", Some(sample_data()));
        let snapshot = cache.get("ns").unwrap();
        assert_eq!(snapshot.len(), 2);
        assert_eq!(snapshot["a"], json!(1));
        assert_eq!(cache.get_key("ns", "a").unwrap(), json!(1));
        assert_eq!(cache.get_key("ns", "b").unwrap(), json!("x"));
        // 缓存里没有的 key 返回 None（不能把「key 缺失」与「namespace 未缓存」混为一谈）
        assert!(cache.get_key("ns", "missing").is_none());
        // 未缓存的 namespace 返回 None
        assert!(cache.get("other").is_none());
        assert!(cache.get_key("other", "a").is_none());
    }

    #[test]
    fn evict_removes_entry() {
        let cache = NamespaceCache::new(Duration::from_secs(60));
        let gen = cache.read_generation();
        cache.store_if_unchanged(gen, "ns", Some(sample_data()));
        cache.invalidate("ns");
        assert!(cache.get("ns").is_none());
        assert!(cache.get_key("ns", "a").is_none());
    }

    // sleep 至少超过 TTL：机器慢只会更过期，不会抖动
    #[test]
    fn entry_expires_after_ttl() {
        let cache = NamespaceCache::new(Duration::from_millis(50));
        let gen = cache.read_generation();
        cache.store_if_unchanged(gen, "ns", Some(sample_data()));
        std::thread::sleep(Duration::from_millis(200));
        assert!(cache.get("ns").is_none());
        assert!(cache.get_key("ns", "a").is_none());
    }

    // 在途回填竞态：记录代数后发生写入（invalidate bump 代数），
    // 携带旧代数的回填必须被拦截，陈旧快照进不了缓存
    #[test]
    fn stale_fill_blocked_by_generation() {
        let cache = NamespaceCache::new(Duration::from_secs(60));
        let stale_gen = cache.read_generation();
        cache.invalidate("ns"); // 代数 +1
        cache.store_if_unchanged(stale_gen, "ns", Some(sample_data()));
        assert!(cache.get("ns").is_none());
    }

    // namespace 不存在（None 快照）不做负缓存
    #[test]
    fn missing_namespace_not_cached() {
        let cache = NamespaceCache::new(Duration::from_secs(60));
        let gen = cache.read_generation();
        cache.store_if_unchanged(gen, "ns", None);
        assert!(cache.get("ns").is_none());
    }

    // ttl 为 0 表示禁用：写永远不回填、读永远未命中
    #[test]
    fn zero_ttl_disables_cache() {
        let cache = NamespaceCache::new(Duration::ZERO);
        let gen = cache.read_generation();
        cache.store_if_unchanged(gen, "ns", Some(sample_data()));
        assert!(cache.get("ns").is_none());
        assert!(cache.get_key("ns", "a").is_none());
    }
}
