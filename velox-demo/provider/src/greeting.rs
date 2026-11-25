//! 领域逻辑：问候语模板管理 + 从 velox-config 周期刷新。
//!
//! 模板优先取配置中心（`{namespace}/{key}`，JSON 字符串值，支持 `{name}` 占位符）；
//! 未配置或拉取失败时沿用当前值（首次为内置默认），保证服务可用性不依赖配置中心。

use std::sync::{Arc, RwLock};
use std::time::Duration;

use tokio::sync::watch;
use tracing::{debug, warn};

use crate::config::ConfigCenterConfig;
use crate::error::ConfigFetchError;

pub const DEFAULT_TEMPLATE: &str = "hello, {name}! (builtin default template)";

pub struct Greeting {
    template: RwLock<String>,
    /// 当前模板是否来自配置中心（false = 内置默认值）
    is_from_config_center: RwLock<bool>,
}

impl Default for Greeting {
    fn default() -> Self {
        Self::new()
    }
}

impl Greeting {
    pub fn new() -> Self {
        Self {
            template: RwLock::new(DEFAULT_TEMPLATE.to_string()),
            is_from_config_center: RwLock::new(false),
        }
    }

    /// 按当前模板渲染问候语。
    pub fn render(&self, name: &str) -> String {
        self.template.read().unwrap().replace("{name}", name)
    }

    pub fn template(&self) -> String {
        self.template.read().unwrap().clone()
    }

    pub fn is_from_config_center(&self) -> bool {
        *self.is_from_config_center.read().unwrap()
    }

    fn apply(&self, template: String) {
        *self.template.write().unwrap() = template;
        *self.is_from_config_center.write().unwrap() = true;
    }

    async fn fetch_once(
        &self,
        cfg: &ConfigCenterConfig,
        http: &reqwest::Client,
    ) -> Result<(), ConfigFetchError> {
        let url = format!(
            "{}/api/v1/config/{}/{}",
            cfg.endpoint, cfg.namespace, cfg.key
        );
        let resp = http
            .get(&url)
            .send()
            .await
            .map_err(|source| ConfigFetchError::Unreachable {
                endpoint: cfg.endpoint.clone(),
                source,
            })?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            debug!(namespace = %cfg.namespace, key = %cfg.key, "配置中心未配置该项，沿用当前模板");
            return Ok(());
        }
        if !resp.status().is_success() {
            return Err(ConfigFetchError::Rejected {
                url,
                status: resp.status().as_u16(),
            });
        }
        let value: serde_json::Value =
            resp.json()
                .await
                .map_err(|source| ConfigFetchError::InvalidBody {
                    url: url.clone(),
                    source,
                })?;
        match value.as_str() {
            Some(s) => {
                self.apply(s.to_string());
                debug!(namespace = %cfg.namespace, key = %cfg.key, "模板已从配置中心刷新");
                Ok(())
            }
            None => {
                debug!(namespace = %cfg.namespace, key = %cfg.key, "配置值不是字符串，忽略");
                Ok(())
            }
        }
    }

    /// 常驻任务：启动即拉取一次，之后按周期刷新；关停信号到达即退出。
    pub async fn run_config_refresh(
        self: Arc<Self>,
        cfg: ConfigCenterConfig,
        http: reqwest::Client,
        mut shutdown: watch::Receiver<bool>,
    ) {
        if let Err(e) = self.fetch_once(&cfg, &http).await {
            debug!(error = %e, "启动期模板拉取失败，暂用当前模板");
        }
        let mut ticker =
            tokio::time::interval(Duration::from_secs(cfg.refresh_interval_secs.max(1)));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        ticker.tick().await; // 丢弃 interval 立即触发的首个 tick
        loop {
            tokio::select! {
                _ = shutdown.changed() => {
                    if *shutdown.borrow_and_update() {
                        return;
                    }
                }
                _ = ticker.tick() => {
                    if let Err(e) = self.fetch_once(&cfg, &http).await {
                        warn!(error = %e, namespace = %cfg.namespace, key = %cfg.key, "模板刷新失败，沿用当前值");
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_replaces_name_placeholder() {
        let g = Greeting::new();
        assert_eq!(
            g.render("velox"),
            "hello, velox! (builtin default template)"
        );
        assert!(!g.is_from_config_center());
    }

    #[test]
    fn apply_switches_source_and_template() {
        let g = Greeting::new();
        g.apply("你好 {name}".to_string());
        assert_eq!(g.render("世界"), "你好 世界");
        assert!(g.is_from_config_center());
    }
}
