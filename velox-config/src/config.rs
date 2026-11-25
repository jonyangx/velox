//! 配置加载：TOML 文件（可选）+ 环境变量覆盖（`VELOX_CONFIG__<SECTION>__<FIELD>`）。

use serde::Deserialize;

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Config {
    pub server: ServerConfig,
    pub storage: StorageConfig,
}

const DEFAULT_MAX_CONCURRENT_REQUESTS: i64 = 10;

fn default_max_concurrent_requests() -> i64 {
    DEFAULT_MAX_CONCURRENT_REQUESTS
}

#[derive(Debug, Clone, Deserialize)]
pub struct ServerConfig {
    pub addr: String,
    /// 单实例最大并发处理请求数，≥1；未配置取 10。
    /// 注意：必须显式指定默认函数，裸 `#[serde(default)]` 会对 i64 取 Default（0，非法）。
    #[serde(default = "default_max_concurrent_requests")]
    pub max_concurrent_requests: i64,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            addr: "0.0.0.0:8200".to_string(),
            max_concurrent_requests: DEFAULT_MAX_CONCURRENT_REQUESTS,
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
        if let Some(v) = env_override("VELOX_CONFIG__SERVER__MAX_CONCURRENT_REQUESTS") {
            cfg.server.max_concurrent_requests = v.parse().map_err(|_| {
                anyhow::anyhow!(
                    "非法配置 VELOX_CONFIG__SERVER__MAX_CONCURRENT_REQUESTS：'{v}'（要求 ≥1 的整数）"
                )
            })?;
        }
        cfg.validate()?;
        Ok(cfg)
    }

    /// FR-6 fail fast：load 返回即保证合法，main 无需二次校验。
    fn validate(&self) -> anyhow::Result<()> {
        if self.server.max_concurrent_requests < 1 {
            anyhow::bail!(
                "非法配置 server.max_concurrent_requests：{}（要求 ≥1 的整数）",
                self.server.max_concurrent_requests
            );
        }
        Ok(())
    }

    /// 已由 validate 保证 ≥1，转换为信号量容量。
    pub fn max_concurrent_requests(&self) -> usize {
        self.server.max_concurrent_requests as usize
    }
}

fn env_override(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| !v.is_empty())
}

