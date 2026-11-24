//! 服务注册与心跳：向 velox-registry 注册本实例并维持心跳，关停时注销。
//!
//! 这是"服务消费方"角色的注册面职责（metadata role=consumer）。注册带退避重试
//! （registry 可能晚于本服务启动）；心跳周期必须小于 registry 的 TTL。

use std::sync::{Arc, RwLock};
use std::time::Duration;

use tokio::sync::watch;
use tracing::{info, warn};
use uuid::Uuid;

use crate::config::RegistryConfig;
use crate::error::LifecycleError;

#[derive(Debug, serde::Deserialize)]
struct RegisterResponse {
    id: Uuid,
}

pub struct Lifecycle {
    cfg: RegistryConfig,
    http: reqwest::Client,
    /// 注册成功后的实例 id（注册失败期间为 None）
    instance_id: RwLock<Option<Uuid>>,
}

impl Lifecycle {
    pub fn new(cfg: RegistryConfig, http: reqwest::Client) -> Self {
        Self {
            cfg,
            http,
            instance_id: RwLock::new(None),
        }
    }

    /// 当前实例 id；注册成功前为 None。
    pub fn instance_id(&self) -> Option<Uuid> {
        *self.instance_id.read().unwrap()
    }

    pub async fn register(&self) -> Result<Uuid, LifecycleError> {
        let url = format!(
            "{}/api/v1/registry/services/{}/instances",
            self.cfg.endpoint, self.cfg.service_name
        );
        let body = serde_json::json!({
            "address": self.cfg.advertise_host,
            "port": self.cfg.advertise_port,
            "metadata": {
                "role": "consumer",
                "version": env!("CARGO_PKG_VERSION"),
            },
        });
        let resp = self
            .http
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|source| LifecycleError::RegistryUnreachable {
                endpoint: self.cfg.endpoint.clone(),
                source,
            })?;
        if !resp.status().is_success() {
            return Err(LifecycleError::RegistryRejected {
                url,
                status: resp.status().as_u16(),
            });
        }
        let parsed: RegisterResponse =
            resp.json()
                .await
                .map_err(|source| LifecycleError::RegistryUnreachable {
                    endpoint: self.cfg.endpoint.clone(),
                    source,
                })?;
        *self.instance_id.write().unwrap() = Some(parsed.id);
        Ok(parsed.id)
    }

    pub async fn heartbeat(&self, id: Uuid) -> Result<(), LifecycleError> {
        let url = format!(
            "{}/api/v1/registry/services/{}/instances/{}/heartbeat",
            self.cfg.endpoint, self.cfg.service_name, id
        );
        self.send_ok(&url, reqwest::Method::PUT).await
    }

    pub async fn deregister(&self, id: Uuid) -> Result<(), LifecycleError> {
        let url = format!(
            "{}/api/v1/registry/services/{}/instances/{}",
            self.cfg.endpoint, self.cfg.service_name, id
        );
        self.send_ok(&url, reqwest::Method::DELETE).await
    }

    async fn send_ok(&self, url: &str, method: reqwest::Method) -> Result<(), LifecycleError> {
        let resp = self
            .http
            .request(method, url)
            .send()
            .await
            .map_err(|source| LifecycleError::RegistryUnreachable {
                endpoint: self.cfg.endpoint.clone(),
                source,
            })?;
        if !resp.status().is_success() {
            return Err(LifecycleError::RegistryRejected {
                url: url.to_string(),
                status: resp.status().as_u16(),
            });
        }
        Ok(())
    }

    /// 常驻任务：注册（退避重试）→ 心跳 → 收到关停信号后注销并返回。
    ///
    /// 通过 `shutdown` 通道实现优雅退出（开发规范第 3.3 节）。
    pub async fn run(self: Arc<Self>, mut shutdown: watch::Receiver<bool>) {
        // 注册循环：成功或关停二者其一
        let mut backoff_secs = 1u64;
        loop {
            tokio::select! {
                _ = shutdown.changed() => {
                    if *shutdown.borrow_and_update() {
                        info!("关停信号到达，注册尚未完成，跳过注册");
                        return;
                    }
                }
                res = self.register() => match res {
                    Ok(id) => {
                        info!(instance = %id, service = %self.cfg.service_name, "registered to velox-registry");
                        break;
                    }
                    Err(e) => {
                        warn!(error = %e, retry_in_secs = backoff_secs, "注册失败，稍后重试");
                        tokio::select! {
                            _ = shutdown.changed() => {
                                if *shutdown.borrow_and_update() { return; }
                            }
                            _ = tokio::time::sleep(Duration::from_secs(backoff_secs)) => {}
                        }
                        backoff_secs = (backoff_secs * 2).min(15);
                    }
                }
            }
        }
        let Some(id) = self.instance_id() else {
            return;
        };

        // 心跳循环
        let mut ticker =
            tokio::time::interval(Duration::from_secs(self.cfg.heartbeat_interval_secs.max(1)));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        ticker.tick().await; // 丢弃 interval 立即触发的首个 tick（注册时已视为活跃）
        loop {
            tokio::select! {
                _ = shutdown.changed() => {
                    if *shutdown.borrow_and_update() {
                        break;
                    }
                }
                _ = ticker.tick() => {
                    if let Err(e) = self.heartbeat(id).await {
                        warn!(error = %e, instance = %id, "心跳失败，下个周期重试");
                    }
                }
            }
        }

        // 优雅退出：尽力注销，失败不阻断进程退出（registry 会按 TTL 剔除兜底）
        match self.deregister(id).await {
            Ok(()) => info!(instance = %id, "已注销实例（优雅退出）"),
            Err(e) => warn!(error = %e, instance = %id, "注销失败，等待 registry 按 TTL 剔除"),
        }
    }
}
