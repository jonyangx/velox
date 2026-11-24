#!/usr/bin/env bash
# 打包脚本：release 编译 + 组装发布目录 + tar.gz 归档。
#
# 用法：
#   scripts/package.sh            # 构建并打包全部子系统
#   scripts/package.sh registry   # 只打包指定子系统（可省略 velox- 前缀）
#
# 产物布局（dist/<name>.tar.gz 解开后）：
#   velox-<版本>-<os>-<arch>/
#   ├── bin/velox-<name>           # release 可执行文件
#   ├── config/velox-<name>.toml   # 开发默认配置（生产配置不入库，见规范第 5 节）
#   ├── README.md
#   └── VERSION                    # 版本号（取自根 Cargo.toml [workspace.package]）

source "$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)/common.sh"
require_toolchain

usage() {
  sed -n '2,14p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
  exit 0
}

crate_args=()
while (( $# > 0 )); do
  case "$1" in
    -h|--help) usage ;;
    *)         crate_args+=("$1") ;;
  esac
  shift
done
resolve_crates "${crate_args[@]}"

# 版本号：取根 Cargo.toml [workspace.package] 段的 version
version="$(awk '/^\[workspace\.package\]/{s=1;next} /^\[/{s=0} s && $1=="version"{gsub(/[",]/,"",$3); print $3; exit}' Cargo.toml)"
[[ -n "$version" ]] || die "无法从 Cargo.toml 解析 [workspace.package].version"

os="$(uname -s | tr '[:upper:]' '[:lower:]')"   # linux / darwin
arch="$(uname -m)"                              # x86_64 / aarch64
name="velox-${version}-${os}-${arch}"
dist="$VELOX_ROOT/dist/$name"

info "版本 $version，目标 $os/$arch，打包 ${__crates[*]}..."

# 1. release 编译
to_cargo_package_args
cargo build --release "${__p_args[@]}"

# 2. 组装发布目录
rm -rf "$dist"
mkdir -p "$dist/bin" "$dist/config"
for c in "${__crates[@]}"; do
  bin="target/release/$c"
  [[ -x "$bin" ]] || die "缺少可执行文件 $bin"
  cp "$bin" "$dist/bin/"
  if [[ -f "$c/config/$c.toml" ]]; then
    cp "$c/config/$c.toml" "$dist/config/"
  else
    warn "$c 无配置文件，跳过（服务将使用内置默认值启动）"
  fi
done
echo "$version" > "$dist/VERSION"
cp README.md "$dist/" 2>/dev/null || true

# 3. 归档
( cd dist && tar -czf "$name.tar.gz" "$name" )

ok "打包完成 → dist/$name.tar.gz"
du -h "dist/$name.tar.gz"
