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

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    /// 环境变量为进程级全局：所有触碰 env 的测试共享一把锁，避免并行互扰
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    const ENV_MAX: &str = "VELOX_CONFIG__SERVER__MAX_CONCURRENT_REQUESTS";

    /// 统一入口：持锁并清理目标环境变量，保证用例起点确定
    fn with_env_cleaned<F: FnOnce()>(f: F) {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::remove_var(ENV_MAX);
        f();
        std::env::remove_var(ENV_MAX);
    }

    fn write_toml(content: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!("velox-config-test-{}.toml", Uuid::new_v4()));
        std::fs::write(&path, content).unwrap();
        path
    }

    /// FR-5.1：未提供配置文件 → 默认 10
    #[test]
    fn default_when_no_file() {
        with_env_cleaned(|| {
            let cfg = Config::load(None).unwrap();
            assert_eq!(cfg.server.max_concurrent_requests, 10);
            assert_eq!(cfg.max_concurrent_requests(), 10);
        });
    }

    /// FR-5.2：配置文件无该键 → 默认 10（固化 serde 显式默认函数陷阱）
    #[test]
    fn default_when_key_missing() {
        with_env_cleaned(|| {
            let path = write_toml(
                "[server]\naddr = \"1.2.3.4:1\"\n\n[storage]\ndata_dir = \"/tmp/vx\"\n",
            );
            let cfg = Config::load(Some(path.to_str().unwrap())).unwrap();
            assert_eq!(cfg.server.max_concurrent_requests, 10);
            assert_eq!(cfg.server.addr, "1.2.3.4:1"); // 其余字段不受影响
        });
    }

    /// AC-5.3/4：环境变量覆盖优先于配置文件；仅设 env 时同样生效
    #[test]
    fn env_overrides_file() {
        with_env_cleaned(|| {
            let path = write_toml(
                "[server]\naddr = \"0.0.0.0:8200\"\nmax_concurrent_requests = 5\n",
            );
            std::env::set_var(ENV_MAX, "3");
            let cfg = Config::load(Some(path.to_str().unwrap())).unwrap();
            assert_eq!(cfg.server.max_concurrent_requests, 3);

            let cfg = Config::load(None).unwrap();
            assert_eq!(cfg.server.max_concurrent_requests, 3);
        });
    }

    /// FR-5.3：空字符串 env 沿用 env_override 过滤语义，回落默认 10
    #[test]
    fn env_empty_falls_back() {
        with_env_cleaned(|| {
            std::env::set_var(ENV_MAX, "");
            let cfg = Config::load(None).unwrap();
            assert_eq!(cfg.server.max_concurrent_requests, 10);
        });
    }

    /// FR-6：0 / -1（toml 或 env）→ 启动失败，错误含字段名与实际值
    #[test]
    fn invalid_zero_negative_fail_fast() {
        with_env_cleaned(|| {
            for v in ["0", "-1"] {
                let path = write_toml(&format!(
                    "[server]\naddr = \"0.0.0.0:8200\"\nmax_concurrent_requests = {v}\n"
                ));
                let err = Config::load(Some(path.to_str().unwrap())).unwrap_err().to_string();
                assert!(err.contains("max_concurrent_requests"), "实际：{err}");
                assert!(err.contains(v), "实际：{err}");
            }

            std::env::set_var(ENV_MAX, "-1");
            // env 负数成功反序列化后由 validate() 统一拒绝（AC-6：含字段名与实际值）
            let err = Config::load(None).unwrap_err().to_string();
            assert!(err.contains("max_concurrent_requests"), "实际：{err}");
            assert!(err.contains("-1"), "实际：{err}");
        });
    }

    /// FR-6：非整数/非数字 → 解析失败，启动终止
    #[test]
    fn invalid_float_string_fail() {
        with_env_cleaned(|| {
            // toml：1.5（浮点）与 "abc"（字符串）被 serde 拒绝，报错含键名与原值
            for v in ["1.5", "\"abc\""] {
                let path = write_toml(&format!(
                    "[server]\naddr = \"0.0.0.0:8200\"\nmax_concurrent_requests = {v}\n"
                ));
                let err = Config::load(Some(path.to_str().unwrap())).unwrap_err().to_string();
                assert!(err.contains("max_concurrent_requests"), "实际：{err}");
                assert!(err.contains(v.trim_matches('"')), "实际：{err}");
            }

            // env：非数字 parse::<i64> 失败，报错含变量名与原始值
            for v in ["abc", "1.5"] {
                std::env::set_var(ENV_MAX, v);
                let err = Config::load(None).unwrap_err().to_string();
                assert!(err.contains(ENV_MAX), "实际：{err}");
                assert!(err.contains(v), "实际：{err}");
            }
        });
    }

    /// AC-6 尾条 + Q6 不封顶：合法极值 1 与 100000 正常加载并生效
    #[test]
    fn valid_extremes() {
        with_env_cleaned(|| {
            for v in ["1", "100000"] {
                let path = write_toml(&format!(
                    "[server]\naddr = \"0.0.0.0:8200\"\nmax_concurrent_requests = {v}\n"
                ));
                let cfg = Config::load(Some(path.to_str().unwrap())).unwrap();
                assert_eq!(cfg.server.max_concurrent_requests, v.parse::<i64>().unwrap());
            }
        });
    }
}
