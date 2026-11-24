#!/usr/bin/env bash
# 构建编译脚本：cargo build（debug 或 release）。
#
# 用法：
#   scripts/build.sh                    # debug 构建全部子系统
#   scripts/build.sh --release          # release 构建全部子系统
#   scripts/build.sh registry mesh      # 只构建指定子系统（可省略 velox- 前缀）
#   scripts/build.sh --release config   # release 构建指定子系统
#
# 产物位置：target/{debug|release}/velox-<name>

source "$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)/common.sh"
require_toolchain

usage() {
  sed -n '2,10p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
  exit 0
}

release=false
crate_args=()
while (( $# > 0 )); do
  case "$1" in
    -r|--release) release=true ;;
    -h|--help)    usage ;;
    -*)           die "未知选项 '$1'（可用：--release）" ;;
    *)            crate_args+=("$1") ;;
  esac
  shift
done

resolve_crates "${crate_args[@]}"
to_cargo_package_args

profile=debug
flags=()
if [[ "$release" == true ]]; then
  profile=release
  flags+=(--release)
fi

info "编译 ${__crates[*]}（$profile）..."
cargo build "${flags[@]}" "${__p_args[@]}"
ok "编译完成 → target/$profile/"
