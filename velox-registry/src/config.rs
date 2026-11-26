//! 配置加载：TOML 文件（可选）+ 环境变量覆盖。
//!
//! 环境变量分层约定（见开发规范第 5 节）：`VELOX_REGISTRY__<SECTION>__<FIELD>`。
//!
//! v0.2 设计变更（GATE2 仲裁定案）：env 空字符串不再被忽略（v0.1 `env_override`
//! 的 `filter(!is_empty)` 已移除）——显式设置空值会进入解析期拒绝或 validate 拒绝
//! 路径（无论持久化是否启用），fail fast 优先于静默回落默认值。

use serde::Deserialize;

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Config {
    pub server: ServerConfig,
    pub registry: RegistryConfig,
    pub persistence: PersistenceConfig,
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

/// 持久化配置节（FR-5，架构 §4.5/G2-3）。
///
/// **容器级 `#[serde(default)]` + 手写 `Default`**：`bool` 的逐字段 serde 默认只会给
/// `false`、`u64` 给 `0`，均不合语义；容器级缺省从 `Default::default()` 补齐，
/// 使 `[persistence]` 整节缺失、或节内只写部分字段（如仅 `enabled = false`、
/// 仅 `data_dir`）都能取其余默认值 true / ./data / 5（TC-12）。
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct PersistenceConfig {
    /// 是否启用落盘与恢复（Q1 定案：默认开启；关闭为排障逃生门）
    pub enabled: bool,
    /// 落盘根目录（Q6/G2-5 定案：默认 `./data` 不变；实际快照在其 registry/ 专属子目录）
    pub data_dir: String,
    /// 心跳脏快照刷盘周期（FR-2）
    pub flush_interval_secs: u64,
}

impl Default for PersistenceConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            data_dir: "./data".to_string(),
            flush_interval_secs: 5,
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
        // 持久化节 env 覆盖（env 优先于 TOML，既有机制）。
        // ENABLED 仅接受 true/false（错误含实际值）；空值不再被忽略（设计变更见模块注释），
        // 走解析期拒绝（无论 enabled，G2-3）。
        if let Some(v) = env_override("VELOX_REGISTRY__PERSISTENCE__ENABLED") {
            cfg.persistence.enabled = parse_bool(
                "VELOX_REGISTRY__PERSISTENCE__ENABLED",
                "persistence.enabled",
                &v,
            )?;
        }
        if let Some(v) = env_override("VELOX_REGISTRY__PERSISTENCE__DATA_DIR") {
            cfg.persistence.data_dir = v;
        }
        if let Some(v) = env_override("VELOX_REGISTRY__PERSISTENCE__FLUSH_INTERVAL_SECS") {
            cfg.persistence.flush_interval_secs = v.parse().map_err(|_| {
                anyhow::anyhow!(
                    "非法 VELOX_REGISTRY__PERSISTENCE__FLUSH_INTERVAL_SECS：'{v}'（要求非负整数）"
                )
            })?;
        }
        cfg.validate()?;
        Ok(cfg)
    }

    /// 集中校验（参照 velox-config，load 末尾调用；AC-5/G2-3）。
    /// 错误均含配置项名与实际值。
    ///
    /// 规则边界（G2-3 定案）：
    /// 1. `flush_interval_secs == 0` → 拒绝（**无论 enabled**：0 对周期字段本身非法）；
    /// 2. `data_dir` 为空 → 拒绝（**无论 enabled**：同理）；
    /// 3. `enabled = true` 且 `flush_interval_secs >= heartbeat_ttl_secs` → 拒绝
    ///    （**仅启用态**；语义为缩小正常刷盘路径的心跳新鲜度丢失窗口，
    ///    不构成「健康实例恢复后必存活」的硬保证——恢复以 FR-2 窗口为准，G2-4）；
    /// 4. `enabled = false`：除规则 1/2 外无其他拒绝项——
    ///    `enabled=false + TTL1 + 默认 flush5` 合法可运行，行为与 v0.1 一致（FR-5/AC-5）。
    fn validate(&self) -> anyhow::Result<()> {
        if self.persistence.flush_interval_secs == 0 {
            anyhow::bail!("非法配置 persistence.flush_interval_secs：0（要求 ≥1 的整数）");
        }
        if self.persistence.data_dir.is_empty() {
            anyhow::bail!("非法配置 persistence.data_dir：''（要求非空目录路径）");
        }
        if self.persistence.enabled
            && self.persistence.flush_interval_secs >= self.registry.heartbeat_ttl_secs
        {
            anyhow::bail!(
                "非法配置 persistence.flush_interval_secs：{}（启用持久化时要求 < registry.heartbeat_ttl_secs={}，否则心跳恢复窗口过大）",
                self.persistence.flush_interval_secs,
                self.registry.heartbeat_ttl_secs
            );
        }
        Ok(())
    }
}

/// env 布尔解析（仅接受 `true`/`false`，错误含变量名与实际值）。
fn parse_bool(env_key: &str, field: &str, v: &str) -> anyhow::Result<bool> {
    match v {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => Err(anyhow::anyhow!(
            "非法配置 {env_key}（{field}）：'{v}'（仅接受 true/false）"
        )),
    }
}

/// env 读取（v0.2 设计变更/GATE2 仲裁）：**空字符串不再被 `filter(!is_empty)` 忽略**，
/// 显式空值进入解析期拒绝或 validate 拒绝路径（无论持久化是否启用）。
fn env_override(key: &str) -> Option<String> {
    std::env::var(key).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    /// 环境变量为进程级全局：所有触碰 env 的测试共享一把锁，避免并行互扰
    /// （同 velox-config/src/config.rs 的 ENV_LOCK 模式）
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    const ENV_ENABLED: &str = "VELOX_REGISTRY__PERSISTENCE__ENABLED";
    const ENV_DATA_DIR: &str = "VELOX_REGISTRY__PERSISTENCE__DATA_DIR";
    const ENV_FLUSH: &str = "VELOX_REGISTRY__PERSISTENCE__FLUSH_INTERVAL_SECS";
    const ENV_TTL: &str = "VELOX_REGISTRY__REGISTRY__HEARTBEAT_TTL_SECS";

    /// 统一入口：持锁并清理本测试涉及的环境变量，保证用例起点确定
    fn with_env_cleaned<F: FnOnce()>(f: F) {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        for key in [ENV_ENABLED, ENV_DATA_DIR, ENV_FLUSH, ENV_TTL] {
            std::env::remove_var(key);
        }
        f();
        for key in [ENV_ENABLED, ENV_DATA_DIR, ENV_FLUSH, ENV_TTL] {
            std::env::remove_var(key);
        }
    }

    fn write_toml(content: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!("velox-registry-cfg-{}.toml", Uuid::new_v4()));
        std::fs::write(&path, content).unwrap();
        path
    }

    /// 默认全值（AC-5）：无配置文件 → enabled=true、./data、flush=5（v0.1 字段沿 v0.1 默认）
    #[test]
    fn persistence_defaults() {
        // 整体持锁（BUG-2）：Config::load 始终读进程 env，
        // 未持锁读取会与改 env 的并行用例跨污染（见 04-test-report.md §5 BUG-2）
        with_env_cleaned(|| {
            let cfg = Config::load(None).unwrap();
            assert!(cfg.persistence.enabled);
            assert_eq!(cfg.persistence.data_dir, "./data");
            assert_eq!(cfg.persistence.flush_interval_secs, 5);
            assert_eq!(cfg.registry.heartbeat_ttl_secs, 30);
        });
    }

    /// 部分节（G2-3/TC-12）：仅 `enabled = false` 或仅 `data_dir` 时，
    /// 其余字段取容器 Default（true/./data/5），不用 serde 逐字段的 false/0 误代。
    #[test]
    fn partial_section_takes_container_defaults() {
        // 整体持锁（BUG-2）：Config::load 始终读进程 env，
        // 未持锁读取会与改 env 的并行用例跨污染（见 04-test-report.md §5 BUG-2）
        with_env_cleaned(|| {
            let path = write_toml("[server]\naddr = \"0.0.0.0:8300\"\n\n[registry]\nheartbeat_ttl_secs = 30\nsweep_interval_secs = 5\n\n[persistence]\nenabled = false\n");
            let cfg = Config::load(Some(path.to_str().unwrap())).unwrap();
            assert!(!cfg.persistence.enabled);
            assert_eq!(
                cfg.persistence.data_dir, "./data",
                "缺省字段取容器默认 ./data"
            );
            assert_eq!(
                cfg.persistence.flush_interval_secs, 5,
                "缺省字段取容器默认 5"
            );

            let path = write_toml("[persistence]\ndata_dir = \"/tmp/only-dir\"\n");
            let cfg = Config::load(Some(path.to_str().unwrap())).unwrap();
            assert!(cfg.persistence.enabled, "缺省 enabled 取容器默认 true");
            assert_eq!(cfg.persistence.data_dir, "/tmp/only-dir");
            assert_eq!(cfg.persistence.flush_interval_secs, 5);
        });
    }

    /// 规则 1：flush=0 拒绝，**enabled=false 同样拒绝**（G2-3：字段本身非法与启用无关）
    #[test]
    fn flush_zero_rejected_even_when_disabled() {
        // 整体持锁（BUG-2）：Config::load 始终读进程 env，
        // 未持锁读取会与改 env 的并行用例跨污染（见 04-test-report.md §5 BUG-2）
        with_env_cleaned(|| {
            for enabled in [true, false] {
                let path = write_toml(&format!(
                    "[persistence]\nenabled = {enabled}\nflush_interval_secs = 0\n"
                ));
                let err = Config::load(Some(path.to_str().unwrap()))
                    .unwrap_err()
                    .to_string();
                assert!(
                    err.contains("persistence.flush_interval_secs"),
                    "实际：{err}"
                );
                assert!(err.contains('0'), "实际：{err}");
            }
        });
    }

    /// 规则 2：data_dir 为空拒绝（无论 enabled；TOML 与 env 两途径）
    #[test]
    fn empty_data_dir_rejected_even_when_disabled() {
        // 整体持锁（BUG-2）：load 读进程 env，须与改 env 的用例串行
        with_env_cleaned(|| {
            let path = write_toml("[persistence]\nenabled = false\ndata_dir = \"\"\n");
            let err = Config::load(Some(path.to_str().unwrap()))
                .unwrap_err()
                .to_string();
            assert!(err.contains("persistence.data_dir"), "实际：{err}");

            std::env::set_var(ENV_ENABLED, "false");
            std::env::set_var(ENV_DATA_DIR, "");
            let err = Config::load(None).unwrap_err().to_string();
            assert!(err.contains("persistence.data_dir"), "实际：{err}");
        });
    }

    /// 规则 3：跨项 `flush >= ttl` 仅 enabled=true 拒绝（G2-3/G2-4）。
    /// 默认 5 < 30 合法；30 >= 30 拒绝；enabled=false + TTL1 + flush5 合法（规则 4）。
    #[test]
    fn cross_field_flush_ge_ttl_only_when_enabled() {
        // 整体持锁（BUG-2）：Config::load 始终读进程 env，
        // 未持锁读取会与改 env 的并行用例跨污染（见 04-test-report.md §5 BUG-2）
        with_env_cleaned(|| {
            // enabled=true + flush=30 >= ttl=30 → 拒绝（含两个配置项名与实际值）
            let path = write_toml("[registry]\nheartbeat_ttl_secs = 30\nsweep_interval_secs = 5\n\n[persistence]\nenabled = true\nflush_interval_secs = 30\n");
            let err = Config::load(Some(path.to_str().unwrap()))
                .unwrap_err()
                .to_string();
            assert!(
                err.contains("persistence.flush_interval_secs"),
                "实际：{err}"
            );
            assert!(err.contains("heartbeat_ttl_secs"), "实际：{err}");

            // enabled=true + flush=5 < ttl=30（默认）→ 合法
            let path = write_toml("[persistence]\nenabled = true\n");
            assert!(Config::load(Some(path.to_str().unwrap())).is_ok());

            // enabled=false + TTL1 + 默认 flush5 → 合法（禁用态不执行规则 3）
            let path = write_toml("[registry]\nheartbeat_ttl_secs = 1\nsweep_interval_secs = 1\n\n[persistence]\nenabled = false\n");
            let cfg = Config::load(Some(path.to_str().unwrap())).unwrap();
            assert!(!cfg.persistence.enabled);
            assert_eq!(cfg.persistence.flush_interval_secs, 5);
        });
    }

    /// 解析期拒绝（无论 enabled）：TOML 类型错误
    #[test]
    fn toml_type_errors_rejected() {
        // 整体持锁（BUG-2）：Config::load 始终读进程 env，
        // 未持锁读取会与改 env 的并行用例跨污染（见 04-test-report.md §5 BUG-2）
        with_env_cleaned(|| {
            for enabled in [true, false] {
                let path = write_toml(&format!(
                    "[persistence]\nenabled = {enabled}\nflush_interval_secs = -1\n"
                ));
                let err = Config::load(Some(path.to_str().unwrap()))
                    .unwrap_err()
                    .to_string();
                assert!(
                    err.contains("解析配置文件"),
                    "TOML 类型错须解析期拒绝，实际：{err}"
                );
            }
        });
    }

    /// env 覆盖合法值生效（优先于 TOML）+ ENABLED 仅接受 true/false
    #[test]
    fn env_persistence_overrides() {
        with_env_cleaned(|| {
            std::env::set_var(ENV_ENABLED, "false");
            std::env::set_var(ENV_DATA_DIR, "/tmp/env-dir");
            std::env::set_var(ENV_FLUSH, "3");
            let cfg = Config::load(None).unwrap();
            assert!(!cfg.persistence.enabled);
            assert_eq!(cfg.persistence.data_dir, "/tmp/env-dir");
            assert_eq!(cfg.persistence.flush_interval_secs, 3);

            // 非法 ENABLED → 拒绝，含实际值（无论值大小写）
            for bad in ["True", "1", "yes", ""] {
                std::env::set_var(ENV_ENABLED, bad);
                let err = Config::load(None).unwrap_err().to_string();
                assert!(err.contains("PERSISTENCE__ENABLED"), "实际：{err}");
            }
            std::env::set_var(ENV_ENABLED, "true");

            // 非法 FLUSH（负数/非数字/空值）→ 解析期拒绝，含实际值
            for bad in ["-1", "abc", "1.5", ""] {
                std::env::set_var(ENV_FLUSH, bad);
                let err = Config::load(None).unwrap_err().to_string();
                assert!(
                    err.contains("PERSISTENCE__FLUSH_INTERVAL_SECS"),
                    "实际：{err}"
                );
                assert!(err.contains(bad), "错误须含实际值 '{bad}'，实际：{err}");
            }
        });
    }

    /// **env 空值设计变更（GATE2 仲裁定案）**：v0.1 的 `filter(!is_empty)` 空值忽略语义
    /// 已移除——空字符串不再回落默认值，而是进入解析期拒绝（数值/布尔）或
    /// validate 拒绝（data_dir），且无论持久化是否启用。
    #[test]
    fn env_empty_value_is_rejected_not_ignored() {
        with_env_cleaned(|| {
            // 既有字段的空值同样不再忽略（行为变更覆盖全部 env 覆盖项）
            std::env::set_var(ENV_TTL, "");
            let err = Config::load(None).unwrap_err().to_string();
            assert!(err.contains("HEARTBEAT_TTL_SECS"), "实际：{err}");
            std::env::remove_var(ENV_TTL);

            // 禁用态下空值仍拒绝（G2-3：解析期拒绝无论 enabled）
            std::env::set_var(ENV_ENABLED, "false");
            std::env::set_var(ENV_FLUSH, "");
            let err = Config::load(None).unwrap_err().to_string();
            assert!(
                err.contains("PERSISTENCE__FLUSH_INTERVAL_SECS"),
                "实际：{err}"
            );
        });
    }

    /// 未提供配置文件时除新增默认外行为与 v0.1 一致（FR-11）：v0.1 字段默认值不变
    #[test]
    fn v01_fields_unchanged_by_default() {
        // 整体持锁（BUG-2）：Config::load 始终读进程 env，
        // 未持锁读取会与改 env 的并行用例跨污染（见 04-test-report.md §5 BUG-2）
        with_env_cleaned(|| {
            let cfg = Config::load(None).unwrap();
            assert_eq!(cfg.server.addr, "0.0.0.0:8300");
            assert_eq!(cfg.registry.heartbeat_ttl_secs, 30);
            assert_eq!(cfg.registry.sweep_interval_secs, 5);
        });
    }
}
