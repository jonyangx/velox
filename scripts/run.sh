#!/usr/bin/env bash
# 运行管理脚本：按依赖顺序启停 velox 各服务（开发环境，前台命令后台化）。
#
# 用法：
#   scripts/run.sh start  [crates...] [--release] [--rebuild]   # 按依赖顺序启动
#   scripts/run.sh stop   [crates...]                           # 按反向依赖顺序停止
#   scripts/run.sh restart [crates...] [--release] [--rebuild]  # 先 stop 再 start
#   scripts/run.sh status                                       # 查看各服务状态
#   scripts/run.sh logs [crate] [--follow] [-n 行数]            # 查看服务日志
#
# 说明：
#   - crate 可省略 velox- 前缀（registry / config / mesh）；
#   - 启动顺序：registry(:8300) → config(:8200) → mesh(:8100)，停止时反向；
#   - 可执行文件缺失时自动先跑一次 scripts/build.sh；--rebuild 强制重新编译；
#   - 每个服务加载各自 config/velox-<name>.toml（无则用内置默认值）；
#   - 日志写入 logs/<crate>.log，pid 写入 .run/<crate>.pid；
#   - 日志级别用 RUST_LOG 控制（如 RUST_LOG=debug scripts/run.sh start）。

source "$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)/common.sh"
require_toolchain

usage() {
  sed -n '2,16p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
  exit 0
}

start_one() {  # $1 = crate, $2 = profile
  local crate="$1" profile="$2"
  local bin="$VELOX_ROOT/target/$profile/$crate"

  if is_running "$crate"; then
    warn "$crate 已在运行（pid $(cat "$VELOX_RUN_DIR/$crate.pid")），跳过"
    return 0
  fi
  [[ -x "$bin" ]] || die "$crate 可执行文件不存在（$bin），请先运行 scripts/build.sh"

  mkdir -p "$VELOX_RUN_DIR" "$VELOX_LOG_DIR"
  local cmd=("$bin")
  local cfg="$VELOX_ROOT/$crate/config/$crate.toml"
  [[ -f "$cfg" ]] && cmd+=("$cfg")   # 服务以第一个位置参数接收配置路径

  info "启动 $crate（:${VELOX_PORTS[$crate]}，$profile）..."
  nohup "${cmd[@]}" >>"$VELOX_LOG_DIR/$crate.log" 2>&1 &
  echo $! >"$VELOX_RUN_DIR/$crate.pid"

  if is_running "$crate" && wait_healthz "$crate"; then
    ok "$crate 就绪（pid $(cat "$VELOX_RUN_DIR/$crate.pid")，http://127.0.0.1:${VELOX_PORTS[$crate]}/healthz）"
  else
    warn "$crate 未通过健康检查，最近日志："
    tail -n 20 "$VELOX_LOG_DIR/$crate.log" || true
    return 1
  fi
}

stop_one() {  # $1 = crate
  local crate="$1"
  if ! is_running "$crate"; then
    rm -f "$VELOX_RUN_DIR/$crate.pid"
    info "$crate 未在运行"
    return 0
  fi
  local pid; pid="$(cat "$VELOX_RUN_DIR/$crate.pid")"
  info "停止 $crate（pid $pid）..."
  kill "$pid" 2>/dev/null || true
  local i; for i in $(seq 1 50); do
    kill -0 "$pid" 2>/dev/null || break
    sleep 0.1
  done
  if kill -0 "$pid" 2>/dev/null; then
    warn "$crate 未在 5s 内退出，强制终止"
    kill -9 "$pid" 2>/dev/null || true
  fi
  rm -f "$VELOX_RUN_DIR/$crate.pid"
  ok "$crate 已停止"
}

do_start() {
  local profile=debug rebuild=false crate_args=()
  while (( $# > 0 )); do
    case "$1" in
      --release) profile=release ;;
      --rebuild) rebuild=true ;;
      -h|--help) usage ;;
      *)         crate_args+=("$1") ;;
    esac
    shift
  done
  resolve_crates "${crate_args[@]}"

  # 依赖顺序过滤：所选 crate 按 VELOX_START_ORDER 排序
  local ordered=() c
  for c in "${VELOX_START_ORDER[@]}"; do
    local want=""
    local s; for s in "${__crates[@]}"; do [[ "$s" == "$c" ]] && want=1; done
    [[ -n "$want" ]] && ordered+=("$c")
  done

  # 可执行文件缺失或 --rebuild 时先编译
  local missing=()
  for c in "${ordered[@]}"; do
    [[ -x "$VELOX_ROOT/target/$profile/$c" ]] || missing+=("$c")
  done
  if (( ${#missing[@]} > 0 )); then
    info "可执行文件缺失（${missing[*]}），先执行编译..."
    "$VELOX_ROOT/scripts/build.sh" $([[ "$profile" == release ]] && echo --release) "${missing[@]#velox-}"
  elif [[ "$rebuild" == true ]]; then
    "$VELOX_ROOT/scripts/build.sh" $([[ "$profile" == release ]] && echo --release) "${ordered[@]#velox-}"
  fi

  local failed=()
  for c in "${ordered[@]}"; do
    start_one "$c" "$profile" || failed+=("$c")
  done
  if (( ${#failed[@]} > 0 )); then
    die "启动失败：${failed[*]}（详见 logs/<crate>.log）"
  fi
  ok "全部启动完成：${ordered[*]}"
}

do_stop() {
  local crate_args=()
  while (( $# > 0 )); do
    case "$1" in
      -h|--help) usage ;;
      *)         crate_args+=("$1") ;;
    esac
    shift
  done
  resolve_crates "${crate_args[@]}"

  local ordered=() c
  for c in "${VELOX_STOP_ORDER[@]}"; do
    local want=""
    local s; for s in "${__crates[@]}"; do [[ "$s" == "$c" ]] && want=1; done
    [[ -n "$want" ]] && ordered+=("$c")
  done
  for c in "${ordered[@]}"; do stop_one "$c"; done
  ok "停止完成：${ordered[*]}"
}

do_status() {
  local c
  printf '%-16s %-8s %-10s %s\n' "服务" "状态" "pid" "端口"
  for c in "${VELOX_CRATES[@]}"; do
    if is_running "$c"; then
      printf '%-16s %-8s %-10s %s\n' "$c" "运行中" "$(cat "$VELOX_RUN_DIR/$c.pid")" ":${VELOX_PORTS[$c]}"
    else
      printf '%-16s %-8s %-10s %s\n' "$c" "未运行" "-" ":${VELOX_PORTS[$c]}"
    fi
  done
}

do_logs() {
  local follow=false lines=50 crate_args=()
  while (( $# > 0 )); do
    case "$1" in
      -f|--follow) follow=true ;;
      -n)          shift; lines="${1:?-n 需要行数参数}" ;;
      -h|--help)   usage ;;
      *)           crate_args+=("$1") ;;
    esac
    shift
  done
  resolve_crates "${crate_args[@]}"

  (( ${#__crates[@]} == 1 )) || die "logs 一次只能查看一个服务（可选：${VELOX_CRATES[*]}）"
  local crate="${__crates[0]}"
  local log="$VELOX_LOG_DIR/$crate.log"
  [[ -f "$log" ]] || die "暂无日志文件：$log（服务可能从未启动过）"
  if [[ "$follow" == true ]]; then
    exec tail -n "$lines" -f "$log"
  else
    tail -n "$lines" "$log"
  fi
}

cmd="${1:-}"
shift || true
case "$cmd" in
  start)   do_start "$@" ;;
  stop)    do_stop "$@" ;;
  restart)
    # restart 复用 start 的选项（--release/--rebuild），但 stop 只接受 crate 参数
    ra=()
    for arg in "$@"; do
      case "$arg" in --release|--rebuild) ;; *) ra+=("$arg") ;; esac
    done
    do_stop "${ra[@]}"
    do_start "$@"
    ;;
  status)  (( $# == 0 )) || die "status 不接受参数"; do_status ;;
  logs)    do_logs "$@" ;;
  -h|--help|"") usage ;;
  *)       die "未知命令 '$cmd'（可用：start / stop / restart / status / logs）" ;;
esac
