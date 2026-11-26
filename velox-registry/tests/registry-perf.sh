#!/usr/bin/env bash
# velox-registry 性能专项（AC-13 / TC-38 协议，G2-6/RQ-2 定案）。
#
# 真 v0.1 基线：commit 28c9114（该提交 registry 仍为内存实现），隔离检出 +
# 独立 CARGO_TARGET_DIR，不切当前工作分支、不修改当前代码。
# 判据：注册/心跳/查询每端点 v0.2/v0.1 P99 ≤ 2 才满足 AC-13；
#       enabled=false 对照仅辅助，不得替代 v0.1 基线。
#
# 冻结测量参数（TC-38）：
#   - 100 服务 × 10 实例 = 1000 实例；心跳负载按 200/s 排程（每实例 5s 续约）；
#   - 查询 100 请求/秒，services 与 instances?healthy=true 交替（分端点统计）；
#   - 注册 churn：先注销后补同槽位（每 worker 独立 churn 槽位，槽位文件无竞争）；
#   - 客户端并发 ≤32；每次请求从发起到完整响应以 curl time_total 计时；
#   - 预热 ≥30s、采样 ≥60s、每被比较端点 ≥1000 有效样本（不足判据即不满足）；
#   - P99 用有效成功样本 nearest-rank ceil(0.99n)；全部失败/超时另计，
#     不得靠丢失败样本改善 P99；任何端点出现请求错误 → 记录缺陷，不得判通过。
#
# 依赖：cargo、curl、jq、awk、git。运行数据写入系统临时目录，不污染仓库。
# 用法：bash velox-registry/tests/registry-perf.sh [采样秒数（默认 60）]
set -euo pipefail

BASELINE_COMMIT="28c91140dd14a29d25dcbc270b2bb23aeb32b2f0"
VELOX_ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)"
SAMPLE_SECS="${1:-60}"
WARMUP_SECS=30
CLIENT_CONCURRENCY=32
SERVICES=100
INSTANCES_PER_SERVICE=10
TOTAL_SLOTS=$((SERVICES * INSTANCES_PER_SERVICE))
WORKER_TICK_SEC=0.05   # 32 worker × 20 tick/s = 640 tick/s，心跳/查询/churn 按 tick 分摊

PERF_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/velox-registry-perf.XXXXXXXX")"
BASELINE_SRC="$PERF_ROOT/baseline"
V1_TARGET="$PERF_ROOT/target-v01"
V1_DATA="$PERF_ROOT/data-v01"
V2_DATA="$PERF_ROOT/data-v02"
V1_PORT=18401
V2_PORT=18402
RAW="$PERF_ROOT/raw"
mkdir -p "$RAW/v01" "$RAW/v02"

SERVER_PID=""
ALL_PIDS=""
info() { printf '[perf] %s\n' "$*"; }
die()  { printf '[perf] ✘ %s\n' "$*" >&2; exit_with_logs 1; }
exit_with_logs() {
  [[ -n "$SERVER_PID" ]] && kill -9 "$SERVER_PID" 2>/dev/null || true
  info "原始测量记录保留在：$PERF_ROOT（raw/ 逐请求样本 + 服务日志），验收报告须引用绝对路径"
  exit "$1"
}

command -v cargo >/dev/null || die "未找到 cargo"
command -v curl  >/dev/null || die "未找到 curl"
command -v jq    >/dev/null || die "未找到 jq"
command -v git   >/dev/null || die "未找到 git"

# ─── 1. 基线隔离检出（不切当前分支，不修改当前代码） ───
info "检出真 v0.1 基线 $BASELINE_COMMIT → $BASELINE_SRC ..."
git -C "$VELOX_ROOT" cat-file -e "$BASELINE_COMMIT" || die "基线提交不存在：$BASELINE_COMMIT"
mkdir -p "$BASELINE_SRC"
git -C "$VELOX_ROOT" archive "$BASELINE_COMMIT" | tar -x -C "$BASELINE_SRC"
# 基线自证：该提交 registry 必须仍是 v0.1 内存实现（无 store.rs / 无 [persistence]）
if [[ -f "$BASELINE_SRC/velox-registry/src/store.rs" ]]; then
  die "基线 $BASELINE_COMMIT 非内存 v0.1（存在 store.rs），协议前置不成立"
fi
if grep -q '\[persistence\]' "$BASELINE_SRC/velox-registry/config/velox-registry.toml" 2>/dev/null; then
  die "基线 $BASELINE_COMMIT 已含 [persistence]，非 v0.1"
fi
info "基线自证通过（无 store.rs、无 [persistence]）"

# ─── 2. 构建（同 release profile；基线用独立 CARGO_TARGET_DIR） ───
info "构建 v0.2（当前代码，release）..."
cargo build --release --quiet -p velox-registry
V2_BIN="$VELOX_ROOT/target/release/velox-registry"
info "构建 v0.1 基线（独立 CARGO_TARGET_DIR=$V1_TARGET）..."
CARGO_TARGET_DIR="$V1_TARGET" cargo build --release --quiet \
  --manifest-path "$BASELINE_SRC/Cargo.toml" -p velox-registry
V1_BIN="$V1_TARGET/release/velox-registry"
[[ -x "$V1_BIN" && -x "$V2_BIN" ]] || die "构建产物缺失"

start_server() { # $1 bin $2 port $3 data_dir $4 日志名 → 回显 pid
  local bin=$1 port=$2 data=$3
  local log="$PERF_ROOT/$4.log"
  VELOX_REGISTRY__SERVER__ADDR="127.0.0.1:$port" \
  VELOX_REGISTRY__REGISTRY__HEARTBEAT_TTL_SECS=60 \
  VELOX_REGISTRY__REGISTRY__SWEEP_INTERVAL_SECS=5 \
  VELOX_REGISTRY__PERSISTENCE__ENABLED=true \
  VELOX_REGISTRY__PERSISTENCE__DATA_DIR="$data" \
  VELOX_REGISTRY__PERSISTENCE__FLUSH_INTERVAL_SECS=5 \
  nohup "$bin" >>"$log" 2>&1 &
  echo $!
}

wait_ready() { # $1 url
  local deadline=$((SECONDS + 20))
  while (( SECONDS < deadline )); do
    curl -fsS -m 2 "$1/healthz" >/dev/null 2>&1 && return 0
    sleep 0.2
  done
  return 1
}

# 样本行格式（制表符分隔）：endpoint \t http_code \t time_total_ms
record() { # $1 样本目录 $2 endpoint $3 code $4 秒（time_total）
  local ms
  ms=$(awk -v t="$4" 'BEGIN{printf "%d", t * 1000}')
  printf '%s\t%s\t%s\n' "$2" "$3" "$ms" >>"$1/samples.tsv"
}

# ─── 3. 负载驱动 ───
fill_pool() { # $1 base_url $2 slots 根目录
  local base=$1 slots=$2 svc slot resp id
  info "填充 ${SERVICES}×${INSTANCES_PER_SERVICE} 稳定池 + $CLIENT_CONCURRENCY churn 槽位 ..."
  mkdir -p "$slots/churn.d"
  for svc in $(seq 1 "$SERVICES"); do
    for slot in $(seq 1 "$INSTANCES_PER_SERVICE"); do
      resp=$(curl -fsS -m 5 -X POST -H 'content-type: application/json' \
        -d "{\"address\":\"10.$((svc % 250)).$((slot % 250)).1\",\"port\":$((9000 + slot))}" \
        "$base/api/v1/registry/services/svc-$svc/instances") \
        || die "填充注册失败（svc-$svc slot-$slot）"
      id=$(jq -r .id <<<"$resp")
      printf '%s\t%s\n' "svc-$svc-$slot" "$id" >>"$slots/stable.tsv"
    done
  done
  # churn 槽位：每个 worker 一个独立槽位文件（先注册初始实例，采样期先注销后补）
  local w
  for w in $(seq 1 "$CLIENT_CONCURRENCY"); do
    resp=$(curl -fsS -m 5 -X POST -H 'content-type: application/json' \
      -d "{\"address\":\"10.$((w % 250)).99.99\",\"port\":7777}" \
      "$base/api/v1/registry/services/svc-$w/instances") \
      || die "填充 churn 失败（worker-$w）"
    id=$(jq -r .id <<<"$resp")
    echo "$id" >"$slots/churn.d/$w"
  done
  info "填充完成（稳定池 $TOTAL_SLOTS + churn $CLIENT_CONCURRENCY）"
}

run_load() { # $1 base_url $2 样本目录 $3 秒数 $4 slots 根目录 $5 阶段名
  local base=$1 rawdir=$2 secs=$3 slots=$4 phase=$5
  mkdir -p "$rawdir"
  : >"$rawdir/samples.tsv"
  local end=$((SECONDS + secs))
  local pids=() w
  # 速率分摊（32 worker × 20 tick/s）：
  #   心跳 200/s → 每 worker 每 3 tick 一次（6.67/s）；
  #   查询 100/s → 每 worker 每 6 tick 一次（3.33/s，奇偶交替两查询端点）；
  #   churn 20/s（先注销后补同槽位）→ 每 worker 每 32 tick 一对（0.625/s）。
  for w in $(seq 1 "$CLIENT_CONCURRENCY"); do
  (
    # worker 局部状态：心跳轮转指针（32 worker 分摊 1000 实例 → 每实例 ~5s 续约）
    hb_idx=$((w % TOTAL_SLOTS + 1))
    churn_id_file="$slots/churn.d/$w"
    stable="$slots/stable.tsv"
    tick=0
    while (( SECONDS < end )); do
      # 心跳（主负载）：轮转稳定池
      if (( tick % 3 == 0 )); then
        hb_id=$(awk -F'\t' -v i="$hb_idx" 'NR==i{print $2}' "$stable")
        hb_svc=$(( (hb_idx - 1) / INSTANCES_PER_SERVICE + 1 ))
        hb_slot=$(( (hb_idx - 1) % INSTANCES_PER_SERVICE + 1 ))
        if [[ -n "$hb_id" ]]; then
          out=$(curl -m 5 -o /dev/null -w '%{http_code} %{time_total}' \
            -X PUT "$base/api/v1/registry/services/svc-$hb_svc/instances/$hb_id/heartbeat" 2>/dev/null) \
            || out="ERR ERR"
          printf 'heartbeat\t%s\t%s\n' "${out%% *}" \
            "$(awk -v t="${out#* }" 'BEGIN{printf "%d", t*1000}')" >>"$rawdir/samples.tsv"
        fi
        hb_idx=$(( hb_idx % TOTAL_SLOTS + 1 ))
      fi
      # 查询：services / instances?healthy=true 交替
      if (( tick % 6 == 0 )); then
        if (( tick % 12 == 0 )); then
          out=$(curl -m 5 -o /dev/null -w '%{http_code} %{time_total}' \
            "$base/api/v1/registry/services" 2>/dev/null) || out="ERR ERR"
          printf 'services\t%s\t%s\n' "${out%% *}" \
            "$(awk -v t="${out#* }" 'BEGIN{printf "%d", t*1000}')" >>"$rawdir/samples.tsv"
        else
          out=$(curl -m 5 -o /dev/null -w '%{http_code} %{time_total}' \
            "$base/api/v1/registry/services/svc-$((w % SERVICES + 1))/instances?healthy=true" 2>/dev/null) || out="ERR ERR"
          printf 'instances_healthy\t%s\t%s\n' "${out%% *}" \
            "$(awk -v t="${out#* }" 'BEGIN{printf "%d", t*1000}')" >>"$rawdir/samples.tsv"
        fi
      fi
      # churn：先注销后补同槽位（每 worker 独立槽位文件，无并发竞争）
      if (( (tick + w) % 32 == 0 )); then
        old_id=$(cat "$churn_id_file" 2>/dev/null || echo "")
        if [[ -n "$old_id" ]]; then
          out=$(curl -m 5 -o /dev/null -w '%{http_code} %{time_total}' \
            -X DELETE "$base/api/v1/registry/services/svc-$w/instances/$old_id" 2>/dev/null) || out="ERR ERR"
          printf 'deregister\t%s\t%s\n' "${out%% *}" \
            "$(awk -v t="${out#* }" 'BEGIN{printf "%d", t*1000}')" >>"$rawdir/samples.tsv"
        fi
        out=$(curl -m 5 -o /dev/null -w '%{http_code} %{time_total}' -X POST \
          -H 'content-type: application/json' \
          -d "{\"address\":\"10.$((w % 250)).99.99\",\"port\":7777}" \
          "$base/api/v1/registry/services/svc-$w/instances" 2>/dev/null) || out="ERR ERR"
        printf 'register\t%s\t%s\n' "${out%% *}" \
          "$(awk -v t="${out#* }" 'BEGIN{printf "%d", t*1000}')" >>"$rawdir/samples.tsv"
        resp=$(curl -fsS -m 5 "$base/api/v1/registry/services/svc-$w/instances" 2>/dev/null || echo "")
        new_id=$(jq -r '.[] | select(.address == "10.'"$((w % 250))"'.99.99") | .id' <<<"$resp" 2>/dev/null | head -1)
        if [[ -n "$new_id" && "$new_id" != "null" ]]; then
          echo "$new_id" >"$churn_id_file"
        fi
      fi
      tick=$(( tick + 1 ))
      sleep "$WORKER_TICK_SEC"
    done
  ) &
    pids+=($!)
  done
  for p in "${pids[@]}"; do wait "$p" 2>/dev/null || true; done
  info "$phase 完成（$(wc -l <"$rawdir/samples.tsv" | tr -d ' ') 条原始样本）"
}

# 汇总：分端点有效样本数、错误数、P50/P99
# nearest-rank：P50=ceil(0.50n)、P99=ceil(0.99n)（BSD awk 无 asort，用 sort -n + sed 取秩）
# $1 样本目录 → 写 $1/summary.tsv；stdout 输出摘要
summarize() {
  local rawdir=$1 out="$1/summary.tsv" ep
  echo -e "endpoint\tsamples\terrors\tP50_ms\tP99_ms" >"$out"
  local times="$PERF_ROOT/.times.$$"
  for ep in heartbeat services instances_healthy register deregister; do
    # 样本总数与错误数（错误 = 非 200 或时长非法；全部另计，不丢样本）
    local totals
    totals=$(awk -F'\t' -v ep="$ep" '
      $1 == ep { total++; if ($2 == 200 && $3 ~ /^[0-9]+$/) ok++; else errors++ }
      END { print total + 0, errors + 0 }' "$rawdir/samples.tsv")
    local total=${totals%% *} errors=${totals##* }
    # 有效成功样本排序（nearest-rank）
    awk -F'\t' -v ep="$ep" '$1 == ep && $2 == 200 && $3 ~ /^[0-9]+$/ {print $3}' \
      "$rawdir/samples.tsv" | sort -n >"$times"
    local n p50_rank p99_rank p50 p99
    n=$(wc -l <"$times" | tr -d ' ')
    if (( n > 0 )); then
      p50_rank=$(( (n + 1) / 2 ))
      p99_rank=$(( (n * 99 + 99) / 100 ))
      p50=$(sed -n "${p50_rank}p" "$times")
      p99=$(sed -n "${p99_rank}p" "$times")
      echo -e "$ep\t$total\t$errors\t$p50\t$p99" >>"$out"
    else
      echo -e "$ep\t$total\t$errors\t-\t-" >>"$out"
    fi
  done
  rm -f "$times"
  cat "$out"
}

run_version() { # $1 bin $2 port $3 data_dir $4 tag
  local bin=$1 port=$2 data=$3 tag=$4
  local url="http://127.0.0.1:$port"
  info "═══ $tag 启动（$url，persistence enabled）═══"
  SERVER_PID=$(start_server "$bin" "$port" "$data" "$tag")
  ALL_PIDS="$ALL_PIDS $SERVER_PID"
  wait_ready "$url" || { info "$tag 未就绪，日志：$PERF_ROOT/$tag.log"; exit_with_logs 1; }
  local slots="$PERF_ROOT/slots-$tag"
  info "$tag 预热 ${WARMUP_SECS}s ..."
  fill_pool "$url" "$slots"
  run_load "$url" "$RAW/$tag-warmup" "$WARMUP_SECS" "$slots" "预热"
  info "$tag 采样 ${SAMPLE_SECS}s ..."
  run_load "$url" "$RAW/$tag" "$SAMPLE_SECS" "$slots" "采样"
  summarize "$RAW/$tag" >"$PERF_ROOT/$tag.summary.render"
  info "$tag 摘要："
  cat "$PERF_ROOT/$tag.summary.render"
  kill -INT "$SERVER_PID" 2>/dev/null || true
  sleep 1
  kill -9 "$SERVER_PID" 2>/dev/null || true
  SERVER_PID=""
}

# ─── 4. 双版本执行（同机、同 release profile、同负载计划） ───
run_version "$V1_BIN" "$V1_PORT" "$V1_DATA" "v01"
run_version "$V2_BIN" "$V2_PORT" "$V2_DATA" "v02"

# ─── 5. 判定（分端点比值每项 ≤2；错误不得忽略；样本不足即不满足） ───
info "═══ AC-13 判定（TC-38：每端点 v0.2/v0.1 P99 ≤ 2）═══"
pass="yes"
while IFS=$'\t' read -r ep s1 e1 p501 p991; do
  [[ "$ep" == "endpoint" ]] && continue
  line2=$(awk -F'\t' -v ep="$ep" '$1 == ep {print}' "$RAW/v02/summary.tsv")
  if [[ -z "$line2" ]]; then
    info "$ep：v0.2 无样本——判据不满足"; pass="no"; continue
  fi
  s2=$(cut -f2 <<<"$line2"); e2=$(cut -f3 <<<"$line2"); p992=$(cut -f5 <<<"$line2")
  if (( e1 > 0 || e2 > 0 )); then
    info "$ep：存在请求错误（v0.1=$e1 v0.2=$e2）——按协议记录缺陷，本轮不得判 AC-13 通过"
    pass="no"
  fi
  if (( s1 < 1000 || s2 < 1000 )); then
    info "$ep：有效样本不足 1000（v0.1=$s1 v0.2=$s2）——按协议判据不满足（延长采样后复测）"
    pass="no"
  fi
  if [[ "$p991" == "-" || "$p992" == "-" || "$p991" == "0" ]]; then
    info "$ep：P99 不可用——判据不满足"; pass="no"; continue
  fi
  ratio=$(awk -v a="$p992" -v b="$p991" 'BEGIN{printf "%.2f", a / b}')
  if awk -v r="$ratio" 'BEGIN{exit !(r <= 2)}'; then
    info "$ep：v0.1 P99=${p991}ms，v0.2 P99=${p992}ms，比值=$ratio → 满足"
  else
    info "$ep：v0.1 P99=${p991}ms，v0.2 P99=${p992}ms，比值=$ratio → 超出（AC-13 违约）"
    pass="no"
  fi
done <"$RAW/v01/summary.tsv"

[[ "$pass" == "yes" ]] \
  && info "════════ AC-13 性能专项 PASS（每端点比值 ≤2、无错误、样本充足）════════" \
  || die "AC-13 性能专项不满足或存在错误——详见分端点输出与 $PERF_ROOT 原始记录"
