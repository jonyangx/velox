//! 配置加载：TOML 文件（可选）+ 环境变量覆盖（`VELOX_CONFIG__<SECTION>__<FIELD>`）。

use serde::Deserialize;
use velox_log::{LogConfig, ResolvedLevel};

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Config {
    pub server: ServerConfig,
    pub storage: StorageConfig,

    /// 日志配置（addlog）：目录/等级；`log_level` 为 load 末尾解析的生效等级
    /// 与来源（不参与 TOML 反序列化，由 resolve_level 填充）。
    pub log: LogConfig,
    #[serde(skip)]
    pub log_level: ResolvedLevel,
}

const DEFAULT_MAX_CONCURRENT_REQUESTS: i64 = 10;

/// 并发上限合法域上界 U = min(i64::MAX, usize::MAX, Semaphore::MAX_PERMITS)（冻结 FR-6/Q6）。
/// 以 u128 无损计算：i64::MAX、usize::MAX、MAX_PERMITS 均可无损表入 u128（const 可求值）。
/// 64 位平台 U = 2305843009213693951，32 位 = 536870911，均 < i64::MAX
/// （tokio 1.54.1 semaphore.rs:479 公开常量 MAX_PERMITS = usize::MAX >> 3）。
const fn min_u128(a: u128, b: u128) -> u128 {
    if a < b {
        a
    } else {
        b
    }
}

const MAX_CONCURRENT_REQUESTS_CAP: u128 = min_u128(
    min_u128(i64::MAX as u128, usize::MAX as u128),
    tokio::sync::Semaphore::MAX_PERMITS as u128,
);

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

        // 日志目录 env 覆盖（沿用非空 env_override 惯例）；level 不经该 helper，
        // 由 resolve_level 直接读 var_os（保留空串语义，见 velox-log §4.4）
        if let Some(v) = env_override("VELOX_CONFIG__LOG__DIR") {
            cfg.log.dir = v;
        }
        // 等级链解析（addlog）：失败 fail-fast（AC-10），真实配置路径作为文件来源上下文
        let abs_path = path.map(|p| velox_log::absolutize(std::path::Path::new(p)));
        cfg.log_level = velox_log::resolve_level(&cfg.log, abs_path.as_deref(), "CONFIG")?;

        Ok(cfg)
    }

    /// FR-6/Q4/Q6 fail fast：合法域为整数 1..=U；`load` 返回即保证合法，main 无需二次校验。
    /// 比较在 u128 无损域中进行——**不得先 `as usize` 再校验**（32 位平台上 i64::MAX
    /// 会被截断成小值而漏判；这正是本轮修复的真实缺口）。
    fn validate(&self) -> anyhow::Result<()> {
        let v = self.server.max_concurrent_requests;
        if v < 1 {
            anyhow::bail!("非法配置 server.max_concurrent_requests：{v}（要求 ≥1 的整数）");
        }
        if v as u128 > MAX_CONCURRENT_REQUESTS_CAP {
            anyhow::bail!(
                "非法配置 server.max_concurrent_requests：{v}（超过本平台信号量技术上界 U={MAX_CONCURRENT_REQUESTS_CAP}）"
            );
        }
        Ok(())
    }

    /// 信号量容量。不变式：`load()` 已 validate 保证 1..=U，且 U ≤ usize::MAX
    /// （U 的定义含 min(usize::MAX)），故 `as usize` 无损；debug_assert 固化该不变式，
    /// 正式约束由 validate 承担，运行时零开销。
    pub fn max_concurrent_requests(&self) -> usize {
        debug_assert!(self.server.max_concurrent_requests >= 1);
        debug_assert!((self.server.max_concurrent_requests as u128) <= MAX_CONCURRENT_REQUESTS_CAP);
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
            let path =
                write_toml("[server]\naddr = \"1.2.3.4:1\"\n\n[storage]\ndata_dir = \"/tmp/vx\"\n");
            let cfg = Config::load(Some(path.to_str().unwrap())).unwrap();
            assert_eq!(cfg.server.max_concurrent_requests, 10);
            assert_eq!(cfg.server.addr, "1.2.3.4:1"); // 其余字段不受影响
        });
    }

    /// AC-5.3/4：环境变量覆盖优先于配置文件；仅设 env 时同样生效
    #[test]
    fn env_overrides_file() {
        with_env_cleaned(|| {
            let path =
                write_toml("[server]\naddr = \"0.0.0.0:8200\"\nmax_concurrent_requests = 5\n");
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
                let err = Config::load(Some(path.to_str().unwrap()))
                    .unwrap_err()
                    .to_string();
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
                let err = Config::load(Some(path.to_str().unwrap()))
                    .unwrap_err()
                    .to_string();
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
                assert_eq!(
                    cfg.server.max_concurrent_requests,
                    v.parse::<i64>().unwrap()
                );
            }
        });
    }

    /// TC-08 情况 5（G1-3）：文件有值 + 无 env → 保留文件值
    #[test]
    fn file_value_kept_without_env() {
        with_env_cleaned(|| {
            let path =
                write_toml("[server]\naddr = \"0.0.0.0:8200\"\nmax_concurrent_requests = 5\n");
            let cfg = Config::load(Some(path.to_str().unwrap())).unwrap();
            assert_eq!(cfg.server.max_concurrent_requests, 5, "空 env 不覆盖文件值");
        });
    }

    /// TC-08 情况 6（G1-3）：文件有值 + 空 env → 不覆盖，保留文件值 5（**不得断言回落 10**）
    #[test]
    fn file_value_kept_with_empty_env() {
        with_env_cleaned(|| {
            let path =
                write_toml("[server]\naddr = \"0.0.0.0:8200\"\nmax_concurrent_requests = 5\n");
            std::env::set_var(ENV_MAX, "");
            let cfg = Config::load(Some(path.to_str().unwrap())).unwrap();
            assert_eq!(cfg.server.max_concurrent_requests, 5);
        });
    }

    /// TC-08 情况 7（G1-3）：配置文件缺键 + 空 env → 默认 10
    #[test]
    fn empty_env_when_key_missing_defaults_to_10() {
        with_env_cleaned(|| {
            let path = write_toml("[server]\naddr = \"0.0.0.0:8200\"\n");
            std::env::set_var(ENV_MAX, "");
            let cfg = Config::load(Some(path.to_str().unwrap())).unwrap();
            assert_eq!(cfg.server.max_concurrent_requests, 10);
        });
    }

    /// TC-09 组合语义：文件非法值 + 非空合法 env = 选中 env（文件值被覆盖、不触发校验）；
    /// 文件合法 + 非空非法 env = 按 env 报错、不回落文件值
    #[test]
    fn illegal_file_with_valid_env_selects_env() {
        with_env_cleaned(|| {
            // 文件 0（本会非法）+ env 3 → 加载成功且生效 3
            let path =
                write_toml("[server]\naddr = \"0.0.0.0:8200\"\nmax_concurrent_requests = 0\n");
            std::env::set_var(ENV_MAX, "3");
            let cfg = Config::load(Some(path.to_str().unwrap())).unwrap();
            assert_eq!(cfg.server.max_concurrent_requests, 3);
            std::env::remove_var(ENV_MAX);

            // 文件 5（合法）+ env 0（非法）→ validate 拒绝 env 覆盖后的 0，不回落文件值 5
            let path =
                write_toml("[server]\naddr = \"0.0.0.0:8200\"\nmax_concurrent_requests = 5\n");
            std::env::set_var(ENV_MAX, "0");
            let err = Config::load(Some(path.to_str().unwrap()))
                .unwrap_err()
                .to_string();
            assert!(
                err.contains("server.max_concurrent_requests"),
                "含字段名（实际：{err}）"
            );
            assert!(err.contains('0'), "含实际值 0（实际：{err}）");
            std::env::remove_var(ENV_MAX);

            // 文件 5（合法）+ env "abc"（非数字）→ parse 阶段拒绝，含 env 全名与原值，不回落文件值 5
            std::env::set_var(ENV_MAX, "abc");
            let err = Config::load(Some(path.to_str().unwrap()))
                .unwrap_err()
                .to_string();
            assert!(err.contains(ENV_MAX), "含 env 全名（实际：{err}）");
            assert!(err.contains("abc"), "含原值（实际：{err}）");
        });
    }

    /// TC-10/12（FR-6/Q6）：技术超界 U+1 与 i64::MAX → validate 上界分支普通可读错误，
    /// 含字段名、原始值与 `U=<数值>` 上界文本；断言在 load 层完成（不构造限流器）。
    #[test]
    fn over_technical_cap_fails_with_bound() {
        with_env_cleaned(|| {
            let u = MAX_CONCURRENT_REQUESTS_CAP;
            let bound_tag = format!("U={u}");
            let u_plus_1 = (u + 1).to_string();
            let i64_max = i64::MAX.to_string();
            for v in [u_plus_1.as_str(), i64_max.as_str()] {
                // 文件路径
                let path = write_toml(&format!(
                    "[server]\naddr = \"0.0.0.0:8200\"\nmax_concurrent_requests = {v}\n"
                ));
                let err = Config::load(Some(path.to_str().unwrap()))
                    .unwrap_err()
                    .to_string();
                assert!(
                    err.contains("server.max_concurrent_requests"),
                    "实际：{err}"
                );
                assert!(err.contains(v), "错误含原始值（实际：{err}）");
                assert!(err.contains(&bound_tag), "错误含上界值（实际：{err}）");

                // env 路径（合法整数、域内解析成功后由 validate 拒绝）
                std::env::set_var(ENV_MAX, v);
                let err = Config::load(None).unwrap_err().to_string();
                assert!(
                    err.contains("server.max_concurrent_requests"),
                    "实际：{err}"
                );
                assert!(err.contains(v), "实际：{err}");
                assert!(err.contains(&bound_tag), "实际：{err}");
                std::env::remove_var(ENV_MAX);
            }
        });
    }

    /// TC-10（FR-6）：超 i64 解析域整数 → env parse 失败含 env 全名与原值（不进转换链）；
    /// 文件同值 → toml 反序列化错误含键名与原值
    #[test]
    fn parse_domain_overflow_keeps_raw_value() {
        with_env_cleaned(|| {
            let v = "99999999999999999999999"; // 23 位数字，超 i64 解析域
            std::env::set_var(ENV_MAX, v);
            let err = Config::load(None).unwrap_err().to_string();
            assert!(err.contains(ENV_MAX), "含 env 全名（实际：{err}）");
            assert!(err.contains(v), "含原值（实际：{err}）");
            std::env::remove_var(ENV_MAX);

            let path = write_toml(&format!(
                "[server]\naddr = \"0.0.0.0:8200\"\nmax_concurrent_requests = {v}\n"
            ));
            let err = Config::load(Some(path.to_str().unwrap()))
                .unwrap_err()
                .to_string();
            assert!(
                err.contains("max_concurrent_requests"),
                "含键名（实际：{err}）"
            );
            assert!(err.contains(v), "含原值（实际：{err}）");
        });
    }

    /// TC-11/12（Q6）：合法域上界 U 本身（文件/env）加载成功，容量一致（usize 值相等），
    /// 且可安全构造容量一致的限流器（Semaphore 名额按需分配，构造安全）
    #[test]
    fn valid_capacity_boundary_u() {
        with_env_cleaned(|| {
            let v = MAX_CONCURRENT_REQUESTS_CAP.to_string();

            // 文件路径
            let path = write_toml(&format!(
                "[server]\naddr = \"0.0.0.0:8200\"\nmax_concurrent_requests = {v}\n"
            ));
            let cfg = Config::load(Some(path.to_str().unwrap())).unwrap();
            assert_eq!(
                cfg.server.max_concurrent_requests as u128,
                MAX_CONCURRENT_REQUESTS_CAP
            );
            let cap = cfg.max_concurrent_requests();
            assert_eq!(cap as u128, MAX_CONCURRENT_REQUESTS_CAP);

            // env 路径
            std::env::set_var(ENV_MAX, &v);
            let cfg_env = Config::load(None).unwrap();
            assert_eq!(cfg_env.max_concurrent_requests(), cap);
            std::env::remove_var(ENV_MAX);

            // 可安全构造容量一致的限流器与信号量（无 panic；tokio 名额按需分配，§8 风险表）
            let limit = crate::limit::ConcurrencyLimit::new(cap);
            assert_eq!(limit.max(), cap);
            let semaphore = tokio::sync::Semaphore::new(cap);
            assert_eq!(semaphore.available_permits(), cap, "容量与配置一致");
        });
    }
}
