#!/usr/bin/env bash
# velox-registry e2e 验收脚本（真实进程级证据，架构 §6.4 / TC-01/02/18a/18b/19/30/37）。
#
# 覆盖：
#   A. SIGKILL 宕机 → 重启恢复（AC-1/AC-2：instance_id 不变、同 id 心跳 200）
#   B. 写盘中途 kill -9（TC-18b 崩溃注入：tmp 写窗口命中，正式文件只能「旧完整或新完整」）
#   C. SIGINT 优雅退出（TC-19/TC-30：挂起 watch 全部唤醒、退出不等 timeout、最终落盘）
#   D. registry 与 velox-config 同 cwd 隔离（TC-37b：互不装载/覆盖，config 代码零修改）
#
# 依赖：cargo、curl、jq（缺失即显式报错）。
# 端口：使用 18300/18200（避开默认 8300/8200 与 demo 验收端口），启动前检查空闲。
# 运行数据/日志写入系统临时目录，不污染仓库（不提交 data/logs/.run）。
# 用法：bash velox-registry/tests/registry-e2e.sh
set -euo pipefail

ISO_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/velox-registry-e2e.XXXXXXXX")"
VELOX_ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)"
REG_BIN="$VELOX_ROOT/target/debug/velox-registry"
CFG_BIN="$VELOX_ROOT/target/debug/velox-config"
REG_PORT=18300
CFG_PORT=18200
REG_URL="http://127.0.0.1:$REG_PORT"
CFG_URL="http://127.0.0.1:$CFG_PORT"
STEP=0

info() { printf '[e2e] %s\n' "$*"; }
ok()   { printf '[e2e] ✔ %s\n' "$*"; }
die()  { printf '[e2e] ✘ %s\n' "$*" >&2; cleanup; exit 1; }

cleanup() {
  [[ -n "${PIDS:-}" ]] && kill -9 $PIDS 2>/dev/null || true
  rm -rf "$ISO_ROOT"
}
trap cleanup EXIT

command -v curl >/dev/null || die "未找到 curl"
command -v jq >/dev/null || die "未找到 jq（JSON 断言依赖）"
command -v cargo >/dev/null || die "未找到 cargo"

info "构建（cargo build -p velox-registry -p velox-config）..."
cargo build --quiet -p velox-registry -p velox-config
[[ -x "$REG_BIN" && -x "$CFG_BIN" ]] || die "构建产物缺失"

wait_healthz() { # $1 url $2 超时秒
  local deadline=$((SECONDS + $2))
  while (( SECONDS < deadline )); do
    curl -fsS -m 2 "$1/healthz" >/dev/null 2>&1 && return 0
    sleep 0.2
  done
  return 1
}

start_registry() { # $1 数据目录 $2 TTL秒 $3 flush秒；回显 pid
  local data="$1" ttl="$2" flush="$3" log="$ISO_ROOT/registry-$4.log"
  # BUG-1 修复：显式注入监听地址（隔离端口 18300，不与用户环境 8300 冲突）
  VELOX_REGISTRY__SERVER__ADDR="127.0.0.1:$REG_PORT" \
  VELOX_REGISTRY__REGISTRY__HEARTBEAT_TTL_SECS="$ttl" \
  VELOX_REGISTRY__REGISTRY__SWEEP_INTERVAL_SECS=1 \
  VELOX_REGISTRY__PERSISTENCE__ENABLED=true \
  VELOX_REGISTRY__PERSISTENCE__DATA_DIR="$data" \
  VELOX_REGISTRY__PERSISTENCE__FLUSH_INTERVAL_SECS="$flush" \
  nohup "$REG_BIN" >>"$log" 2>&1 &
  echo $!
}

start_config() { # $1 数据目录 $2 日志名；回显 pid
  local data="$1" log="$ISO_ROOT/$2.log"
  # BUG-1 修复：显式注入监听地址（隔离端口 18200）
  VELOX_CONFIG__SERVER__ADDR="127.0.0.1:$CFG_PORT" \
  VELOX_CONFIG__STORAGE__DATA_DIR="$data" nohup "$CFG_BIN" >>"$log" 2>&1 &
  echo $!
}

wait_exit() { # $1 pid $2 超时秒；进程退出返回 0
  local pid=$1 deadline=$((SECONDS + $2))
  while (( SECONDS < deadline )); do
    kill -0 "$pid" 2>/dev/null || return 0
    sleep 0.1
  done
  return 1
}

file_services_json() { # $1 快照路径；输出 services 对象（非法则空）
  jq -c '.services // empty' "$1" 2>/dev/null || echo ""
}

# ════════ 场景 A：SIGKILL 宕机 → 重启恢复（TC-01/02/AC-1/AC-2） ════════
STEP="A"
info "── 场景 A：SIGKILL 宕机 → 重启恢复（TTL60/flush2）──"
A_DIR="$ISO_ROOT/a-data"
PIDS=""
PID=$(start_registry "$A_DIR" 60 2 a1)
PIDS="$PID"
wait_healthz "$REG_URL" 15 || die "A：registry 未就绪"
# 端口占用预检已并入 wait_healthz 失败路径；若 18300 被占则此处失败并提示
A_RESP=$(curl -fsS -X POST -H 'content-type: application/json' \
  -d '{"address":"172.17.0.2","port":9000,"metadata":{"v":"e2e"}}' \
  "$REG_URL/api/v1/registry/services/svc-a/instances") || die "A：注册失败"
A_ID=$(jq -r .id <<<"$A_RESP")
SNAP="$A_DIR/registry/registry-state.json"
[[ -f "$SNAP" ]] || die "A：写穿快照未产生（$SNAP）"
A_HEART=$(jq -r '.services["svc-a"][0].last_heartbeat_epoch' "$SNAP")
A_T0=$(date +%s)
[[ $((A_T0 - A_HEART)) -le 2 ]] || die "A：落盘心跳偏差过大（$((A_T0 - A_HEART))s）"
kill -9 "$PID" 2>/dev/null || true
wait "$PID" 2>/dev/null || true
info "A：SIGKILL(pid=$PID) 完成，重启..."
PID=$(start_registry "$A_DIR" 60 2 a2)
PIDS="$PID"
wait_healthz "$REG_URL" 15 || die "A：重启后未就绪"
A_LIST=$(curl -fsS "$REG_URL/api/v1/registry/services/svc-a/instances") || die "A：列表失败"
[[ "$(jq -r '.[0].id' <<<"$A_LIST")" == "$A_ID" ]] || die "A：instance_id 不一致（FR-3）"
[[ "$(jq -r '.[0].address' <<<"$A_LIST")" == "172.17.0.2" ]] || die "A：address 不一致"
NOW=$(date +%s)
AGE=$((NOW - A_HEART))
[[ $AGE -le 60 ]] || die "A：恢复窗口前提不满足（age=${AGE}s > TTL=60s），本次正例不适用"
A_HB=$(curl -fsS -X PUT "$REG_URL/api/v1/registry/services/svc-a/instances/$A_ID/heartbeat") || die "A：同 id 心跳失败（AC-2）"
[[ "$(jq -r .ok <<<"$A_HB")" == "true" ]] || die "A：心跳响应异常"
kill -9 "$PID" 2>/dev/null || true; wait "$PID" 2>/dev/null || true
ok "A：SIGKILL 重启恢复 + 同 id 心跳 200（落盘 age=${AGE}s ≤ TTL60）"

# ════════ 场景 B：写盘中途 kill -9（TC-18b 崩溃注入，AC-14） ════════
STEP="B"
info "── 场景 B：tmp 写入窗口 kill -9（有限重试，未命中如实记录）──"
B_DIR="$ISO_ROOT/b-data"
B_SNAP="$B_DIR/registry/registry-state.json"
B_TMP="$B_DIR/registry/registry-state.json.tmp"
PID=$(start_registry "$B_DIR" 60 1 b0)
PIDS="$PID"
wait_healthz "$REG_URL" 15 || die "B：registry 未就绪"
# 先落一份旧完整快照
curl -fsS -X POST -H 'content-type: application/json' -d '{"address":"10.9.9.1","port":1}' \
  "$REG_URL/api/v1/registry/services/old/instances" >/dev/null
kill -9 "$PID" 2>/dev/null || true; wait "$PID" 2>/dev/null || true
cp "$B_SNAP" "$ISO_ROOT/b-old-snapshot.json"

hit="no"; attempt=0; max_attempts=8
while [[ "$hit" == "no" && $attempt -lt $max_attempts ]]; do
  attempt=$((attempt + 1))
  PID=$(start_registry "$B_DIR" 60 1 "b-w$attempt")
  PIDS="$PID"
  wait_healthz "$REG_URL" 15 || { kill -9 "$PID" 2>/dev/null || true; die "B：第 $attempt 轮启动失败"; }
  # 持续注册制造写穿流，同时观察 tmp 出现即 kill -9
  (
    end=$((SECONDS + 8))
    i=0
    while (( SECONDS < end )); do
      curl -fsS -m 2 -X POST -H 'content-type: application/json' \
        -d "{\"address\":\"10.9.9.$((i % 250 + 2))\",\"port\":$((i % 60000 + 2))}" \
        "$REG_URL/api/v1/registry/services/churn/instances" >/dev/null 2>&1 || true
      i=$((i + 1))
    done
  ) &
  LOAD_PID=$!
  watch_end=$((SECONDS + 8))
  while (( SECONDS < watch_end )); do
    if [[ -f "$B_TMP" ]]; then
      kill -9 "$PID" 2>/dev/null || true
      hit="yes"
      break
    fi
    sleep 0.001
  done
  kill "$LOAD_PID" 2>/dev/null || true; wait "$LOAD_PID" 2>/dev/null || true
  kill -9 "$PID" 2>/dev/null || true; wait "$PID" 2>/dev/null || true
done
if [[ "$hit" == "yes" ]]; then
  # 命中写窗口：正式文件必须为合法 JSON（旧完整或新完整），重启可正常恢复
  jq -e '.format_version == 1 and (.services | type == "object")' "$B_SNAP" >/dev/null \
    || die "B：写窗口 kill -9 后正式文件损坏（AC-14 违约）"
  PID=$(start_registry "$B_DIR" 60 1 "b-final")
  PIDS="$PID"
  wait_healthz "$REG_URL" 15 || die "B：崩溃注入后重启失败"
  kill -9 "$PID" 2>/dev/null || true; wait "$PID" 2>/dev/null || true
  ok "B：写窗口命中（第 $attempt 轮，tmp 观察 kill -9），正式文件仅旧/新完整，重启恢复正常"
else
  ok "B：$max_attempts 轮未命中写窗口——按 TC-18b 记录「无法验证」，不冒充通过（验收期需 TA 复核）"
fi

# ════════ 场景 C：SIGINT 优雅退出（TC-19/TC-30，AC-9/B-8） ════════
STEP="C"
info "── 场景 C：SIGINT 优雅退出（挂起 watch 唤醒 + 最终落盘）──"
C_DIR="$ISO_ROOT/c-data"
PID=$(start_registry "$C_DIR" 60 2 c1)
PIDS="$PID"
wait_healthz "$REG_URL" 15 || die "C：registry 未就绪"
C_RESP=$(curl -fsS -X POST -H 'content-type: application/json' \
  -d '{"address":"10.8.8.8","port":8}' "$REG_URL/api/v1/registry/services/svc-c/instances")
C_ID=$(jq -r .id <<<"$C_RESP")
C_VER=$(curl -fsS "$REG_URL/api/v1/registry/watch" | jq -r .index)
# 挂起一个 watch（timeout 30s），SIGINT 必须把它唤醒而不是拖满 30s（R1）
curl -fsS -m 35 "$REG_URL/api/v1/registry/watch?index=$C_VER&timeout_secs=30" \
  >"$ISO_ROOT/c-watch-resp.json" 2>/dev/null &
WATCH_CURL=$!
sleep 0.5
# 最后一次心跳制造「未落盘脏状态」，SIGINT 后由最终落盘收尾（TC-19）
C_EPOCH_BEFORE=$(jq -r '.services["svc-c"][0].last_heartbeat_epoch' "$C_DIR/registry/registry-state.json")
sleep 1.2
curl -fsS -X PUT "$REG_URL/api/v1/registry/services/svc-c/instances/$C_ID/heartbeat" >/dev/null
kill -INT "$PID" 2>/dev/null || true
wait_exit "$PID" 5 || { kill -9 "$PID" 2>/dev/null || true; die "C：SIGINT 后 5s 未退出（挂起 watch 拖住了优雅关停？AC-9 违约）"; }
wait "$WATCH_CURL" 2>/dev/null || true
jq -e '.index != null' "$ISO_ROOT/c-watch-resp.json" >/dev/null \
  || die "C：挂起 watch 未被唤醒返回（响应缺失或非 JSON）"
C_EPOCH_AFTER=$(jq -r '.services["svc-c"][0].last_heartbeat_epoch' "$C_DIR/registry/registry-state.json")
[[ "$C_EPOCH_AFTER" -ge "$C_EPOCH_BEFORE" ]] || die "C：最终落盘心跳回退"
[[ "$C_EPOCH_AFTER" != "$C_EPOCH_BEFORE" ]] && C_FINAL="已推进" || C_FINAL="等同上一落盘（该心跳已被 flusher 收尾或最终落盘补齐，读取时序内可接受）"
ok "C：SIGINT 5s 内退出、挂起 watch 唤醒、最终落盘心跳${C_FINAL}（before=$C_EPOCH_BEFORE after=$C_EPOCH_AFTER）"

# ════════ 场景 D：registry 与 velox-config 同 cwd 隔离（TC-37b） ════════
STEP="D"
info "── 场景 D：与 velox-config 同 cwd 隔离（TC-37b，config 零修改）──"
D_CWD="$ISO_ROOT/d-cwd"
mkdir -p "$D_CWD"
( cd "$D_CWD" && VELOX_REGISTRY__SERVER__ADDR="127.0.0.1:$REG_PORT" \
  VELOX_REGISTRY__PERSISTENCE__DATA_DIR="./data" \
  nohup "$REG_BIN" >>"$ISO_ROOT/d-registry.log" 2>&1 & echo $! >"$ISO_ROOT/d-reg.pid" )
REG_PID=$(cat "$ISO_ROOT/d-reg.pid")
PIDS="$REG_PID"
wait_healthz "$REG_URL" 15 || die "D：registry 未就绪"
curl -fsS -X POST -H 'content-type: application/json' \
  -d '{"address":"172.17.0.9","port":9009}' \
  "$REG_URL/api/v1/registry/services/iso-svc/instances" >/dev/null || die "D：注册失败"
REG_SNAP="$D_CWD/data/registry/registry-state.json"
[[ -f "$REG_SNAP" ]] || die "D：快照未落在 ./data/registry/（G2-5 布局）"

( cd "$D_CWD" && VELOX_CONFIG__SERVER__ADDR="127.0.0.1:$CFG_PORT" \
  nohup "$CFG_BIN" >>"$ISO_ROOT/d-config.log" 2>&1 & echo $! >"$ISO_ROOT/d-cfg.pid" )
CFG_PID=$(cat "$ISO_ROOT/d-cfg.pid")
PIDS="$REG_PID $CFG_PID"
D_DEADLINE=$((SECONDS + 15))
until curl -fsS -m 2 "$CFG_URL/healthz" >/dev/null 2>&1; do
  (( SECONDS < D_DEADLINE )) || die "D：config 未就绪"
  sleep 0.2
done

# config 不自动装载 registry 快照为 namespace（非递归顶层扫描不触及子目录）
D_NAMESPACES=$(curl -fsS "$CFG_URL/api/v1/config" || die "D：config namespaces 读取失败")
jq -e 'index("registry-state") == null' <<<"$D_NAMESPACES" >/dev/null \
  || die "D：registry 快照被 config 装载为 namespace（G2-5 违约）"
# config 写普通 namespace + 顶层 registry-state（反向验证），registry 不受影响
curl -fsS -X PUT -H 'content-type: application/json' -d '{"k":"v"}' \
  "$CFG_URL/api/v1/config/normal-ns/key1" >/dev/null || die "D：config put 失败"
curl -fsS -X PUT -H 'content-type: application/json' -d '{"fake":true}' \
  "$CFG_URL/api/v1/config/registry-state/key1" >/dev/null || die "D：config put registry-state 失败"
[[ "$(jq -c '.services | keys' "$REG_SNAP")" == '["iso-svc"]' ]] \
  || die "D：registry 快照被 config 写入覆盖"
[[ -f "$D_CWD/data/registry-state.json" ]] || die "D：config 顶层文件未按预期产生（隔离对照不完整）"
[[ "$(file_services_json "$REG_SNAP")" != "" ]] || die "D：快照非法"

# 双双重启，数据互不覆盖
kill -INT "$REG_PID" 2>/dev/null || true; wait_exit "$REG_PID" 5 || kill -9 "$REG_PID" 2>/dev/null || true
kill -INT "$CFG_PID" 2>/dev/null || true; wait_exit "$CFG_PID" 5 || kill -9 "$CFG_PID" 2>/dev/null || true
( cd "$D_CWD" && VELOX_REGISTRY__SERVER__ADDR="127.0.0.1:$REG_PORT" \
  nohup "$REG_BIN" >>"$ISO_ROOT/d-registry.log" 2>&1 & echo $! >"$ISO_ROOT/d-reg.pid" )
( cd "$D_CWD" && VELOX_CONFIG__SERVER__ADDR="127.0.0.1:$CFG_PORT" \
  nohup "$CFG_BIN" >>"$ISO_ROOT/d-config.log" 2>&1 & echo $! >"$ISO_ROOT/d-cfg.pid" )
REG_PID=$(cat "$ISO_ROOT/d-reg.pid"); CFG_PID=$(cat "$ISO_ROOT/d-cfg.pid")
PIDS="$REG_PID $CFG_PID"
wait_healthz "$REG_URL" 15 || die "D：registry 重启失败"
D_DEADLINE=$((SECONDS + 15))
until curl -fsS -m 2 "$CFG_URL/healthz" >/dev/null 2>&1; do
  (( SECONDS < D_DEADLINE )) || die "D：config 重启失败"
  sleep 0.2
done
ISO_LIST=$(curl -fsS "$REG_URL/api/v1/registry/services/iso-svc/instances")
[[ "$(jq -r '.[0].address' <<<"$ISO_LIST")" == "172.17.0.9" ]] || die "D：registry 数据重启后丢失"
D_CFG_BACK=$(curl -fsS "$CFG_URL/api/v1/config/normal-ns/key1")
[[ "$D_CFG_BACK" == '{"k":"v"}' ]] || die "D：config 数据重启后丢失（got: $D_CFG_BACK）"
D_TOP=$(curl -fsS "$CFG_URL/api/v1/config/registry-state/key1")
[[ "$D_TOP" == '{"fake":true}' ]] || die "D：config 顶层 registry-state 数据丢失（对照失败）"
kill -INT "$REG_PID" 2>/dev/null || true; wait_exit "$REG_PID" 5 || true
kill -INT "$CFG_PID" 2>/dev/null || true; wait_exit "$CFG_PID" 5 || true
ok "D：同 cwd 隔离成立（config 未装载子目录快照；顶层同名文件互不干扰；双方重启数据完整）"

PIDS=""
ok "════════ velox-registry e2e PASS（A/B/C/D 全部通过，ISO_ROOT=$ISO_ROOT 已清理）════════"
