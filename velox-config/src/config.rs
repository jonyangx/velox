//! 配置加载：TOML 文件（可选）+ 环境变量覆盖（`VELOX_CONFIG__<SECTION>__<FIELD>`）。

use serde::Deserialize;

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Config {
    pub server: ServerConfig,
    pub storage: StorageConfig,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ServerConfig {
    pub addr: String,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            addr: "0.0.0.0:8200".to_string(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct StorageConfig {
    pub data_dir: String,
}

impl Default for StorageConfig {
    fn default() -> Self {
        Self {
            data_dir: "./data".to_string(),
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

        if let Some(v) = env_override("VELOX_CONFIG__SERVER__ADDR") {
            cfg.server.addr = v;
        }
        if let Some(v) = env_override("VELOX_CONFIG__STORAGE__DATA_DIR") {
            cfg.storage.data_dir = v;
        }
        Ok(cfg)
    }
}

fn env_override(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| !v.is_empty())
}
