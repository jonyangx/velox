#!/usr/bin/env bash
# velox 工程脚本公共库 —— 供 scripts/ 下各脚本 source，不直接执行。
#
# 约定（见 velox-mono/docs/development.md）：
#   - git 仓库根 = Cargo workspace 根 = 本目录的上一级；
#   - 启动依赖顺序：registry → config → mesh（mesh 需要前两者做服务发现/取配置）；
#   - 健康检查统一 GET /healthz。

set -euo pipefail

# workspace 根目录：本文件位于 <root>/scripts/ 下
VELOX_ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$VELOX_ROOT"

# 子系统清单（与根 Cargo.toml members 一致）与启停顺序
VELOX_CRATES=(velox-registry velox-config velox-mesh)
VELOX_START_ORDER=(velox-registry velox-config velox-mesh)
VELOX_STOP_ORDER=(velox-mesh velox-config velox-registry)
declare -A VELOX_PORTS=([velox-registry]=8300 [velox-config]=8200 [velox-mesh]=8100)

# 运行时目录
VELOX_RUN_DIR="$VELOX_ROOT/.run"   # pid 文件
VELOX_LOG_DIR="$VELOX_ROOT/logs"   # 服务日志（logs/<crate>.log）

# ---- 着色输出 ----
if [[ -t 1 ]]; then
  C_INFO=$'\033[1;34m'; C_OK=$'\033[1;32m'; C_WARN=$'\033[1;33m'; C_ERR=$'\033[1;31m'; C_OFF=$'\033[0m'
else
  C_INFO=''; C_OK=''; C_WARN=''; C_ERR=''; C_OFF=''
fi
info() { printf '%s[velox]%s %s\n' "$C_INFO" "$C_OFF" "$*"; }
ok()   { printf '%s[ ok ]%s %s\n' "$C_OK"   "$C_OFF" "$*"; }
warn() { printf '%s[warn]%s %s\n' "$C_WARN" "$C_OFF" "$*"; }
die()  { printf '%s[fail]%s %s\n' "$C_ERR"  "$C_OFF" "$*" >&2; exit 1; }

# ---- 工具链检查 ----
require_toolchain() {
  command -v cargo >/dev/null 2>&1 || die "未找到 cargo，请先安装 rustup（见 README 快速开始）"
}

# ---- crate 参数解析 ----
# 将命令行参数解析为 crate 列表（写入全局数组 __crates）。
# 支持简称（registry）与全名（velox-registry）；无参数 = 全部子系统。
# 用法：resolve_crates "${非选项参数[@]}"
resolve_crates() {
  __crates=()
  for arg in "$@"; do
    local matched=""
    for c in "${VELOX_CRATES[@]}"; do
      if [[ "$arg" == "$c" || "$arg" == "${c#velox-}" ]]; then matched="$c"; break; fi
    done
    [[ -n "$matched" ]] || die "未知子系统 '$arg'（可选：${VELOX_CRATES[*]}，可省略 velox- 前缀）"
    local dup=""
    if (( ${#__crates[@]} > 0 )); then
      local c; for c in "${__crates[@]}"; do [[ "$c" == "$matched" ]] && dup=1; done
    fi
    [[ -n "$dup" ]] || __crates+=("$matched")
  done
  (( ${#__crates[@]} > 0 )) || __crates=("${VELOX_CRATES[@]}")
}

# 将 __crates 展开为 cargo 的 -p 参数列表（写入全局数组 __p_args）
to_cargo_package_args() {
  __p_args=()
  local c; for c in "${__crates[@]}"; do __p_args+=(-p "$c"); done
}

# ---- 进程管理辅助（run.sh 使用） ----
is_running() {  # $1 = crate；退出码 0 = 运行中
  local pidfile="$VELOX_RUN_DIR/$1.pid"
  [[ -f "$pidfile" ]] || return 1
  local pid; pid="$(cat "$pidfile" 2>/dev/null)" || return 1
  [[ -n "$pid" ]] && kill -0 "$pid" 2>/dev/null
}

wait_healthz() {  # $1 = crate；等待 /healthz 就绪（最多 10s），无 curl 时跳过
  local crate="$1" port="${VELOX_PORTS[$1]}"
  command -v curl >/dev/null 2>&1 || return 0
  local i; for i in $(seq 1 50); do
    curl -fsS -m 1 "http://127.0.0.1:$port/healthz" >/dev/null 2>&1 && return 0
    sleep 0.2
  done
  return 1
}
