//! 配置加载：TOML 文件（可选）+ 环境变量覆盖。
//!
//! 环境变量分层约定（见开发规范第 5 节）：`VELOX_REGISTRY__<SECTION>__<FIELD>`。

use serde::Deserialize;

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Config {
    pub server: ServerConfig,
    pub registry: RegistryConfig,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ServerConfig {
    pub addr: String,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            addr: "0.0.0.0:8300".to_string(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct RegistryConfig {
    pub heartbeat_ttl_secs: u64,
    pub sweep_interval_secs: u64,
}

impl Default for RegistryConfig {
    fn default() -> Self {
        Self {
            heartbeat_ttl_secs: 30,
            sweep_interval_secs: 5,
        }
    }
}

impl Config {
    /// `path` 为命令行传入的配置文件路径；缺省时全部使用默认值。
    pub fn load(path: Option<&str>) -> anyhow::Result<Self> {
        let mut cfg: Config = match path {
            Some(p) => {
                let raw = std::fs::read_to_string(p)
                    .map_err(|e| anyhow::anyhow!("读取配置文件 {p} 失败：{e}"))?;
                toml::from_str(&raw).map_err(|e| anyhow::anyhow!("解析配置文件 {p} 失败：{e}"))?
            }
            None => Config::default(),
        };

        if let Some(v) = env_override("VELOX_REGISTRY__SERVER__ADDR") {
            cfg.server.addr = v;
        }
        if let Some(v) = env_override("VELOX_REGISTRY__REGISTRY__HEARTBEAT_TTL_SECS") {
            cfg.registry.heartbeat_ttl_secs = v
                .parse()
                .map_err(|_| anyhow::anyhow!("非法 HEARTBEAT_TTL_SECS：'{v}'"))?;
        }
        if let Some(v) = env_override("VELOX_REGISTRY__REGISTRY__SWEEP_INTERVAL_SECS") {
            cfg.registry.sweep_interval_secs = v
                .parse()
                .map_err(|_| anyhow::anyhow!("非法 SWEEP_INTERVAL_SECS：'{v}'"))?;
        }
        Ok(cfg)
    }
}

fn env_override(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| !v.is_empty())
}
