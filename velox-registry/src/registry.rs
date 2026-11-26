//! 服务注册领域模型：注册、心跳续约、注销、TTL 剔除与查询。
//!
//! v0.2 新增（架构 §4.3）：全局变更版本号（watch 长轮询）、脏标记（周期刷盘）、
//! 变更广播通道、启动恢复（按绝对时间 TTL 过滤）与全量快照导出。
//!
//! 并发说明：`std::sync::RwLock` 仅保护短暂的内存读写（含快照克隆），临界区内
//! 无 IO、无 await；落盘由 `SnapshotStore::flush` 的串行锁覆盖（store.rs）。
//! 版本号与实例表同处一把锁（`RegistryState`），watch 一次读锁同时取得
//! `(version, 快照)`，杜绝「版本新、快照旧」错配（架构不变量 1）。

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::RwLock;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::Serialize;
use uuid::Uuid;

use crate::store::{PersistedInstance, RegistrySnapshot};

/// 快照文件格式版本（与 store.rs 的 `FORMAT_VERSION` 一致）
pub const SNAPSHOT_FORMAT_VERSION: u32 = 1;

#[derive(Debug, thiserror::Error)]
pub enum RegistryError {
    #[error("instance '{instance}' not found in service '{service}'")]
    InstanceNotFound { service: String, instance: String },
    #[error("service '{0}' not found")]
    ServiceNotFound(String),
}

/// 实例内部表示（含心跳计时，不对外暴露）。
/// 内存保持 `Instant`：运行期 TTL 语义与 v0.1 逐字节一致（架构 §4.3/§7 B1）。
#[derive(Debug)]
struct Instance {
    address: String,
    port: u16,
    metadata: HashMap<String, String>,
    last_heartbeat: Instant,
}

/// 对外视图（HTTP / mesh 拉取快照 / watch 快照共用此结构）。
#[derive(Debug, Serialize)]
pub struct InstanceView {
    pub id: Uuid,
    pub service: String,
    pub address: String,
    pub port: u16,
    pub metadata: HashMap<String, String>,
    /// 距上次心跳的秒数
    pub seconds_since_heartbeat: u64,
    pub healthy: bool,
}

/// 领域状态：全局版本号 + 实例表，同锁保护（架构不变量 1）。
struct RegistryState {
    /// 全局变更版本号，从 0 起；重启归零不持久化（FR-8）
    version: u64,
    services: HashMap<String, HashMap<Uuid, Instance>>,
}

/// 启动恢复统计（NFR-4 启动日志字段）。
#[derive(Debug, Clone, Copy, Default)]
pub struct RestoreStats {
    pub services: usize,
    pub instances: usize,
    pub dropped_expired: usize,
}

/// 注册表：service 名 → 实例集合 + 版本号 + 脏标记 + 变更广播。
pub struct Registry {
    state: RwLock<RegistryState>,
    ttl: Duration,
    /// true = 内存新于文件；置位方：任何 mutation、任一 flush 失败后的置回（G2-1）。
    /// 仅两种操作：`take_dirty`（swap false，仅 flusher 在读快照之前调用）与
    /// `mark_dirty`（mutation 置位 / 失败置回，幂等）；成功路径不清除（架构不变量 5）。
    dirty: AtomicBool,
    /// 变更广播：send(version) 非阻塞，写锁内调用安全；无接收者时错误忽略
    change_tx: tokio::sync::watch::Sender<u64>,
}

/// 当前系统时间的 epoch 秒（恢复判定与快照时间戳共用）。
pub fn now_epoch() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

impl Registry {
    pub fn new(ttl: Duration) -> Self {
        let (change_tx, _) = tokio::sync::watch::channel(0);
        Self {
            state: RwLock::new(RegistryState {
                version: 0,
                services: HashMap::new(),
            }),
            ttl,
            dirty: AtomicBool::new(false),
            change_tx,
        }
    }

    /// 启动恢复：`now_epoch` 下按 `now_epoch - last_heartbeat_epoch > ttl_secs` 丢弃
    /// 超时实例（FR-4，**严格大于**：age == ttl 保留）。空/缺失服务条目不建。
    /// 仅恢复期同步调用（FR-3：接客之前）。
    ///
    /// `dropped_expired > 0 → mark_dirty`（延迟清理，G2-4/§9 非阻塞纠正）：被过滤实例
    /// 仍在文件中，由周期 flusher 在一个周期内把文件改写为过滤后状态；不做启动期
    /// 额外同步写。dropped_expired == 0 → 不置 dirty；启动后立即崩溃时，文件中的
    /// 过期实例在下次启动被再次过滤，幂等无害。
    pub fn restored(
        ttl: Duration,
        snapshot: RegistrySnapshot,
        now_epoch: u64,
    ) -> (Self, RestoreStats) {
        let ttl_secs = ttl.as_secs();
        let registry = Self::new(ttl);
        let mut stats = RestoreStats::default();
        {
            let mut state = registry.state.write().unwrap();
            for (name, instances) in snapshot.services {
                let mut map = HashMap::new();
                for p in instances {
                    let age = now_epoch.saturating_sub(p.last_heartbeat_epoch);
                    if age > ttl_secs {
                        stats.dropped_expired += 1;
                        tracing::warn!(
                            service = %name,
                            instance = %p.id,
                            age_secs = age,
                            ttl_secs,
                            "恢复时心跳已超 TTL，丢弃（FR-4）"
                        );
                        continue;
                    }
                    // 换算回单调时钟；age 超过进程运行时长时 `checked_sub` 失败，
                    // 钳位到 now（兜底：实例已通过 TTL 过滤，钳位只影响下次到期判定的
                    // 秒级偏差，下次心跳即复位；禁止直接 `Instant::now() - Duration` 防 panic）
                    let last_heartbeat = Instant::now()
                        .checked_sub(Duration::from_secs(age))
                        .unwrap_or_else(Instant::now);
                    map.insert(
                        p.id,
                        Instance {
                            address: p.address,
                            port: p.port,
                            metadata: p.metadata,
                            last_heartbeat,
                        },
                    );
                    stats.instances += 1;
                }
                if !map.is_empty() {
                    stats.services += 1;
                    state.services.insert(name, map);
                }
            }
        }
        if stats.dropped_expired > 0 {
            registry.mark_dirty();
        }
        (registry, stats)
    }

    /// 变更提交（写锁内调用）：版本 +1（批次事件由调用方保证单次）、置脏、广播唤醒。
    /// `tokio::sync::watch::Sender::send` 非阻塞，锁内安全；无接收者时忽略。
    fn record_change(&self, version: &mut u64) {
        *version += 1;
        self.dirty.store(true, Ordering::SeqCst);
        let _ = self.change_tx.send(*version);
    }

    /// 注册新实例，分配 `instance_id` 并返回初始视图（version+1 / dirty / notify）。
    pub fn register(
        &self,
        service: &str,
        address: String,
        port: u16,
        metadata: HashMap<String, String>,
    ) -> InstanceView {
        let id = Uuid::new_v4();
        let instance = Instance {
            address: address.clone(),
            port,
            metadata: metadata.clone(),
            last_heartbeat: Instant::now(),
        };
        let mut state = self.state.write().unwrap();
        state
            .services
            .entry(service.to_string())
            .or_default()
            .insert(id, instance);
        self.record_change(&mut state.version);
        drop(state);
        tracing::info!(
            service,
            instance = %id,
            address = %address,
            port,
            "instance registered"
        );
        InstanceView {
            id,
            service: service.to_string(),
            address,
            port,
            metadata,
            seconds_since_heartbeat: 0,
            healthy: true,
        }
    }

    /// 心跳续约：仅更新内存并置脏（FR-2，永不因落盘失败报错）。
    /// 心跳使 unhealthy→healthy（「复活」）时 version+1 并唤醒 watch（QA-3 定案：
    /// 写锁内一次 `elapsed > ttl` 判断，避免快照内容变化但客户端等满 timeout）；
    /// 健康内续约不 +1（内容无变化，仅置脏）。
    pub fn heartbeat(&self, service: &str, id: Uuid) -> Result<(), RegistryError> {
        let mut state = self.state.write().unwrap();
        let instances = state
            .services
            .get_mut(service)
            .ok_or_else(|| RegistryError::ServiceNotFound(service.to_string()))?;
        let instance = instances
            .get_mut(&id)
            .ok_or_else(|| RegistryError::InstanceNotFound {
                service: service.to_string(),
                instance: id.to_string(),
            })?;
        let revived = instance.last_heartbeat.elapsed() > self.ttl;
        instance.last_heartbeat = Instant::now();
        if revived {
            // 复活：健康状态翻转可被 watch 感知，视为内容变化
            self.record_change(&mut state.version);
        } else {
            self.dirty.store(true, Ordering::SeqCst);
        }
        Ok(())
    }

    /// 主动注销；最后一个实例移除后连同服务条目一并清理（version+1 / dirty / notify）。
    pub fn deregister(&self, service: &str, id: Uuid) -> Result<(), RegistryError> {
        let mut state = self.state.write().unwrap();
        let instances = state
            .services
            .get_mut(service)
            .ok_or_else(|| RegistryError::ServiceNotFound(service.to_string()))?;
        instances
            .remove(&id)
            .ok_or_else(|| RegistryError::InstanceNotFound {
                service: service.to_string(),
                instance: id.to_string(),
            })?;
        if instances.is_empty() {
            state.services.remove(service);
        }
        self.record_change(&mut state.version);
        drop(state);
        tracing::info!(service, instance = %id, "instance deregistered");
        Ok(())
    }

    /// 剔除心跳超时实例，返回剔除数量（含随之清空的服务条目）。
    /// 一次 sweep 移除 k≥1 个实例：批次整体 version+1 / dirty / notify（QA-2 定案）。
    pub fn sweep_expired(&self) -> usize {
        let mut state = self.state.write().unwrap();
        let mut removed = 0usize;
        state.services.retain(|service, instances| {
            instances.retain(|id, inst| {
                let expired = inst.last_heartbeat.elapsed() > self.ttl;
                if expired {
                    removed += 1;
                    tracing::warn!(service, instance = %id, "instance expired, removed");
                }
                !expired
            });
            !instances.is_empty()
        });
        if removed > 0 {
            self.record_change(&mut state.version);
        }
        removed
    }

    /// 全部服务名（排序输出，便于调试与快照对比）。
    pub fn list_services(&self) -> Vec<String> {
        let mut names: Vec<String> = self
            .state
            .read()
            .unwrap()
            .services
            .keys()
            .cloned()
            .collect();
        names.sort();
        names
    }

    /// 实例列表；`only_healthy` 时仅返回 TTL 内有心跳的实例。
    /// 服务不存在视为错误（便于调用方发现拼写错误）。
    pub fn list_instances(
        &self,
        service: &str,
        only_healthy: bool,
    ) -> Result<Vec<InstanceView>, RegistryError> {
        let state = self.state.read().unwrap();
        let instances = state
            .services
            .get(service)
            .ok_or_else(|| RegistryError::ServiceNotFound(service.to_string()))?;
        let mut views: Vec<InstanceView> = instances
            .iter()
            .map(|(id, inst)| Self::view(service, *id, inst, self.ttl))
            .filter(|v| !only_healthy || v.healthy)
            .collect();
        views.sort_by(|a, b| (&a.address, a.port).cmp(&(&b.address, b.port)));
        Ok(views)
    }

    // ---- watch 支撑（FR-8） ----

    /// 当前全局版本号（查询不触发变更，FR-9）。
    pub fn version(&self) -> u64 {
        self.state.read().unwrap().version
    }

    /// 订阅变更通道（watch handler 在读快照**之前**调用，杜绝「读后变、通道漏通知」）。
    pub fn subscribe(&self) -> tokio::sync::watch::Receiver<u64> {
        self.change_tx.subscribe()
    }

    /// 单次读锁同时取 (version, 各服务健康实例列表)：服务名 BTreeMap 排序，
    /// 实例按 (address, port) 排序；无健康实例的服务不出现在 map 中。
    pub fn watch_snapshot(&self) -> (u64, BTreeMap<String, Vec<InstanceView>>) {
        let state = self.state.read().unwrap();
        let mut map = BTreeMap::new();
        for (name, instances) in &state.services {
            let mut views: Vec<InstanceView> = instances
                .iter()
                .map(|(id, inst)| Self::view(name, *id, inst, self.ttl))
                .filter(|v| v.healthy)
                .collect();
            views.sort_by(|a, b| (&a.address, a.port).cmp(&(&b.address, b.port)));
            if !views.is_empty() {
                map.insert(name.clone(), views);
            }
        }
        (state.version, map)
    }

    /// 持久化全量快照（含未健康未剔除实例；恢复期由 FR-4 二次过滤）。
    /// 调用方约定：在 flush_lock 锁内调用（G2-1 纪律）。
    pub fn snapshot(&self) -> RegistrySnapshot {
        let state = self.state.read().unwrap();
        let now = now_epoch();
        let mut services = BTreeMap::new();
        for (name, instances) in &state.services {
            let mut list: Vec<PersistedInstance> = instances
                .iter()
                .map(|(id, inst)| PersistedInstance {
                    id: *id,
                    address: inst.address.clone(),
                    port: inst.port,
                    metadata: inst.metadata.clone(),
                    // Instant → epoch：now − 已流逝秒数（秒精度）
                    last_heartbeat_epoch: now
                        .saturating_sub(inst.last_heartbeat.elapsed().as_secs()),
                })
                .collect();
            list.sort_by(|a, b| (&a.address, a.port, a.id).cmp(&(&b.address, b.port, b.id)));
            services.insert(name.clone(), list);
        }
        RegistrySnapshot {
            format_version: SNAPSHOT_FORMAT_VERSION,
            saved_at_epoch: now,
            services,
        }
    }

    /// 原子取走脏标记；true 则调用方执行 flush（FR-2）。
    /// **纪律（G2-1/TC-05c）**：必须先 take、再在 flush_lock 内取快照——顺序颠倒时，
    /// 「取快照 → 并发 mutation 置 dirty → 成功 flush」会丢失该新修改的重试标记。
    pub fn take_dirty(&self) -> bool {
        self.dirty.swap(false, Ordering::SeqCst)
    }

    /// flush 失败后原子置回（幂等，G2-1）。成功路径**不得**调用：成功不清除任何标记，
    /// 快照之后的新 dirty 只由其自身置位表达。失败置回后，即使无新 mutation，
    /// 下个周期也会重试（TC-05b/TC-17）。
    pub fn mark_dirty(&self) {
        self.dirty.store(true, Ordering::SeqCst);
    }

    fn view(service: &str, id: Uuid, inst: &Instance, ttl: Duration) -> InstanceView {
        let elapsed = inst.last_heartbeat.elapsed();
        InstanceView {
            id,
            service: service.to_string(),
            address: inst.address.clone(),
            port: inst.port,
            metadata: inst.metadata.clone(),
            seconds_since_heartbeat: elapsed.as_secs(),
            healthy: elapsed <= ttl,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread::sleep;

    fn ttl(secs: u64) -> Duration {
        Duration::from_secs(secs)
    }

    /// 版本递增规则全表（架构 §4.3）：
    /// 注册 +1、注销 +1、一次 sweep 批次 +1、复活心跳 +1、健康续约/查询不 +1。
    #[test]
    fn version_bump_rules() {
        let reg = Registry::new(ttl(30));
        assert_eq!(reg.version(), 0);

        let v1 = reg.register("svc", "10.0.0.1".into(), 9000, HashMap::new());
        assert_eq!(reg.version(), 1, "注册 +1");

        reg.heartbeat("svc", v1.id).unwrap();
        assert_eq!(reg.version(), 1, "健康内续约不 +1");

        let v2 = reg.register("svc", "10.0.0.2".into(), 9001, HashMap::new());
        assert_eq!(reg.version(), 2, "第二次注册 +1");

        reg.deregister("svc", v2.id).unwrap();
        assert_eq!(reg.version(), 3, "注销 +1");

        // 空 sweep：无移除不递增
        assert_eq!(reg.sweep_expired(), 0);
        assert_eq!(reg.version(), 3, "空 sweep 不 +1");

        // 查询不 +1（FR-9）
        let _ = reg.list_services();
        let _ = reg.list_instances("svc", false);
        let _ = reg.watch_snapshot();
        assert_eq!(reg.version(), 3);

        // 复活心跳：TTL 过去后实例 unhealthy，迟到心跳翻回健康 → +1
        sleep(Duration::from_millis(30));
        let short = Registry::new(Duration::from_millis(20));
        let a = short.register("s", "1.1.1.1".into(), 1, HashMap::new());
        assert_eq!(short.version(), 1);
        sleep(Duration::from_millis(40)); // 超过 TTL，unhealthy
        assert!(!short.list_instances("s", false).unwrap()[0].healthy);
        short.heartbeat("s", a.id).unwrap();
        assert_eq!(short.version(), 2, "复活心跳 +1");
        // 复活后的立即续约：不再 +1
        short.heartbeat("s", a.id).unwrap();
        assert_eq!(short.version(), 2);
    }

    /// 一次 sweep 移除多实例：批次整体仅 +1（QA-2 定案）。
    #[test]
    fn sweep_batch_bumps_once() {
        let reg = Registry::new(Duration::from_millis(20));
        let a = reg.register("svc", "1.1.1.1".into(), 1, HashMap::new());
        let b = reg.register("svc", "1.1.1.2".into(), 2, HashMap::new());
        assert_eq!(reg.version(), 2);
        sleep(Duration::from_millis(40));
        let removed = reg.sweep_expired();
        assert_eq!(removed, 2, "两个实例同批剔除");
        assert_eq!(reg.version(), 3, "批次整体 +1（非逐实例 +2）");
        assert!(reg.list_services().is_empty(), "实例清空后服务条目一并清理");
        let _ = (a, b);
    }

    /// 恢复 TTL 过滤（FR-4，固定 now_epoch 注入）：严格 `age > ttl`——
    /// age == ttl 保留（TC-08c 边界），age == ttl+1 丢弃。
    #[test]
    fn restored_filters_by_absolute_time_strict_boundary() {
        let now = 1_000_000u64;
        let mk = |epoch: u64| PersistedInstance {
            id: Uuid::new_v4(),
            address: "10.0.0.1".into(),
            port: 9000,
            metadata: HashMap::new(),
            last_heartbeat_epoch: epoch,
        };
        let mut services = BTreeMap::new();
        services.insert("svc".to_string(), vec![mk(now), mk(now - 30), mk(now - 31)]);
        let snapshot = RegistrySnapshot {
            format_version: SNAPSHOT_FORMAT_VERSION,
            saved_at_epoch: now,
            services,
        };
        let (reg, stats) = Registry::restored(ttl(30), snapshot, now);
        assert_eq!(stats.instances, 2, "age=0 与 age=30（==TTL）保留");
        assert_eq!(stats.dropped_expired, 1, "age=31（>TTL）丢弃");
        assert_eq!(stats.services, 1);
        // 全量列表含两个存活实例；healthy 判定按恢复后的真实流逝：
        // age==TTL 边界实例在恢复完成后的查询时刻已因微秒级流逝再次过期（TC-08 注明
        // 「相等边界在 HTTP 往返前即可再次过期」），因此不以其证明边界保留，
        // 边界语义由上面的 dropped/kept 计数证明；age=0 实例保持健康。
        let all = reg.list_instances("svc", false).unwrap();
        assert_eq!(all.len(), 2);
        let healthy = reg.list_instances("svc", true).unwrap();
        assert_eq!(healthy.len(), 1);
        assert_eq!(healthy[0].seconds_since_heartbeat, 0);
    }

    /// 恢复后同 id 可心跳（AC-2 领域层）：instance_id 不变。
    #[test]
    fn restored_keeps_instance_id_and_allows_heartbeat() {
        let now = now_epoch();
        let id = Uuid::new_v4();
        let mut services = BTreeMap::new();
        services.insert(
            "svc".to_string(),
            vec![PersistedInstance {
                id,
                address: "10.0.0.9".into(),
                port: 1234,
                metadata: HashMap::from([("k".to_string(), "中文\"引号\"\n换行".to_string())]),
                last_heartbeat_epoch: now,
            }],
        );
        let snapshot = RegistrySnapshot {
            format_version: SNAPSHOT_FORMAT_VERSION,
            saved_at_epoch: now,
            services,
        };
        let (reg, stats) = Registry::restored(ttl(60), snapshot, now);
        assert_eq!(stats.instances, 1);
        let view = &reg.list_instances("svc", false).unwrap()[0];
        assert_eq!(view.id, id);
        assert_eq!(
            view.metadata["k"], "中文\"引号\"\n换行",
            "B-12 metadata 内容完整恢复"
        );
        reg.heartbeat("svc", id).unwrap(); // 原实例 id 心跳成功（AC-2）
        reg.deregister("svc", id).unwrap();
    }

    /// dropped_expired>0 → 置 dirty（延迟清理）；==0 → 不置 dirty（G2-4）。
    #[test]
    fn restored_sets_dirty_only_when_dropped() {
        let now = 5_000_000u64;
        let mk = |epoch: u64| PersistedInstance {
            id: Uuid::new_v4(),
            address: "10.0.0.1".into(),
            port: 1,
            metadata: HashMap::new(),
            last_heartbeat_epoch: epoch,
        };
        // 无丢弃
        let mut fresh = BTreeMap::new();
        fresh.insert("svc".to_string(), vec![mk(now)]);
        let (reg, stats) = Registry::restored(
            ttl(30),
            RegistrySnapshot {
                format_version: SNAPSHOT_FORMAT_VERSION,
                saved_at_epoch: now,
                services: fresh,
            },
            now,
        );
        assert_eq!(stats.dropped_expired, 0);
        assert!(!reg.take_dirty(), "无丢弃不得置 dirty");

        // 有丢弃
        let mut stale = BTreeMap::new();
        stale.insert("svc".to_string(), vec![mk(now - 100)]);
        let (reg2, stats2) = Registry::restored(
            ttl(30),
            RegistrySnapshot {
                format_version: SNAPSHOT_FORMAT_VERSION,
                saved_at_epoch: now,
                services: stale,
            },
            now,
        );
        assert_eq!(stats2.dropped_expired, 1);
        assert!(reg2.take_dirty(), "dropped>0 必须置 dirty（文件延迟清理）");
        assert!(!reg2.take_dirty(), "take 原子取走后为 false");
    }

    /// dirty 状态机：mark 幂等、take 取走、mutation 置位、失败置回后再 take 可得。
    #[test]
    fn dirty_state_machine() {
        let reg = Registry::new(ttl(30));
        assert!(!reg.take_dirty());
        reg.mark_dirty();
        reg.mark_dirty(); // 幂等
        assert!(reg.take_dirty());
        assert!(!reg.take_dirty(), "take 后清除");

        reg.register("svc", "1.1.1.1".into(), 1, HashMap::new());
        assert!(reg.take_dirty(), "mutation 置位");
        // 模拟 flush 失败置回 → 无新 mutation 也能再 take（TC-05b）
        reg.mark_dirty();
        assert!(reg.take_dirty());
    }

    /// Instant↔epoch 换算往返：快照 epoch ≈ now − 已流逝秒数。
    #[test]
    fn snapshot_epoch_roundtrip() {
        let reg = Registry::new(ttl(30));
        reg.register("svc", "10.0.0.1".into(), 9000, HashMap::new());
        let snap = reg.snapshot();
        let now = now_epoch();
        let inst = &snap.services["svc"][0];
        assert!(now >= inst.last_heartbeat_epoch);
        assert!(now - inst.last_heartbeat_epoch <= 1, "秒精度误差 ≤1s");
        assert_eq!(snap.format_version, SNAPSHOT_FORMAT_VERSION);
        assert_eq!(snap.saved_at_epoch, now);
    }

    /// `checked_sub` 钳位兜底：age 超过进程运行时长时钳位到 now，不 panic（R2）。
    #[test]
    fn restored_clamps_huge_age_instead_of_panicking() {
        let now = now_epoch();
        let id = Uuid::new_v4();
        let mut services = BTreeMap::new();
        services.insert(
            "svc".to_string(),
            vec![PersistedInstance {
                id,
                address: "10.0.0.1".into(),
                port: 1,
                metadata: HashMap::new(),
                last_heartbeat_epoch: 0, // age ≈ 1.7e9 秒，远超进程运行时长
            }],
        );
        // TTL 取极大值使该实例通过 TTL 过滤（过滤在前，钳位在后）
        let ttl = Duration::from_secs(u64::MAX);
        let (reg, stats) = Registry::restored(
            ttl,
            RegistrySnapshot {
                format_version: SNAPSHOT_FORMAT_VERSION,
                saved_at_epoch: now,
                services,
            },
            now,
        );
        assert_eq!(
            stats.instances, 1,
            "通过 TTL 过滤的实例被保留（全程不 panic）"
        );
        // checked_sub 行为平台相关：macOS 的 Instant 为 timespec 表示，超大 age 仍可
        // 表示（返回 Some，视图秒数为极大值）；Linux uptime 表示则返回 None 走钳位
        // （视图秒数≈0）。两者都必须无 panic 且实例可查询——此处仅断言可查询性，
        // 不断言平台相关的秒数结果。
        let view = &reg.list_instances("svc", false).unwrap()[0];
        assert_eq!(view.id, id);
        assert_eq!(view.address, "10.0.0.1");
    }

    /// watch_snapshot：只含健康实例、无健康实例的服务不出现、服务名有序。
    #[test]
    fn watch_snapshot_shape() {
        let reg = Registry::new(Duration::from_millis(20));
        reg.register("b-svc", "2.2.2.2".into(), 2, HashMap::new());
        reg.register("a-svc", "1.1.1.1".into(), 1, HashMap::new());
        sleep(Duration::from_millis(40)); // 两服务全部过期（unhealthy，未剔除）
        let (version, map) = reg.watch_snapshot();
        assert_eq!(version, 2);
        assert!(map.is_empty(), "无健康实例的服务不出现在 watch 快照");
    }

    /// 变更广播：mutation 后订阅者可观察到新版本号（AC-8 前提）。
    #[test]
    fn change_channel_notifies() {
        let reg = Registry::new(ttl(30));
        let mut rx = reg.subscribe();
        assert_eq!(*rx.borrow(), 0);
        reg.register("svc", "1.1.1.1".into(), 1, HashMap::new());
        assert!(rx.has_changed().unwrap(), "mutation 后通道有新值");
        assert_eq!(*rx.borrow_and_update(), 1);
    }
}
