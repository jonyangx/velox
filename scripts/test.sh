#!/usr/bin/env bash
# 测试脚本：按开发规范第 1/6 节执行检查（CI 门槛 = fmt + clippy + build + test 全绿）。
#
# 用法：
#   scripts/test.sh            # 完整 CI 门槛：fmt + clippy + build + test
#   scripts/test.sh --lint     # 只跑静态检查：fmt + clippy
#   scripts/test.sh --fast     # 快速反馈：fmt + test（跳过 clippy 与独立 build）
#   scripts/test.sh --unit     # 只跑 cargo test
#
# 任何一步失败立即终止并以非零码退出。

source "$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)/common.sh"
require_toolchain

usage() {
  sed -n '2,10p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
  exit 0
}

mode=full
case "${1:-}" in
  "")        mode=full ;;
  --lint)    mode=lint ;;
  --fast)    mode=fast ;;
  --unit)    mode=unit ;;
  -h|--help) usage ;;
  *)         die "未知参数 '${1:-}'（可用：--lint / --fast / --unit）" ;;
esac
(( $# <= 1 )) || die "只支持一个模式参数"

run_stage() {  # $1 = 阶段名，其余 = 命令
  local stage="$1"; shift
  info "── $stage：$* ..."
  if "$@"; then
    ok "$stage 通过"
  else
    die "$stage 失败，请修复后重试"
  fi
}

if [[ "$mode" == full || "$mode" == lint || "$mode" == fast ]]; then
  run_stage "格式检查" cargo fmt --all -- --check
fi
if [[ "$mode" == full || "$mode" == lint ]]; then
  run_stage "静态检查" cargo clippy --workspace --all-targets -- -D warnings
fi
if [[ "$mode" == full ]]; then
  run_stage "编译" cargo build --workspace
fi
if [[ "$mode" == full || "$mode" == fast || "$mode" == unit ]]; then
  run_stage "测试" cargo test --workspace
fi

ok "全部检查通过（模式：$mode）"
