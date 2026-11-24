//! 服务发现：从 velox-registry 周期拉取健康实例快照，供代理转发使用。
//!
//! 降级语义：registry 不可达时沿用最后一次成功快照（只读），热路径不依赖中心组件。

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// registry 返回的实例视图（仅 mesh 感兴趣的字段，与 registry 的 InstanceView 对齐）。
#[derive(Debug, Deserialize)]
struct RegistryInstance {
    id: Uuid,
    address: String,
    port: u16,
}

/// 可转发的一个上游端点。
#[derive(Debug, Clone, Serialize)]
pub struct Endpoint {
    pub instance_id: Uuid,
    pub address: String,
    pub port: u16,
}

impl Endpoint {
    pub fn base_url(&self) -> String {
        format!("http://{}:{}", self.address, self.port)
    }
}

/// 一次成功拉取的全量快照（只含健康实例）。
#[derive(Debug, Default)]
pub struct Snapshot {
    /// BTreeMap 保证输出顺序稳定，便于调试对比
    pub services: BTreeMap<String, Vec<Endpoint>>,
    pub fetched_at: Option<Instant>,
}

impl Snapshot {
    pub fn service_count(&self) -> usize {
        self.services.len()
    }

    pub fn instance_count(&self) -> usize {
        self.services.values().map(|v| v.len()).sum()
    }

    pub fn age_secs(&self) -> Option<u64> {
        self.fetched_at.map(|t| t.elapsed().as_secs())
    }
}

pub struct Discovery {
    endpoint: String,
    http: reqwest::Client,
    /// 仅短暂替换 `Arc<Snapshot>`，不跨 `.await`，故用 std 锁
    snapshot: RwLock<Arc<Snapshot>>,
    /// 轮询负载均衡游标
    next: AtomicUsize,
}

impl Discovery {
    pub fn new(endpoint: String, http: reqwest::Client) -> Self {
        Self {
            endpoint,
            http,
            snapshot: RwLock::new(Arc::new(Snapshot::default())),
            next: AtomicUsize::new(0),
        }
    }

    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    /// 全量刷新快照；成功返回健康实例总数。
    pub async fn refresh(&self) -> anyhow::Result<usize> {
        let services: Vec<String> = self
            .http
            .get(format!("{}/api/v1/registry/services", self.endpoint))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;

        let mut map = BTreeMap::new();
        let mut total = 0usize;
        for service in services {
            let instances: Vec<RegistryInstance> = self
                .http
                .get(format!(
                    "{}/api/v1/registry/services/{service}/instances?healthy=true",
                    self.endpoint
                ))
                .send()
                .await?
                .error_for_status()?
                .json()
                .await?;
            total += instances.len();
            map.insert(
                service,
                instances
                    .into_iter()
                    .map(|i| Endpoint {
                        instance_id: i.id,
                        address: i.address,
                        port: i.port,
                    })
                    .collect(),
            );
        }

        let snapshot = Arc::new(Snapshot {
            services: map,
            fetched_at: Some(Instant::now()),
        });
        *self.snapshot.write().unwrap() = snapshot;
        Ok(total)
    }

    /// 当前快照（`Arc` 克隆，热路径读取几乎无锁开销）。
    pub fn snapshot(&self) -> Arc<Snapshot> {
        self.snapshot.read().unwrap().clone()
    }

    /// 轮询选取目标服务的一个健康实例。
    pub fn pick(&self, service: &str) -> Option<Endpoint> {
        let snapshot = self.snapshot();
        let endpoints = snapshot.services.get(service)?;
        if endpoints.is_empty() {
            return None;
        }
        let idx = self.next.fetch_add(1, Ordering::Relaxed) % endpoints.len();
        Some(endpoints[idx].clone())
    }
}

/// 后台刷新循环：失败只告警，不中断服务（降级到旧快照）。
pub async fn run_poller(discovery: Arc<Discovery>, interval: Duration) {
    let mut ticker = tokio::time::interval(interval);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        ticker.tick().await;
        match discovery.refresh().await {
            Ok(n) => tracing::debug!(instances = n, "registry snapshot refreshed"),
            Err(e) => tracing::warn!(error = %e, "registry unreachable, serving stale snapshot"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_discovery() -> Discovery {
        let http = reqwest::Client::new();
        Discovery::new("http://127.0.0.1:1".to_string(), http)
    }

    #[test]
    fn pick_on_empty_snapshot_is_none() {
        assert!(empty_discovery().pick("svc-a").is_none());
    }

    #[test]
    fn round_robin_cycles_instances() {
        let discovery = empty_discovery();
        let endpoints: Vec<Endpoint> = (0..3)
            .map(|i| Endpoint {
                instance_id: Uuid::new_v4(),
                address: format!("10.0.0.{i}"),
                port: 9000,
            })
            .collect();
        let snapshot = Arc::new(Snapshot {
            services: BTreeMap::from([("svc-a".to_string(), endpoints)]),
            fetched_at: Some(Instant::now()),
        });
        *discovery.snapshot.write().unwrap() = snapshot;

        let mut picked = Vec::new();
        for _ in 0..6 {
            picked.push(discovery.pick("svc-a").unwrap().address);
        }
        // 3 实例轮询 6 次：每个地址恰好出现 2 次
        for i in 0..3 {
            assert_eq!(
                picked
                    .iter()
                    .filter(|a| **a == format!("10.0.0.{i}"))
                    .count(),
                2
            );
        }
    }
}
