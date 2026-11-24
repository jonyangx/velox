#!/usr/bin/env bash
# velox-demo 端到端验收（host 模式，仓库根 = Cargo workspace 根）。
#
# 覆盖链路：健康检查 → 注册/发现 → 直连 provider → 配置中心热更（builtin→config_center）
#   → mesh 转发一致性 → 扩容第二实例验证轮询 LB → SIGKILL 宕机剔除 → SIGTERM 优雅注销。
#
# 前置依赖：cargo、curl、jq（jq 缺失请先安装，脚本会显式报错）。
# 端口占用（固定）：8100/8101/8200/8300/9000/9001/9100，启动前必须空闲。
# 运行方式：bash velox-demo/scripts/acceptance.sh
# 注意：为加速验收，registry 使用短 TTL（6s）/快 sweep（2s）——仅限验收环境，勿用于生产配置。

set -euo pipefail

# ---- 公共库：VELOX_ROOT / die / ok / info / warn / wait_healthz（CWD 切至仓库根） ----
source "$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)"/scripts/common.sh

# ---- 依赖检查 ----
command -v curl >/dev/null 2>&1 || die "未找到 curl，请先安装（apt/dnf install curl 或 brew install curl）"
command -v jq   >/dev/null 2>&1 || die "未找到 jq（JSON 断言依赖它）：apt/dnf install jq 或 brew install jq"

# ---- 常量（端口规划见 velox-mono/docs/architecture.md §5） ----
BUILD_BIN="$VELOX_ROOT/target/debug"
PROV_CFG="velox-demo/provider/config/demo-provider.toml"
CONS_CFG="velox-demo/consumer/config/demo-consumer.toml"
REG_URL="http://127.0.0.1:8300"          # velox-registry
CFG_URL="http://127.0.0.1:8200"          # velox-config
MESH_URL="http://127.0.0.1:8100"         # velox-mesh 数据面
MESH_ADMIN_URL="http://127.0.0.1:8101"   # velox-mesh 管理面
PROV_URL="http://127.0.0.1:9000"         # demo-provider 实例 1
CONS_URL="http://127.0.0.1:9100"         # demo-consumer
GREET_NAME="velox"
TEMPLATE='你好 {name}！（来自配置中心）'
TEMPLATE_JSON="\"$TEMPLATE\""            # config center 存的是 JSON 字符串值

# ---- 辅助：带上下文的 GET / jq ----
must_get() {  # $1 url；失败（非 2xx/连接失败）直接 die
  local url="$1" body
  if ! body="$(curl -fsS -m 5 "$url")"; then
    die "GET $url 失败（连接失败或非 2xx）"
  fi
  printf '%s' "$body"
}

jq_get() {  # $1 原始 body $2 jq 过滤器 $3 失败上下文描述；结果走 stdout
  local out
  if ! out="$(jq -e -r "$2" <<<"$1" 2>/dev/null)"; then
    info "原始响应：$1"
    die "$3：jq 解析失败（filter=$2）"
  fi
  printf '%s' "$out"
}

assert_json_eq() {  # $1 描述 $2 url $3 jq 过滤器 $4 期望值（一次性断言）
  local desc="$1" url="$2" filter="$3" want="$4" body got
  body="$(must_get "$url")"
  got="$(jq_get "$body" "$filter" "$desc")"
  if [[ "$got" != "$want" ]]; then
    info "原始响应：$body"
    die "$desc：期望 '$want'，实际 '$got'"
  fi
  ok "$desc：$got"
}

wait_json_eq() {  # $1 描述 $2 url $3 jq 过滤器 $4 期望值 $5 超时秒 $6 轮询间隔(默认0.5s)
  local desc="$1" url="$2" filter="$3" want="$4" timeout="$5" poll="${6:-0.5}"
  local deadline=$((SECONDS + timeout)) body="" got=""
  while (( SECONDS < deadline )); do
    if body="$(curl -fsS -m 5 "$url" 2>/dev/null)" \
       && got="$(jq -e -r "$filter" <<<"$body" 2>/dev/null)" \
       && [[ "$got" == "$want" ]]; then
      ok "$desc：$got"
      return 0
    fi
    sleep "$poll"
  done
  info "最后一次原始响应：${body:-<请求始终失败>}"
  die "$desc：${timeout}s 内未等到 '$want'（filter=$filter）"
}

start_demo() {  # $1 运行名（.run/<名>.pid + logs/<名>.log） $2 二进制名 $3 配置路径 其后 VAR=VAL...
  local name="$1" bin="$2" cfg="$3"; shift 3
  local pidfile="$VELOX_RUN_DIR/$name.pid" log="$VELOX_LOG_DIR/$name.log"
  if [[ -f "$pidfile" ]] && kill -0 "$(cat "$pidfile" 2>/dev/null)" 2>/dev/null; then
    die "$name 已在运行（pid $(cat "$pidfile")），请先清理后再验收"
  fi
  mkdir -p "$VELOX_RUN_DIR" "$VELOX_LOG_DIR"
  info "启动 $name（覆盖：$*）..."
  nohup env "$@" "$BUILD_BIN/$bin" "$cfg" >>"$log" 2>&1 &
  echo $! >"$pidfile"
}

# ---- 收尾：杀 demo 进程 + 反向停止 core 三件套（无论成败都执行） ----
cleanup() {
  local rc=$?
  trap - EXIT
  info "收尾：清理 demo 进程与 velox 子系统..."
  local name pidfile pid
  for name in demo-provider-2 demo-consumer demo-provider; do
    pidfile="$VELOX_RUN_DIR/$name.pid"
    if [[ -f "$pidfile" ]]; then
      pid="$(cat "$pidfile" 2>/dev/null || true)"
      [[ -n "$pid" ]] && kill -9 "$pid" 2>/dev/null || true
      rm -f "$pidfile"
    fi
  done
  scripts/run.sh stop || true
  # 清除验收期间写穿落盘的模板（velox-config data_dir=./data），给仓库留下干净树（data/ 目录保留）
  rm -f "$VELOX_ROOT/data/demo.json"
  exit "$rc"
}

# ---- 步骤 0：前置检查（端口必须空闲、demo 配置必须在位） ----
info "步骤 0：前置检查（固定端口 8100/8101/8200/8300/9000/9001/9100 必须空闲）"
[[ -f "$VELOX_ROOT/$PROV_CFG" ]] || die "缺少配置 $PROV_CFG（应由 demo 服务实现提供）"
[[ -f "$VELOX_ROOT/$CONS_CFG" ]] || die "缺少配置 $CONS_CFG（应由 demo 服务实现提供）"
# velox-config 写穿落盘（data_dir=./data）：清除上一轮验收持久化的模板，
# 保证每轮验收都从内置模板（builtin）起步；data/ 目录本身保留
rm -f "$VELOX_ROOT/data/demo.json"
bad_ports=()
for p in 8100 8101 8200 8300 9000 9001 9100; do
  # 无 -f：任意 HTTP 响应（含 404）即视为端口被占用
  if curl -s -m 1 -o /dev/null "http://127.0.0.1:$p/" 2>/dev/null; then
    bad_ports+=("$p")
  fi
done
((${#bad_ports[@]} == 0)) || die "端口已被占用：${bad_ports[*]}（请先 scripts/run.sh stop 或清理旧实例）"
ok "前置检查通过"

# ---- 步骤 1：编译 ----
info "步骤 1/5：编译（scripts/build.sh 编译 core 三件套 + cargo 编译 demo 两服务）..."
scripts/build.sh
cargo build --quiet -p demo-provider -p demo-consumer
[[ -x "$BUILD_BIN/demo-provider" ]] || die "缺少 $BUILD_BIN/demo-provider"
[[ -x "$BUILD_BIN/demo-consumer" ]] || die "缺少 $BUILD_BIN/demo-consumer"
ok "编译完成"

trap cleanup EXIT

# ---- 步骤 2：短 TTL 启动 core 三件套（registry(:8300) → config(:8200) → mesh(:8100)） ----
info "步骤 2/5：短 TTL 启动 velox 子系统（heartbeat_ttl=6s / sweep=2s，仅限验收环境）..."
export VELOX_REGISTRY__REGISTRY__HEARTBEAT_TTL_SECS=6
export VELOX_REGISTRY__REGISTRY__SWEEP_INTERVAL_SECS=2
scripts/run.sh start

# ---- 步骤 3：启动 demo 两服务（心跳 2s，加快注册/剔除收敛） ----
info "步骤 3/5：启动 demo-provider(:9000) 与 demo-consumer(:9100)..."
start_demo demo-provider demo-provider "$PROV_CFG" \
  VELOX_PROVIDER__REGISTRY__HEARTBEAT_INTERVAL_SECS=2
start_demo demo-consumer demo-consumer "$CONS_CFG" \
  VELOX_CONSUMER__REGISTRY__HEARTBEAT_INTERVAL_SECS=2

# ---- 步骤 4：端到端断言链（任一步失败即打印原始响应并 exit 1） ----
info "步骤 4/5：端到端断言链..."

# 4.1 五服务健康检查（含 mesh :8101 管理面）
info "── 4.1 健康检查 ──"
assert_json_eq "registry  :8300 /healthz"  "$REG_URL/healthz"        '.service' "velox-registry"
assert_json_eq "config    :8200 /healthz"  "$CFG_URL/healthz"        '.service' "velox-config"
assert_json_eq "mesh 数据面 :8100 /healthz" "$MESH_URL/healthz"       '.service' "velox-mesh"
assert_json_eq "mesh 管理面 :8101 /healthz" "$MESH_ADMIN_URL/healthz" '.service' "velox-mesh"
wait_json_eq   "provider  :9000 /healthz"  "$PROV_URL/healthz"       '.service' "demo-provider" 10
wait_json_eq   "consumer  :9100 /healthz"  "$CONS_URL/healthz"       '.service' "demo-consumer" 10

# 4.2 注册中心：两个 demo 服务均已登记，且各有 1 个健康实例
info "── 4.2 注册与发现 ──"
wait_json_eq "registry 已登记 demo-provider" "$REG_URL/api/v1/registry/services" \
  'index("demo-provider") != null' "true" 15
wait_json_eq "registry 已登记 demo-consumer" "$REG_URL/api/v1/registry/services" \
  'index("demo-consumer") != null' "true" 15
assert_json_eq "demo-provider 健康实例数=1" \
  "$REG_URL/api/v1/registry/services/demo-provider/instances?healthy=true" 'length' 1
assert_json_eq "demo-consumer 健康实例数=1" \
  "$REG_URL/api/v1/registry/services/demo-consumer/instances?healthy=true" 'length' 1

# 4.3 直连 provider：初始模板来自内置默认
info "── 4.3 直连 provider（template_source=builtin） ──"
assert_json_eq "直连 greeting template_source" \
  "$PROV_URL/api/v1/greeting/$GREET_NAME" '.template_source' "builtin"

# 4.4 配置中心热更：写 demo/greeting-template → provider 轮询生效
info "── 4.4 配置中心热更（builtin → config_center） ──"
if ! put_resp="$(curl -fsS -m 5 -X PUT -H 'Content-Type: application/json' \
    -d "$TEMPLATE_JSON" "$CFG_URL/api/v1/config/demo/greeting-template")"; then
  die "PUT $CFG_URL/api/v1/config/demo/greeting-template 失败"
fi
put_ok="$(jq_get "$put_resp" '.ok' "config PUT 响应")"
[[ "$put_ok" == "true" ]] || { info "原始响应：$put_resp"; die "config PUT：期望 ok=true，实际 '$put_ok'"; }
ok "config PUT demo/greeting-template 成功"
readback="$(must_get "$CFG_URL/api/v1/config/demo/greeting-template")"
[[ "$readback" == "$TEMPLATE_JSON" ]] || { info "原始响应：$readback"; die "config 读回不一致：期望 $TEMPLATE_JSON"; }
ok "config 读回一致"
wait_json_eq "provider 模板热更生效" "$PROV_URL/api/v1/provider/status" \
  '.template_source' "config_center" 15

# 4.5 consumer 经 mesh 转发：via/问候语/实例 id 三处一致
info "── 4.5 consumer → mesh → provider 转发一致性 ──"
cons_body="$(must_get "$CONS_URL/api/v1/consumer/greeting/$GREET_NAME")"
direct_body="$(must_get "$PROV_URL/api/v1/greeting/$GREET_NAME")"
prov_status="$(must_get "$PROV_URL/api/v1/provider/status")"
via="$(jq_get "$cons_body" '.via' "consumer greeting")"
[[ "$via" == "velox-mesh" ]] || { info "原始响应：$cons_body"; die "consumer 链路：期望 via=velox-mesh，实际 '$via'"; }
ok "consumer 链路 via=velox-mesh"
direct_greeting="$(jq_get "$direct_body" '.greeting' "直连 provider greeting")"
cons_greeting="$(jq_get "$cons_body" '.provider.greeting' "consumer provider.greeting")"
[[ "$cons_greeting" == "$direct_greeting" ]] || {
  info "consumer 响应：$cons_body"; info "直连响应：$direct_body"
  die "provider.greeting 不一致：经 mesh='$cons_greeting'，直连='$direct_greeting'"
}
ok "provider.greeting 经 mesh 与直连一致：$cons_greeting"
cons_source="$(jq_get "$cons_body" '.provider.template_source' "consumer provider.template_source")"
[[ "$cons_source" == "config_center" ]] || { info "原始响应：$cons_body"; die "consumer 链路 template_source：期望 config_center，实际 '$cons_source'"; }
ok "consumer 链路使用配置中心模板"
prov_iid="$(jq_get "$prov_status" '.instance_id' "provider status instance_id")"
cons_iid="$(jq_get "$cons_body" '.provider.instance_id' "consumer provider.instance_id")"
[[ "$cons_iid" == "$prov_iid" ]] || {
  info "provider status：$prov_status"; info "consumer 响应：$cons_body"
  die "instance_id 不一致：consumer 看到的 '$cons_iid' ≠ provider status '$prov_iid'"
}
ok "instance_id 与 provider status 一致（${prov_iid%%-*}…）"
assert_json_eq "consumer status registered" "$CONS_URL/api/v1/consumer/status" '.registered' "true"

# 4.6 扩容第二 provider 实例（:9001）验证轮询 LB
info "── 4.6 扩容与轮询 LB ──"
start_demo demo-provider-2 demo-provider "$PROV_CFG" \
  VELOX_PROVIDER__SERVER__ADDR=0.0.0.0:9001 \
  VELOX_PROVIDER__REGISTRY__ADVERTISE_PORT=9001 \
  VELOX_PROVIDER__REGISTRY__HEARTBEAT_INTERVAL_SECS=2
wait_json_eq "registry demo-provider 健康实例数=2" \
  "$REG_URL/api/v1/registry/services/demo-provider/instances?healthy=true" 'length' 2 15
# mesh 快照每 poll_secs=5 刷新，等它看到 2 个实例再打流量
wait_json_eq "mesh 快照 demo-provider 实例数=2" \
  "$MESH_ADMIN_URL/api/v1/mesh/routes" '.services["demo-provider"] | length' 2 15
lb_ids=()
for i in 1 2 3 4 5 6; do
  lb_body="$(must_get "$CONS_URL/api/v1/consumer/greeting/$GREET_NAME")"
  lb_ids+=("$(jq_get "$lb_body" '.provider.instance_id' "LB 第 $i 次调用")")
  sleep 0.2
done
distinct="$(printf '%s\n' "${lb_ids[@]}" | sort -u | wc -l | tr -d ' ')"
[[ "$distinct" == "2" ]] || {
  info "实例序列：${lb_ids[*]}"
  die "轮询 LB：6 次调用去重实例数=$distinct，期望 2"
}
ok "轮询 LB：6 次调用覆盖 2 个不同实例"

# 4.7 SIGKILL 第二实例模拟宕机 → TTL(6s)+sweep(2s) 剔除（覆盖 mesh poll=5s 余量）
info "── 4.7 宕机剔除（SIGKILL 第二实例） ──"
pid2="$(cat "$VELOX_RUN_DIR/demo-provider-2.pid")"
kill -9 "$pid2" 2>/dev/null || true
rm -f "$VELOX_RUN_DIR/demo-provider-2.pid"
wait_json_eq "registry demo-provider 健康实例数回落=1" \
  "$REG_URL/api/v1/registry/services/demo-provider/instances?healthy=true" 'length' 1 20 1

# 4.8 SIGTERM 第一 provider 优雅注销 → registry 服务清单不再包含 demo-provider
#     （若 provider 实现仅处理 SIGINT，则由 TTL 剔除兜底，同样能在超时内满足断言）
info "── 4.8 优雅注销（SIGTERM 第一 provider） ──"
pid1="$(cat "$VELOX_RUN_DIR/demo-provider.pid")"
kill -TERM "$pid1" 2>/dev/null || true
wait_json_eq "registry 不再包含 demo-provider" \
  "$REG_URL/api/v1/registry/services" 'index("demo-provider") == null' "true" 15

# ---- 步骤 5：PASS 摘要（EXIT trap 负责收尾） ----
info "步骤 5/5：全部断言通过"
ok "════════ velox-demo 验收 PASS ════════"
info "覆盖：健康检查×6 → 注册/发现（各 1 健康实例）→ 直连 builtin → 配置热更 config_center \
→ mesh 转发一致（via/greeting/instance_id）→ 扩容轮询 LB（6 次 2 实例）→ SIGKILL 剔除回落 \
→ SIGTERM 优雅注销"
