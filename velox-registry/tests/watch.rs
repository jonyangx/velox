//! watch 长轮询集成测试（架构 §6.2 watch.rs）：
//! 缺省 index 立即返回 × {空表 v0、恢复有实例 v0、version>0}（G2-2/RQ-3）；
//! 显式 index=0 × {v0 挂起、v>0 立即}；显式相等挂起（反例保留）；挂起中被注册/
//! 注销/sweep/复活心跳唤醒严格 <1s 且快照正确（AC-8/AC-10/QA-3）；timeout 非法
//! 400、解析失败统一映射（AC-11/G2-2）；shutdown 通道唤醒挂起请求（AC-9）；
//! B-7 index 过大立即纠正；纯查询不触发（AC-10b）。

mod common;

use common::*;
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;
use velox_registry::registry::{now_epoch, Registry};
use velox_registry::store::SnapshotStore;

/// 缺省 index = 首次快照请求：三版本场景全部立即返回（G2-2/RQ-3）
#[tokio::test]
async fn default_index_immediate_three_scenarios() {
    // 场景 1：空表 version=0
    let dir = temp_dir();
    let cfg = config(dir.to_str().unwrap(), true, 30, 5);
    let (app, _store, _reg, _tx) = app_from_config(&cfg).unwrap();
    let (status, v, elapsed) =
        finish_within(spawn_request(&app, get("/api/v1/registry/watch")), 1).await;
    assert_eq!(status, 200);
    assert_eq!(v["index"], 0);
    assert_eq!(v["services"], json!({}));
    assert!(
        elapsed < Duration::from_millis(500),
        "缺省 index 空表立即返回，实际 {elapsed:?}"
    );
    cleanup(&dir);

    // 场景 2：恢复有实例 version=0（不走 HTTP 注册，直接 restored 构造）
    let now = now_epoch();
    let snapshot = json!({
        "format_version": 1, "saved_at_epoch": now,
        "services": {"svc": [{"id": uuid::Uuid::new_v4().to_string(), "address": "3.3.3.3",
            "port": 3, "metadata": {}, "last_heartbeat_epoch": now}]}
    });
    let dir = temp_dir();
    std::fs::create_dir_all(dir.join("registry")).unwrap();
    std::fs::write(
        dir.join("registry/registry-state.json"),
        snapshot.to_string(),
    )
    .unwrap();
    let cfg = config(dir.to_str().unwrap(), true, 30, 5);
    let (app, _store, _reg, _tx) = app_from_config(&cfg).unwrap();
    let (status, v, elapsed) =
        finish_within(spawn_request(&app, get("/api/v1/registry/watch")), 1).await;
    assert_eq!(status, 200);
    assert_eq!(v["index"], 0, "恢复不递增版本（重启归零语义）");
    assert_eq!(
        v["services"]["svc"].as_array().unwrap().len(),
        1,
        "恢复快照随缺省 index 立即返回"
    );
    assert!(elapsed < Duration::from_millis(500));
    cleanup(&dir);

    // 场景 3：version>0
    let dir = temp_dir();
    let cfg = config(dir.to_str().unwrap(), true, 30, 5);
    let (app, _store, _reg, _tx) = app_from_config(&cfg).unwrap();
    register_one(&app, "svc", "4.4.4.4", 4).await;
    let (status, v, elapsed) =
        finish_within(spawn_request(&app, get("/api/v1/registry/watch")), 1).await;
    assert_eq!(status, 200);
    assert_eq!(v["index"], 1);
    assert!(elapsed < Duration::from_millis(500));
    cleanup(&dir);
}

/// 显式 index=0 在 version=0 时按「相等」挂起（不与缺省混同）；
/// 注册唤醒：严格 <1s、index+1、快照含新实例（AC-8a/F-WATCH）。
#[tokio::test]
async fn explicit_zero_suspends_at_v0_and_wakes_on_register() {
    let dir = temp_dir();
    let cfg = config(dir.to_str().unwrap(), true, 30, 5);
    let (app, _store, _reg, _tx) = app_from_config(&cfg).unwrap();

    let mut handle = spawn_request(&app, get("/api/v1/registry/watch?index=0&timeout_secs=30"));
    still_pending(&mut handle, 0).await; // 300ms 内未完成 = 挂起（反例：显式 0 ≠ 缺省）

    let t0 = std::time::Instant::now();
    register_one(&app, "svc-x", "10.1.1.1", 1).await;
    let (status, v, elapsed) = finish_within(handle, 1).await;
    assert_eq!(status, 200);
    assert_eq!(v["index"], 1, "唤醒后 index = 旧值+1");
    assert_eq!(
        v["services"]["svc-x"].as_array().unwrap().len(),
        1,
        "快照含新实例"
    );
    // 保守判定：从变更请求发起前到完整响应 ≤1s（AC-8 冻结阈值，不放宽）
    let total = t0.elapsed();
    assert!(
        total < Duration::from_secs(1),
        "注册唤醒必须 <1s，实测 {total:?}"
    );
    let _ = elapsed;
    cleanup(&dir);
}

/// 显式 index=0 在 version>0 时立即返回（与挂起分支分离）
#[tokio::test]
async fn explicit_zero_immediate_when_version_positive() {
    let dir = temp_dir();
    let cfg = config(dir.to_str().unwrap(), true, 30, 5);
    let (app, _store, _reg, _tx) = app_from_config(&cfg).unwrap();
    register_one(&app, "svc", "4.4.4.4", 4).await;
    let (status, v, elapsed) = finish_within(
        spawn_request(&app, get("/api/v1/registry/watch?index=0")),
        1,
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(v["index"], 1);
    assert!(
        elapsed < Duration::from_millis(500),
        "显式 0 × v>0 立即返回，实际 {elapsed:?}"
    );
    cleanup(&dir);
}

/// 显式相等 index 挂起（反例保留）；超时返回当前快照 200、index 不变（TC-24）
#[tokio::test]
async fn explicit_equal_suspends_then_times_out_with_current_snapshot() {
    let dir = temp_dir();
    let cfg = config(dir.to_str().unwrap(), true, 30, 5);
    let (app, _store, _reg, _tx) = app_from_config(&cfg).unwrap();
    register_one(&app, "svc", "5.5.5.5", 5).await; // version=1

    let mut handle = spawn_request(&app, get("/api/v1/registry/watch?index=1&timeout_secs=1"));
    still_pending(&mut handle, 0).await; // 显式相等挂起（反例保留）
    let (status, v, elapsed) = finish_within(handle, 3).await;
    assert_eq!(status, 200, "超时返回 200（FR-8）");
    assert_eq!(v["index"], 1, "超时返回当前版本（无变更）");
    assert_eq!(v["services"]["svc"].as_array().unwrap().len(), 1);
    assert!(
        elapsed >= Duration::from_millis(900) && elapsed < Duration::from_millis(2500),
        "timeout_secs=1 应在 ~1s 返回（不提前、不等默认 30s），实际 {elapsed:?}"
    );
    cleanup(&dir);
}

/// index 落后立即返回（AC-8b）；index 过大（B-7）立即返回并以响应内 index 纠正
#[tokio::test]
async fn stale_and_oversized_index_return_immediately() {
    let dir = temp_dir();
    let cfg = config(dir.to_str().unwrap(), true, 30, 5);
    let (app, _store, _reg, _tx) = app_from_config(&cfg).unwrap();
    register_one(&app, "svc", "6.6.6.6", 6).await; // version=1

    for uri in [
        "/api/v1/registry/watch?index=0",
        "/api/v1/registry/watch?index=99",
    ] {
        let (status, v, elapsed) = finish_within(spawn_request(&app, get(uri)), 1).await;
        assert_eq!(status, 200, "{uri}");
        assert_eq!(v["index"], 1, "{uri}：以当前版本纠正客户端（B-7）");
        assert!(
            elapsed < Duration::from_millis(500),
            "{uri} 立即返回，实际 {elapsed:?}"
        );
    }
    cleanup(&dir);
}

/// deregister 唤醒且快照不含该实例（AC-10a）；纯查询不唤醒不递增（AC-10b）
#[tokio::test]
async fn deregister_wakes_queries_do_not() {
    let dir = temp_dir();
    let cfg = config(dir.to_str().unwrap(), true, 30, 5);
    let (app, _store, registry, _tx) = app_from_config(&cfg).unwrap();
    let id = register_one(&app, "svc", "7.7.7.7", 7).await; // version=1

    let mut handle = spawn_request(&app, get("/api/v1/registry/watch?index=1&timeout_secs=30"));
    still_pending(&mut handle, 0).await;

    // 挂起期间发起纯查询：不得唤醒、不得递增（AC-10b/FR-9）
    let (_, _) = json_of(&app, get("/healthz")).await;
    let (_, _) = json_of(&app, get("/api/v1/registry/services")).await;
    let (_, _) = json_of(
        &app,
        get("/api/v1/registry/services/svc/instances?healthy=true"),
    )
    .await;
    let (qs, _, _) = finish_within(
        spawn_request(&app, get("/api/v1/registry/watch?index=0")),
        1,
    )
    .await;
    assert_eq!(qs, 200, "查询类 watch（旧 index）自身立即返回");
    still_pending(&mut handle, 0).await; // 原挂起仍未被唤醒
    assert_eq!(registry.version(), 1, "纯查询后版本不变");

    // 注销唤醒：<1s、index+1、快照不含该实例
    let t0 = std::time::Instant::now();
    let (status, _) = json_of(
        &app,
        method(
            "DELETE",
            &format!("/api/v1/registry/services/svc/instances/{id}"),
        ),
    )
    .await;
    assert_eq!(status, 200);
    let (status, v, _) = finish_within(handle, 1).await;
    assert_eq!(status, 200);
    assert_eq!(v["index"], 2);
    assert!(v["services"].get("svc").is_none(), "快照不含已注销实例");
    assert!(
        t0.elapsed() < Duration::from_secs(1),
        "注销唤醒 <1s，实测 {t0:?}"
    );
    cleanup(&dir);
}

/// sweep 一批移除 k≥1 实例：批次整体 +1、唤醒且快照不含被剔除实例（AC-10/QA-2）
#[tokio::test]
async fn sweep_batch_bumps_once_and_wakes() {
    let dir = temp_dir();
    // TTL 200ms / flush 1s：flush=1 < ttl=0.2 不成立——领域层直接构造 Registry 绕开配置跨项校验
    let registry = Arc::new(Registry::new(Duration::from_millis(200)));
    let store = Arc::new(SnapshotStore::open(temp_dir()).unwrap());
    let (app, _tx) = app_with(Arc::clone(&registry), Some(Arc::clone(&store)));
    let _a = register_one(&app, "svc", "8.8.8.1", 1).await;
    let _b = register_one(&app, "svc", "8.8.8.2", 2).await;
    assert_eq!(registry.version(), 2);

    let mut handle = spawn_request(&app, get("/api/v1/registry/watch?index=2&timeout_secs=30"));
    still_pending(&mut handle, 0).await;

    std::thread::sleep(Duration::from_millis(300)); // A/B 全部过期
    let removed = registry.sweep_expired(); // 受控单次 sweep：整批移除
    assert_eq!(removed, 2);

    let (status, v, _) = finish_within(handle, 1).await;
    assert_eq!(status, 200);
    assert_eq!(v["index"], 3, "一次 sweep 批次整体 +1（QA-2：非逐实例 +2）");
    assert!(v["services"].get("svc").is_none(), "快照不含被剔除实例");

    // 空 sweep：无移除不递增
    assert_eq!(registry.sweep_expired(), 0);
    assert_eq!(registry.version(), 3);
    cleanup_all(&[dir, store.data_dir().to_path_buf()]);
}

/// 复活心跳唤醒（QA-3/TC-29）：healthy→unhealthy 不递增，复活心跳 +1 且 <1s；
/// 健康内续约不再唤醒。
#[tokio::test]
async fn revive_heartbeat_watches_exactly_once() {
    let dir = temp_dir();
    let registry = Arc::new(Registry::new(Duration::from_millis(200)));
    let store = Arc::new(SnapshotStore::open(temp_dir()).unwrap());
    let (app, _tx) = app_with(Arc::clone(&registry), Some(Arc::clone(&store)));
    let id = register_one(&app, "svc", "9.9.9.9", 9).await;
    assert_eq!(registry.version(), 1);

    std::thread::sleep(Duration::from_millis(300)); // 过期未剔除（无 sweep 任务）
    let (_, list) = json_of(&app, get("/api/v1/registry/services/svc/instances")).await;
    assert_eq!(list[0]["healthy"], false, "TTL 到点翻转 unhealthy");
    assert_eq!(
        registry.version(),
        1,
        "自然翻转不单独递增（FR-9.4 已知取舍）"
    );

    let mut handle = spawn_request(&app, get("/api/v1/registry/watch?index=1&timeout_secs=30"));
    still_pending(&mut handle, 0).await;

    // 迟到心跳复活：+1 并唤醒
    let t0 = std::time::Instant::now();
    let (status, _) = json_of(
        &app,
        method(
            "PUT",
            &format!("/api/v1/registry/services/svc/instances/{id}/heartbeat"),
        ),
    )
    .await;
    assert_eq!(status, 200);
    let (status, v, _) = finish_within(handle, 1).await;
    assert_eq!(status, 200);
    assert_eq!(v["index"], 2, "复活心跳 +1（QA-3）");
    assert_eq!(
        v["services"]["svc"][0]["healthy"], true,
        "唤醒快照含健康实例"
    );
    assert!(
        t0.elapsed() < Duration::from_secs(1),
        "复活唤醒 <1s，实测 {t0:?}"
    );

    // 复活后的健康续约：不唤醒
    let mut handle = spawn_request(&app, get("/api/v1/registry/watch?index=2&timeout_secs=30"));
    still_pending(&mut handle, 0).await;
    let (status, _) = json_of(
        &app,
        method(
            "PUT",
            &format!("/api/v1/registry/services/svc/instances/{id}/heartbeat"),
        ),
    )
    .await;
    assert_eq!(status, 200);
    still_pending(&mut handle, 0).await; // 300ms 后仍挂起：健康内续约不递增
    handle.abort();
    cleanup_all(&[dir, store.data_dir().to_path_buf()]);
}

/// shutdown 通道唤醒挂起请求（AC-9）：send(true) → <1s 返回当前快照 200
#[tokio::test]
async fn shutdown_wakes_suspended_watchers() {
    let dir = temp_dir();
    let cfg = config(dir.to_str().unwrap(), true, 30, 5);
    let (app, _store, _reg, shutdown_tx) = app_from_config(&cfg).unwrap();
    register_one(&app, "svc", "10.10.10.10", 10).await;

    let mut h1 = spawn_request(&app, get("/api/v1/registry/watch?index=1&timeout_secs=30"));
    let mut h2 = spawn_request(&app, get("/api/v1/registry/watch?index=1&timeout_secs=30"));
    still_pending(&mut h1, 0).await;
    still_pending(&mut h2, 0).await;

    shutdown_tx.send(true).unwrap();
    let (status, v, elapsed) = finish_within(h1, 1).await;
    assert_eq!(status, 200, "关停唤醒返回当前快照（非 5xx）");
    assert_eq!(v["index"], 1);
    assert!(
        elapsed < Duration::from_secs(1),
        "关停唤醒 <1s，实测 {elapsed:?}"
    );
    let (status, _, _) = finish_within(h2, 1).await;
    assert_eq!(status, 200);
    cleanup(&dir);
}

/// AC-11/TC-22/TC-40 集成：timeout 非法 400（含缺省 index）；解析失败统一
/// `invalid_parameter` JSON（abc/负数/溢出/空值）
#[tokio::test]
async fn param_errors_all_400_invalid_parameter() {
    let dir = temp_dir();
    let cfg = config(dir.to_str().unwrap(), true, 30, 5);
    let (app, _store, _reg, _tx) = app_from_config(&cfg).unwrap();

    // 范围校验：0 / 301（含缺省 index 请求——先校验后分支，TC-22）
    for uri in [
        "/api/v1/registry/watch?timeout_secs=0",
        "/api/v1/registry/watch?timeout_secs=301",
        "/api/v1/registry/watch?index=5&timeout_secs=0",
        "/api/v1/registry/watch?timeout_secs=0&index=5",
    ] {
        let (status, v) = json_of(&app, get(uri)).await;
        assert_eq!(status, 400, "{uri}");
        assert_eq!(v["code"], "invalid_parameter", "{uri}");
        assert!(
            v["message"]
                .as_str()
                .unwrap_or_default()
                .contains("timeout_secs"),
            "{uri}"
        );
    }

    // 解析失败：非数字 / 负数 / u64 溢出 / 空值（TC-40）
    for uri in [
        "/api/v1/registry/watch?index=abc",
        "/api/v1/registry/watch?index=-1",
        "/api/v1/registry/watch?index=18446744073709551616", // u64::MAX + 1
        "/api/v1/registry/watch?index=",
        "/api/v1/registry/watch?timeout_secs=abc",
        "/api/v1/registry/watch?timeout_secs=-1",
        "/api/v1/registry/watch?timeout_secs=",
    ] {
        let (status, v) = json_of(&app, get(uri)).await;
        assert_eq!(status, 400, "{uri}");
        assert_eq!(v["code"], "invalid_parameter", "{uri}");
        assert!(
            !v["message"].as_str().unwrap_or_default().is_empty(),
            "{uri}"
        );
    }
    cleanup(&dir);
}

/// watch 响应结构：与列表端点同构（InstanceView 复用），仅健康实例（TC-41 顺带覆盖）
#[tokio::test]
async fn watch_response_shape_matches_list_endpoint() {
    let dir = temp_dir();
    let cfg = config(dir.to_str().unwrap(), true, 30, 5);
    let (app, _store, _reg, _tx) = app_from_config(&cfg).unwrap();
    register_one(&app, "svc", "11.11.11.11", 11).await;

    let (_, watch_v) = json_of(&app, get("/api/v1/registry/watch")).await;
    let (_, list_v) = json_of(
        &app,
        get("/api/v1/registry/services/svc/instances?healthy=true"),
    )
    .await;
    let w = &watch_v["services"]["svc"][0];
    let l = &list_v[0];
    for field in [
        "id",
        "service",
        "address",
        "port",
        "metadata",
        "seconds_since_heartbeat",
        "healthy",
    ] {
        assert_eq!(w[field], l[field], "字段 {field} 与列表端点同构");
    }
    assert_eq!(w["healthy"], true);
    cleanup(&dir);
}

fn cleanup(dir: &std::path::Path) {
    std::fs::remove_dir_all(dir).ok();
}

fn cleanup_all(dirs: &[std::path::PathBuf]) {
    for d in dirs {
        std::fs::remove_dir_all(d).ok();
    }
}
