//! 配置加载：TOML 文件（可选）+ 环境变量覆盖（`VELOX_MESH__<SECTION>__<FIELD>`）。

use serde::Deserialize;
use velox_log::{LogConfig, ResolvedLevel};

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Config {
    pub server: ServerConfig,
    pub registry: RegistryConfig,
    pub proxy: ProxyConfig,

    /// 日志配置（addlog）：目录/等级；`log_level` 为 load 末尾解析的生效等级
    /// 与来源（不参与 TOML 反序列化，由 resolve_level 填充）。
    pub log: LogConfig,
    #[serde(skip)]
    pub log_level: ResolvedLevel,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ServerConfig {
    pub inbound_addr: String,
    pub admin_addr: String,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            inbound_addr: "0.0.0.0:8100".to_string(),
            admin_addr: "127.0.0.1:8101".to_string(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct RegistryConfig {
    pub endpoint: String,
    pub poll_secs: u64,
}

impl Default for RegistryConfig {
    fn default() -> Self {
        Self {
            endpoint: "http://127.0.0.1:8300".to_string(),
            poll_secs: 5,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct ProxyConfig {
    pub connect_timeout_secs: u64,
    pub request_timeout_secs: u64,
}

impl Default for ProxyConfig {
    fn default() -> Self {
        Self {
            connect_timeout_secs: 3,
            request_timeout_secs: 30,
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

        if let Some(v) = env_override("VELOX_MESH__SERVER__INBOUND_ADDR") {
            cfg.server.inbound_addr = v;
        }
        if let Some(v) = env_override("VELOX_MESH__SERVER__ADMIN_ADDR") {
            cfg.server.admin_addr = v;
        }
        if let Some(v) = env_override("VELOX_MESH__REGISTRY__ENDPOINT") {
            cfg.registry.endpoint = v;
        }
        if let Some(v) = env_override("VELOX_MESH__REGISTRY__POLL_SECS") {
            cfg.registry.poll_secs = v
                .parse()
                .map_err(|_| anyhow::anyhow!("非法 POLL_SECS：'{v}'"))?;
        }
        if let Some(v) = env_override("VELOX_MESH__PROXY__CONNECT_TIMEOUT_SECS") {
            cfg.proxy.connect_timeout_secs = v
                .parse()
                .map_err(|_| anyhow::anyhow!("非法 CONNECT_TIMEOUT_SECS：'{v}'"))?;
        }
        if let Some(v) = env_override("VELOX_MESH__PROXY__REQUEST_TIMEOUT_SECS") {
            cfg.proxy.request_timeout_secs = v
                .parse()
                .map_err(|_| anyhow::anyhow!("非法 REQUEST_TIMEOUT_SECS：'{v}'"))?;
        }

        // 日志目录 env 覆盖（沿用非空 env_override 惯例）；level 不经该 helper，
        // 由 resolve_level 直接读 var_os（保留空串语义，见 velox-log §4.4）
        if let Some(v) = env_override("VELOX_MESH__LOG__DIR") {
            cfg.log.dir = v;
        }
        // 等级链解析（addlog）：失败 fail-fast（AC-10），真实配置路径作为文件来源上下文
        let abs_path = path.map(|p| velox_log::absolutize(std::path::Path::new(p)));
        cfg.log_level = velox_log::resolve_level(&cfg.log, abs_path.as_deref(), "MESH")?;

        Ok(cfg)
    }
}

fn env_override(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| !v.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_valid_socket_addrs() {
        let cfg = Config::load(None).unwrap();
        cfg.server
            .inbound_addr
            .parse::<std::net::SocketAddr>()
            .unwrap();
        cfg.server
            .admin_addr
            .parse::<std::net::SocketAddr>()
            .unwrap();
        assert_eq!(cfg.registry.poll_secs, 5);
    }

    #[test]
    fn env_overrides_apply() {
        // 注：环境变量在并发测试中可能互相影响，此处仅验证存在性逻辑；端到端覆盖见集成测试
        assert!(env_override("VELOX_MESH__NON_EXISTENT_KEY").is_none());
    }
}
