//! 配置加载：TOML 文件（可选）+ 环境变量覆盖（`VELOX_PROVIDER__<SECTION>__<FIELD>`）。
//!
//! 约定与 velox 子系统一致（见开发规范第 5 节）：配置文件为第一个位置参数，
//! 环境变量双下划线分层，未知字段容忍。

use serde::Deserialize;

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Config {
    pub server: ServerConfig,
    pub registry: RegistryConfig,
    pub config_center: ConfigCenterConfig,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ServerConfig {
    pub addr: String,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            addr: "0.0.0.0:9000".to_string(),
        }
    }
}

/// 服务提供方在 velox-registry 中的身份与地址。
#[derive(Debug, Clone, Deserialize)]
pub struct RegistryConfig {
    /// velox-registry 地址
    pub endpoint: String,
    /// 注册使用的服务名（mesh 经此名称发现本服务）
    pub service_name: String,
    /// 对外通告的主机名/IP（必须 mesh 可达；容器中为服务名）
    pub advertise_host: String,
    pub advertise_port: u16,
    /// 心跳周期（必须小于 registry 的 heartbeat_ttl_secs）
    pub heartbeat_interval_secs: u64,
}

impl Default for RegistryConfig {
    fn default() -> Self {
        Self {
            endpoint: "http://127.0.0.1:8300".to_string(),
            service_name: "demo-provider".to_string(),
            advertise_host: "127.0.0.1".to_string(),
            advertise_port: 9000,
            heartbeat_interval_secs: 5,
        }
    }
}

/// velox-config 对接：问候语模板的命名空间与键。
#[derive(Debug, Clone, Deserialize)]
pub struct ConfigCenterConfig {
    pub endpoint: String,
    pub namespace: String,
    pub key: String,
    pub refresh_interval_secs: u64,
}

impl Default for ConfigCenterConfig {
    fn default() -> Self {
        Self {
            endpoint: "http://127.0.0.1:8200".to_string(),
            namespace: "demo".to_string(),
            key: "greeting-template".to_string(),
            refresh_interval_secs: 5,
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

        if let Some(v) = env_override("VELOX_PROVIDER__SERVER__ADDR") {
            cfg.server.addr = v;
        }
        if let Some(v) = env_override("VELOX_PROVIDER__REGISTRY__ENDPOINT") {
            cfg.registry.endpoint = v;
        }
        if let Some(v) = env_override("VELOX_PROVIDER__REGISTRY__SERVICE_NAME") {
            cfg.registry.service_name = v;
        }
        if let Some(v) = env_override("VELOX_PROVIDER__REGISTRY__ADVERTISE_HOST") {
            cfg.registry.advertise_host = v;
        }
        if let Some(v) = env_override("VELOX_PROVIDER__REGISTRY__ADVERTISE_PORT") {
            cfg.registry.advertise_port = v
                .parse()
                .map_err(|_| anyhow::anyhow!("非法 ADVERTISE_PORT：'{v}'"))?;
        }
        if let Some(v) = env_override("VELOX_PROVIDER__REGISTRY__HEARTBEAT_INTERVAL_SECS") {
            cfg.registry.heartbeat_interval_secs = v
                .parse()
                .map_err(|_| anyhow::anyhow!("非法 HEARTBEAT_INTERVAL_SECS：'{v}'"))?;
        }
        if let Some(v) = env_override("VELOX_PROVIDER__CONFIG_CENTER__ENDPOINT") {
            cfg.config_center.endpoint = v;
        }
        if let Some(v) = env_override("VELOX_PROVIDER__CONFIG_CENTER__NAMESPACE") {
            cfg.config_center.namespace = v;
        }
        if let Some(v) = env_override("VELOX_PROVIDER__CONFIG_CENTER__KEY") {
            cfg.config_center.key = v;
        }
        if let Some(v) = env_override("VELOX_PROVIDER__CONFIG_CENTER__REFRESH_INTERVAL_SECS") {
            cfg.config_center.refresh_interval_secs = v
                .parse()
                .map_err(|_| anyhow::anyhow!("非法 REFRESH_INTERVAL_SECS：'{v}'"))?;
        }
        Ok(cfg)
    }
}

fn env_override(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| !v.is_empty())
}
