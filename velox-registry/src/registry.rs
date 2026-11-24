//! 服务注册领域模型：注册、心跳续约、注销、TTL 剔除与查询。
//!
//! 并发说明：`std::sync::RwLock` 仅保护短暂的内存读写，所有方法不跨 `.await`。

use std::collections::HashMap;
use std::sync::RwLock;
use std::time::{Duration, Instant};

use serde::Serialize;
use uuid::Uuid;

#[derive(Debug, thiserror::Error)]
pub enum RegistryError {
    #[error("instance '{instance}' not found in service '{service}'")]
    InstanceNotFound { service: String, instance: String },
    #[error("service '{0}' not found")]
    ServiceNotFound(String),
}

/// 实例内部表示（含心跳计时，不对外暴露）。
#[derive(Debug)]
struct Instance {
    address: String,
    port: u16,
    metadata: HashMap<String, String>,
    last_heartbeat: Instant,
}

/// 对外视图（HTTP / mesh 拉取快照共用此结构）。
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

/// 注册表：service 名 → 实例集合。
pub struct Registry {
    services: RwLock<HashMap<String, HashMap<Uuid, Instance>>>,
    ttl: Duration,
}

impl Registry {
    pub fn new(ttl: Duration) -> Self {
        Self {
            services: RwLock::new(HashMap::new()),
            ttl,
        }
    }

    /// 注册新实例，分配 `instance_id` 并返回初始视图。
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
        self.services
            .write()
            .unwrap()
            .entry(service.to_string())
            .or_default()
            .insert(id, instance);
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

    /// 心跳续约。
    pub fn heartbeat(&self, service: &str, id: Uuid) -> Result<(), RegistryError> {
        let mut services = self.services.write().unwrap();
        let instances = services
            .get_mut(service)
            .ok_or_else(|| RegistryError::ServiceNotFound(service.to_string()))?;
        let instance = instances
            .get_mut(&id)
            .ok_or_else(|| RegistryError::InstanceNotFound {
                service: service.to_string(),
                instance: id.to_string(),
            })?;
        instance.last_heartbeat = Instant::now();
        Ok(())
    }

    /// 主动注销；最后一个实例移除后连同服务条目一并清理。
    pub fn deregister(&self, service: &str, id: Uuid) -> Result<(), RegistryError> {
        let mut services = self.services.write().unwrap();
        let instances = services
            .get_mut(service)
            .ok_or_else(|| RegistryError::ServiceNotFound(service.to_string()))?;
        instances
            .remove(&id)
            .ok_or_else(|| RegistryError::InstanceNotFound {
                service: service.to_string(),
                instance: id.to_string(),
            })?;
        if instances.is_empty() {
            services.remove(service);
        }
        tracing::info!(service, instance = %id, "instance deregistered");
        Ok(())
    }

    /// 全部服务名（排序输出，便于调试与快照对比）。
    pub fn list_services(&self) -> Vec<String> {
        let mut names: Vec<String> = self.services.read().unwrap().keys().cloned().collect();
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
        let services = self.services.read().unwrap();
        let instances = services
            .get(service)
            .ok_or_else(|| RegistryError::ServiceNotFound(service.to_string()))?;
        let mut views: Vec<InstanceView> = instances
            .iter()
            .map(|(id, inst)| self.view(service, *id, inst))
            .filter(|v| !only_healthy || v.healthy)
            .collect();
        views.sort_by(|a, b| (&a.address, a.port).cmp(&(&b.address, b.port)));
        Ok(views)
    }

    /// 剔除心跳超时实例，返回剔除数量（含随之清空的服务条目）。
    pub fn sweep_expired(&self) -> usize {
        let mut services = self.services.write().unwrap();
        let mut removed = 0usize;
        services.retain(|service, instances| {
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
        removed
    }

    fn view(&self, service: &str, id: Uuid, inst: &Instance) -> InstanceView {
        let elapsed = inst.last_heartbeat.elapsed();
        InstanceView {
            id,
            service: service.to_string(),
            address: inst.address.clone(),
            port: inst.port,
            metadata: inst.metadata.clone(),
            seconds_since_heartbeat: elapsed.as_secs(),
            healthy: elapsed <= self.ttl,
        }
    }
}
