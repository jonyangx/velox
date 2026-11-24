//! 配置加载：TOML 文件（可选）+ 环境变量覆盖（`VELOX_CONSUMER__<SECTION>__<FIELD>`）。
//!
//! 约定与 velox 子系统一致（见开发规范第 5 节）：配置文件为第一个位置参数，
//! 环境变量双下划线分层，空值忽略，未知字段容忍。

use serde::Deserialize;

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Config {
    pub server: ServerConfig,
    pub registry: RegistryConfig,
    pub mesh: MeshConfig,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct ServerConfig {
    pub addr: String,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            addr: "0.0.0.0:9100".to_string(),
        }
    }
}

/// 服务消费方在 velox-registry 中的身份与地址。
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct RegistryConfig {
    /// velox-registry 地址
    pub endpoint: String,
    /// 注册使用的服务名
    pub service_name: String,
    /// 对外通告的主机名/IP
    pub advertise_host: String,
    pub advertise_port: u16,
    /// 心跳周期（必须小于 registry 的 heartbeat_ttl_secs）
    pub heartbeat_interval_secs: u64,
}

impl Default for RegistryConfig {
    fn default() -> Self {
        Self {
            endpoint: "http://127.0.0.1:8300".to_string(),
            service_name: "demo-consumer".to_string(),
            advertise_host: "127.0.0.1".to_string(),
            advertise_port: 9100,
            heartbeat_interval_secs: 5,
        }
    }
}

/// velox-mesh 对接：出口流量的代理入口与请求超时。
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct MeshConfig {
    /// velox-mesh 入口地址
    pub endpoint: String,
    /// 经 mesh 调用上游的单请求超时（秒）
    pub request_timeout_secs: u64,
}

impl Default for MeshConfig {
    fn default() -> Self {
        Self {
            endpoint: "http://127.0.0.1:8100".to_string(),
            request_timeout_secs: 10,
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

        if let Some(v) = env_override("VELOX_CONSUMER__SERVER__ADDR") {
            cfg.server.addr = v;
        }
        if let Some(v) = env_override("VELOX_CONSUMER__REGISTRY__ENDPOINT") {
            cfg.registry.endpoint = v;
        }
        if let Some(v) = env_override("VELOX_CONSUMER__REGISTRY__SERVICE_NAME") {
            cfg.registry.service_name = v;
        }
        if let Some(v) = env_override("VELOX_CONSUMER__REGISTRY__ADVERTISE_HOST") {
            cfg.registry.advertise_host = v;
        }
        if let Some(v) = env_override("VELOX_CONSUMER__REGISTRY__ADVERTISE_PORT") {
            cfg.registry.advertise_port = v
                .parse()
                .map_err(|_| anyhow::anyhow!("非法 ADVERTISE_PORT：'{v}'"))?;
        }
        if let Some(v) = env_override("VELOX_CONSUMER__REGISTRY__HEARTBEAT_INTERVAL_SECS") {
            cfg.registry.heartbeat_interval_secs = v
                .parse()
                .map_err(|_| anyhow::anyhow!("非法 HEARTBEAT_INTERVAL_SECS：'{v}'"))?;
        }
        if let Some(v) = env_override("VELOX_CONSUMER__MESH__ENDPOINT") {
            cfg.mesh.endpoint = v;
        }
        if let Some(v) = env_override("VELOX_CONSUMER__MESH__REQUEST_TIMEOUT_SECS") {
            cfg.mesh.request_timeout_secs = v
                .parse()
                .map_err(|_| anyhow::anyhow!("非法 REQUEST_TIMEOUT_SECS：'{v}'"))?;
        }
        Ok(cfg)
    }
}

fn env_override(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| !v.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 环境变量是进程全局的：凡改环境变量的测试串行执行，避免互相踩踏。
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn defaults_match_contract() {
        let cfg = Config::load(None).expect("默认配置加载失败");
        assert_eq!(cfg.server.addr, "0.0.0.0:9100");
        assert_eq!(cfg.registry.endpoint, "http://127.0.0.1:8300");
        assert_eq!(cfg.registry.service_name, "demo-consumer");
        assert_eq!(cfg.registry.advertise_host, "127.0.0.1");
        assert_eq!(cfg.registry.advertise_port, 9100);
        assert_eq!(cfg.registry.heartbeat_interval_secs, 5);
        assert_eq!(cfg.mesh.endpoint, "http://127.0.0.1:8100");
        assert_eq!(cfg.mesh.request_timeout_secs, 10);
    }

    #[test]
    fn loads_from_toml_file_and_tolerates_unknown_fields() {
        let dir = std::env::temp_dir().join("demo-consumer-config-test");
        std::fs::create_dir_all(&dir).expect("创建临时目录失败");
        let path = dir.join("consumer.toml");
        std::fs::write(
            &path,
            r#"
[server]
addr = "0.0.0.0:9300"
extra_unknown = "忽略我"

[registry]
endpoint = "http://127.0.0.1:18000"

# 未知 section：容忍
[unknown_section]
foo = "bar"
"#,
        )
        .expect("写入临时配置失败");

        let cfg =
            Config::load(Some(path.to_str().expect("临时路径非法"))).expect("加载临时配置失败");
        assert_eq!(cfg.server.addr, "0.0.0.0:9300");
        assert_eq!(cfg.registry.endpoint, "http://127.0.0.1:18000");
        // 部分 section：缺失字段沿用默认值
        assert_eq!(cfg.registry.service_name, "demo-consumer");
        assert_eq!(cfg.registry.advertise_port, 9100);
        // 未覆盖的 section 沿用默认值
        assert_eq!(cfg.mesh.endpoint, "http://127.0.0.1:8100");
    }

    #[test]
    fn env_overrides_take_precedence() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::set_var("VELOX_CONSUMER__SERVER__ADDR", "0.0.0.0:19999");
        std::env::set_var("VELOX_CONSUMER__MESH__ENDPOINT", "http://127.0.0.1:18001");
        std::env::set_var("VELOX_CONSUMER__REGISTRY__ADVERTISE_PORT", "19998");
        // 空值必须被忽略
        std::env::set_var("VELOX_CONSUMER__REGISTRY__SERVICE_NAME", "");

        let cfg = Config::load(None).expect("环境覆盖加载失败");
        assert_eq!(cfg.server.addr, "0.0.0.0:19999");
        assert_eq!(cfg.mesh.endpoint, "http://127.0.0.1:18001");
        assert_eq!(cfg.registry.advertise_port, 19998);
        assert_eq!(cfg.registry.service_name, "demo-consumer");

        std::env::remove_var("VELOX_CONSUMER__SERVER__ADDR");
        std::env::remove_var("VELOX_CONSUMER__MESH__ENDPOINT");
        std::env::remove_var("VELOX_CONSUMER__REGISTRY__ADVERTISE_PORT");
        std::env::remove_var("VELOX_CONSUMER__REGISTRY__SERVICE_NAME");
    }

    #[test]
    fn invalid_number_env_value_is_rejected() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::set_var("VELOX_CONSUMER__MESH__REQUEST_TIMEOUT_SECS", "not-a-number");
        let res = Config::load(None);
        std::env::remove_var("VELOX_CONSUMER__MESH__REQUEST_TIMEOUT_SECS");
        assert!(res.is_err(), "非法数字必须报错");
    }
}
