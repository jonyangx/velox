//! 等级来源选择与语法校验（架构 §4.4）。
//!
//! 纪律：**先选最高优先来源，只校验生效字符串**——合法高层可覆盖较低非法等级；
//! `RUST_LOG`（非空时）保留 tracing `EnvFilter` 完整兼容语法（裸词可为合法 target）；
//! 新渠道（`VELOX_*__LOG__LEVEL` / 文件 `[log].level`）为严格白名单语法
//! （裸六等级或显式 `target=level`，不接受裸 target/空值/未知等级）。
//! 环境读取经 [`EnvInputs`]（var_os 语义：区分 unset/已设置为空/非 UTF-8），
//! 生产读真实环境，测试注入 map，不在并行单测里修改进程环境。

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use tracing_subscriber::EnvFilter;

use crate::{LevelSource, LogConfig, ResolvedLevel};

/// 新渠道允许的裸等级（小写，严格匹配）
const BARE_LEVELS: [&str; 6] = ["trace", "debug", "info", "warn", "error", "off"];

/// 环境变量读取抽象：生产 = 真实进程环境；测试 = 注入 map（含非 UTF-8 值）。
#[derive(Debug, Clone, Default)]
pub(crate) struct EnvInputs {
    vars: BTreeMap<String, OsString>,
}

impl EnvInputs {
    /// 真实进程环境（var_os：保留非 UTF-8 值以便选中时报错并标变量名）
    pub fn from_std_env() -> Self {
        Self {
            vars: std::env::vars_os()
                .filter_map(|(k, v)| k.into_string().ok().map(|k| (k, v)))
                .collect(),
        }
    }

    /// 测试注入（纯 UTF-8 map）
    #[cfg(test)]
    pub fn from_map(map: BTreeMap<String, String>) -> Self {
        Self {
            vars: map
                .into_iter()
                .map(|(k, v)| (k, OsString::from(v)))
                .collect(),
        }
    }

    /// 测试注入（原始 OsString，可携带非 UTF-8 值）
    #[cfg(test)]
    pub fn from_raw_map(map: BTreeMap<String, OsString>) -> Self {
        Self { vars: map }
    }

    fn get(&self, key: &str) -> Option<&OsString> {
        self.vars.get(key)
    }
}

/// 解析为 UTF-8；失败给出变量名（§4.4.1：选中变量非 UTF-8 也报错并标变量名）
fn as_utf8<'a>(name: &str, raw: &'a OsString) -> anyhow::Result<&'a str> {
    raw.to_str()
        .ok_or_else(|| anyhow::anyhow!("非法日志配置 {name}：值非 UTF-8"))
}

/// 新渠道严格白名单校验（§4.4.2）：
/// 允许小写裸等级 `trace|debug|info|warn|error|off`，或逗号分隔的
/// 裸等级 / 显式 `target=level`（target 非空、字符仅 ASCII 字母/数字/下划线/连字符/冒号、
/// 至少含一个字母或下划线；项两侧允许 ASCII 空白；空项/尾逗号拒绝）。
/// 不接受裸 target、空值、数字等级、未知等级、span/field 匹配语法。
pub(crate) fn validate_strict_level(value: &str) -> Result<(), String> {
    if value.is_empty() {
        return Err("empty level".to_string());
    }
    for item in value.split(',') {
        let item = item.trim_matches(|c: char| c.is_ascii_whitespace());
        if item.is_empty() {
            return Err(format!("空指令项（含尾逗号）：'{value}'"));
        }
        match item.split_once('=') {
            None => {
                if !BARE_LEVELS.contains(&item) {
                    return Err(format!(
                        "未知裸等级 '{item}'（新渠道不接受裸 target；完整语法请用 RUST_LOG）"
                    ));
                }
            }
            Some((target, level)) => {
                if target.is_empty() {
                    return Err("target 不能为空".to_string());
                }
                if !valid_target(target) {
                    return Err(format!(
                        "target '{target}' 非法（仅 ASCII 字母/数字/下划线/连字符/冒号，且至少含一个字母或下划线）"
                    ));
                }
                if !BARE_LEVELS.contains(&level) {
                    return Err(format!("指令 '{item}' 的等级 '{level}' 非法"));
                }
            }
        }
    }
    Ok(())
}

fn valid_target(target: &str) -> bool {
    target
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == ':')
        && target.chars().any(|c| c.is_ascii_alphabetic() || c == '_')
}

/// 来源选择（§4.4.1，先选最高优先、只校验生效字符串）：
/// 1. `RUST_LOG` 已设置且原始值 ≠ ""（空串视作未设置）；
/// 2. 否则 `VELOX_<prefix>__LOG__LEVEL` 只要已设置就选中（含空串）；
/// 3. 否则文件显式 `[log].level`（file_level_present=true，空串保留为 Some("")）；
/// 4. 否则 Default（warn）。
pub(crate) fn select_source(
    log: &LogConfig,
    path: Option<&Path>,
    prefix: &str,
    env: &EnvInputs,
) -> anyhow::Result<Selection> {
    if let Some(raw) = env.get("RUST_LOG") {
        let value = as_utf8("RUST_LOG", raw)?;
        if !value.is_empty() {
            return Ok(Selection {
                value: value.to_string(),
                source: LevelSource::Env("RUST_LOG".to_string()),
            });
        }
    }
    let var = format!("VELOX_{prefix}__LOG__LEVEL");
    if let Some(raw) = env.get(&var) {
        let value = as_utf8(&var, raw)?;
        return Ok(Selection {
            value: value.to_string(),
            source: LevelSource::Env(var),
        });
    }
    if log.file_level_present {
        let file = absolutize_config_path(path);
        return Ok(Selection {
            value: log.level.clone(),
            source: LevelSource::File(file),
        });
    }
    Ok(Selection {
        value: "warn".to_string(),
        source: LevelSource::Default,
    })
}

/// 相对配置路径以 cwd 拼接为绝对上下文（§4.4.3：错误写绝对路径，不笼统写"文件或 env"）
pub(crate) fn absolutize_config_path(path: Option<&Path>) -> PathBuf {
    match path {
        Some(p) if p.is_absolute() => p.to_path_buf(),
        Some(p) => match std::env::current_dir() {
            Ok(cwd) => cwd.join(p),
            Err(_) => p.to_path_buf(),
        },
        // file_level_present=true 必然来自真实配置文件；防御性兜底
        None => PathBuf::from("<unknown-config-path>"),
    }
}

/// 来源 + 值的组合（校验/解析前使用）
pub(crate) struct Selection {
    pub value: String,
    pub source: LevelSource,
}

/// 由 Selection 构造 [`ResolvedLevel`]：按来源选择语法并严格解析，失败 fail-fast
/// （错误含实际值与准确来源：env 变量名或绝对文件路径 + log.level 键）。
pub(crate) fn build_resolved(sel: Selection) -> anyhow::Result<ResolvedLevel> {
    match &sel.source {
        LevelSource::Default => Ok(ResolvedLevel::default()),
        LevelSource::Env(name) if name == "RUST_LOG" => {
            // 仅空白且非空串非法（§4.4.1）；其余保留 EnvFilter 完整兼容语法
            if sel.value.trim().is_empty() {
                anyhow::bail!(
                    "非法日志等级 value=\"{}\" source=env:RUST_LOG: 仅空白的 RUST_LOG 非法",
                    sel.value
                );
            }
            let filter = EnvFilter::try_new(&sel.value).map_err(|e| {
                anyhow::anyhow!(
                    "非法日志等级 value=\"{}\" source=env:RUST_LOG: {e}",
                    sel.value
                )
            })?;
            Ok(ResolvedLevel {
                value: sel.value,
                source: sel.source,
                filter,
            })
        }
        LevelSource::Env(name) => {
            let filter = parse_strict(&sel.value).map_err(|reason| {
                anyhow::anyhow!(
                    "非法日志等级 value=\"{}\" source=env:{name}: {reason}",
                    sel.value
                )
            })?;
            Ok(ResolvedLevel {
                value: sel.value,
                source: sel.source,
                filter,
            })
        }
        LevelSource::File(path) => {
            let filter = parse_strict(&sel.value).map_err(|reason| {
                anyhow::anyhow!(
                    "非法日志等级 value=\"{}\" source=file:{} key=log.level: {reason}",
                    sel.value,
                    path.display()
                )
            })?;
            Ok(ResolvedLevel {
                value: sel.value,
                source: sel.source,
                filter,
            })
        }
    }
}

/// 新渠道两步解析：白名单校验通过后再 `EnvFilter::try_new` 严格解析（§4.4.2）
fn parse_strict(value: &str) -> Result<EnvFilter, String> {
    validate_strict_level(value)?;
    EnvFilter::try_new(value).map_err(|e| e.to_string())
}

#[cfg(test)]
pub(crate) fn resolve_from(
    log: &LogConfig,
    path: Option<&Path>,
    prefix: &str,
    env: &EnvInputs,
) -> anyhow::Result<ResolvedLevel> {
    let sel = select_source(log, path, prefix, env)?;
    build_resolved(sel)
}
