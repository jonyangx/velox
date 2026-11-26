//! 持久化集成测试（架构 §6.2 persistence.rs）：
//! 重启往返（AC-1/AC-2，恢复窗口前提显式构造）、超 TTL 丢弃（AC-4）、
//! 损坏跳过（AC-6）、禁用零落盘（AC-5，含禁用兼容分支）、写穿失败内存优先 +
//! 撤销后无新 mutation 自动重试（AC-7/G2-1/TC-05b/17）、周期合并刷盘（AC-3）、
//! dropped 延迟清理（G2-4）、递归建目录（TC-14）与原子写路径（AC-14）。

mod common;

use common::*;
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;
use velox_registry::registry::{now_epoch, Registry};
use velox_registry::store::{LoadOutcome, SnapshotStore};

/// TC-04/TC-03/AC-1/AC-2：注册即落盘；drop → 同目录重 open → 恢复字段一致；
/// 恢复窗口前提显式构造（恢复时落盘心跳 age ≤ 本次 TTL）；原 id 心跳 200。
#[tokio::test]
async fn restart_recovers_instances_with_same_id() {
    let dir = temp_dir();
    let cfg = config(dir.to_str().unwrap(), true, 60, 5);
    let (app, store, _registry, _tx) = app_from_config(&cfg).unwrap();

    // 注册：写穿——响应完成即文件已含该实例（TC-04）
    let (status, body) = json_of(
        &app,
        post_json(
            "/api/v1/registry/services/svc-a/instances",
            json!({"address": "10.0.0.1", "port": 9000, "metadata": {"k": "中文\"引号\"\n换行"}}),
        ),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    let id = body["id"].as_str().unwrap().to_string();

    // 落盘文件字段完整性（FR-1：绝对时间戳）
    let path = store.as_ref().unwrap().snapshot_path();
    let raw = std::fs::read(&path).unwrap();
    let file: serde_json::Value = serde_json::from_slice(&raw).unwrap();
    assert_eq!(file["format_version"], 1);
    assert!(file["saved_at_epoch"].is_u64());
    let inst = &file["services"]["svc-a"][0];
    assert_eq!(inst["id"].as_str().unwrap(), id);
    assert_eq!(inst["address"], "10.0.0.1");
    assert_eq!(inst["port"], 9000);
    assert!(inst["last_heartbeat_epoch"].is_u64());

    // 模拟重启：丢弃内存态，同目录重新 open + load + restored（真实启动路径）
    let persisted_epoch = inst["last_heartbeat_epoch"].as_u64().unwrap();
    drop(app);
    drop(store);
    let (app, store, _reg, _tx) = app_from_config(&cfg).unwrap();
    let now = now_epoch();
    let age = now.saturating_sub(persisted_epoch);
    assert!(
        age <= 60,
        "恢复窗口前提：落盘心跳 age={age}s ≤ 本次 TTL=60s"
    );

    let (status, list) = json_of(&app, get("/api/v1/registry/services/svc-a/instances")).await;
    assert_eq!(status, 200);
    assert_eq!(list.as_array().unwrap().len(), 1, "重启后实例恢复");
    let restored = &list[0];
    assert_eq!(
        restored["id"].as_str().unwrap(),
        id,
        "instance_id 保持不变（FR-3）"
    );
    assert_eq!(restored["address"], "10.0.0.1");
    assert_eq!(restored["port"], 9000);
    assert_eq!(
        restored["metadata"]["k"], "中文\"引号\"\n换行",
        "B-12 内容完整恢复"
    );

    // AC-2：恢复后原实例 id 心跳成功（恢复窗口内）
    let (status, body) = json_of(
        &app,
        method(
            "PUT",
            &format!("/api/v1/registry/services/svc-a/instances/{id}/heartbeat"),
        ),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(body["ok"], true);
    assert!(!path.to_string_lossy().contains("registry-state.json.tmp"));
    let _ = store.as_ref().unwrap().snapshot_path();
    cleanup(&dir);
}

/// TC-07/AC-4：落盘心跳距 now 超 TTL 的实例不恢复；严格 age > ttl 由单测边界覆盖，
/// 此处集成验证「陈旧实例不在任何列表」。
#[tokio::test]
async fn restore_drops_instances_beyond_ttl() {
    let dir = temp_dir();
    let now = now_epoch();
    let fresh_id = uuid::Uuid::new_v4();
    let stale_id = uuid::Uuid::new_v4();
    let snapshot = json!({
        "format_version": 1,
        "saved_at_epoch": now,
        "services": {
            "svc": [
                {"id": fresh_id.to_string(), "address": "10.0.0.1", "port": 1,
                 "metadata": {}, "last_heartbeat_epoch": now - 3},
                {"id": stale_id.to_string(), "address": "10.0.0.2", "port": 2,
                 "metadata": {}, "last_heartbeat_epoch": now - 60}
            ]
        }
    });
    std::fs::create_dir_all(dir.join("registry")).unwrap();
    std::fs::write(
        dir.join("registry/registry-state.json"),
        snapshot.to_string(),
    )
    .unwrap();

    let cfg = config(dir.to_str().unwrap(), true, 30, 5);
    let (app, _store, _reg, _tx) = app_from_config(&cfg).unwrap();
    let (_, services) = json_of(&app, get("/api/v1/registry/services")).await;
    assert_eq!(services, json!(["svc"]));
    let (_, list) = json_of(&app, get("/api/v1/registry/services/svc/instances")).await;
    let ids: Vec<&str> = list
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["id"].as_str().unwrap())
        .collect();
    assert_eq!(
        ids,
        vec![fresh_id.to_string().as_str()],
        "仅 age≤TTL 的实例恢复"
    );
    // 陈旧实例 id 心跳 → 404 instance_not_found（AC-4 的行为面：不恢复即不可续约）
    let (status, err) = json_of(
        &app,
        method(
            "PUT",
            &format!("/api/v1/registry/services/svc/instances/{stale_id}/heartbeat"),
        ),
    )
    .await;
    assert_eq!(status, 404);
    assert_eq!(err["code"], "instance_not_found");
    cleanup(&dir);
}

/// TC-13/AC-6：损坏夹具逐场景整文件跳过 + 空表接客（healthz 200）。
#[tokio::test]
async fn corrupted_snapshot_skipped_and_serves() {
    let fixtures: Vec<(&str, serde_json::Value)> = vec![
        ("截断 JSON", json!("This is not valid JSON at all")),
        ("缺字段", json!({"format_version": 1, "saved_at_epoch": 1})),
        (
            "未知版本",
            json!({"format_version": 99, "saved_at_epoch": 1, "services": {}}),
        ),
        (
            "字段类型错",
            json!({"format_version": 1, "saved_at_epoch": "x", "services": {}}),
        ),
    ];
    for (desc, bad) in fixtures {
        let dir = temp_dir();
        std::fs::create_dir_all(dir.join("registry")).unwrap();
        std::fs::write(dir.join("registry/registry-state.json"), bad.to_string()).unwrap();
        let cfg = config(dir.to_str().unwrap(), true, 30, 5);
        let boot = velox_registry::init_persistence(&cfg).unwrap();
        assert_eq!(boot.skipped, 1, "{desc}：skipped=1");
        assert_eq!(boot.restored_instances, 0, "{desc}");
        let (app, _tx) = app_with(boot.registry, boot.store);
        let (status, health) = json_of(&app, get("/healthz")).await;
        assert_eq!(status, 200, "{desc}");
        assert_eq!(health["status"], "ok");
        let (_, services) = json_of(&app, get("/api/v1/registry/services")).await;
        assert_eq!(services, json!([]), "{desc}：按空注册表启动");
        cleanup(&dir);
    }
}

/// TC-10/AC-5：enabled=false 不创建/不写/不恢复，已有文件内容不变，行为与 v0.1 一致。
#[tokio::test]
async fn disabled_persists_nothing() {
    let dir = temp_dir();
    // 预置一份有效快照（含 1 个存活实例）
    let now = now_epoch();
    let snapshot = json!({
        "format_version": 1, "saved_at_epoch": now,
        "services": {"svc": [{"id": uuid::Uuid::new_v4().to_string(), "address": "9.9.9.9",
        "port": 1, "metadata": {}, "last_heartbeat_epoch": now}]}
    });
    std::fs::create_dir_all(dir.join("registry")).unwrap();
    let path = dir.join("registry/registry-state.json");
    std::fs::write(&path, snapshot.to_string()).unwrap();
    let before = std::fs::read(&path).unwrap();

    // 禁用启动：不调用 load（不恢复），store=None
    let cfg = config(dir.to_str().unwrap(), false, 1, 5); // TTL1 < 默认 flush5：禁用态合法（G2-3 规则 4）
    let (app, store, _reg, _tx) = app_from_config(&cfg).unwrap();
    assert!(store.is_none(), "禁用时 store 必须为 None");

    // 注册（内存可查）——结构变更挂接对 None 直接 Ok，不落盘
    let id = register_one(&app, "svc", "8.8.8.8", 1).await;
    let (_, list) = json_of(&app, get("/api/v1/registry/services/svc/instances")).await;
    let arr = list.as_array().unwrap();
    assert_eq!(
        arr.len(),
        1,
        "禁用时不恢复已有快照（不调用 load），内存仅新注册实例"
    );
    assert_ne!(arr[0]["id"].as_str().unwrap(), "", "新实例可查");
    let _ = id;
    let (status, _) = json_of(
        &app,
        method(
            "PUT",
            &format!("/api/v1/registry/services/svc/instances/{id}/heartbeat"),
        ),
    )
    .await;
    assert_eq!(status, 200);

    // 文件内容不变、无 tmp 产生
    let after = std::fs::read(&path).unwrap();
    assert_eq!(before, after, "禁用时已有快照文件不得被改写");
    assert!(!dir.join("registry/registry-state.json.tmp").exists());
    // 心跳已置 dirty 但无 flusher/store —— 内存行为与 v0.1 完全一致
    assert!(store.is_none());
    cleanup(&dir);
}

/// TC-16/TC-17/G2-1/AC-7：写穿失败 → 500 persist_failed（响应不泄露细节）、
/// 内存优先可查；撤销注入后**无新 mutation**，模拟周期任务重试成功追平（含结构失败）。
#[tokio::test]
async fn write_failure_memory_first_then_retry_succeeds() {
    let dir = temp_dir();
    let cfg = config(dir.to_str().unwrap(), true, 60, 1);
    let (app, store, registry, _tx) = app_from_config(&cfg).unwrap();
    let dir_path = store
        .as_ref()
        .unwrap()
        .snapshot_path()
        .parent()
        .unwrap()
        .to_path_buf();

    // 注入确定性写失败：tmp 路径变成目录（File::create 必失败）
    let tmp = dir_path.join("registry-state.json.tmp");
    std::fs::create_dir(&tmp).unwrap();

    // 注册：500 persist_failed（精确 code/message），实例仍可查（内存优先）
    let (status, err) = json_of(
        &app,
        post_json(
            "/api/v1/registry/services/svc/instances",
            json!({"address": "7.7.7.7", "port": 1}),
        ),
    )
    .await;
    assert_eq!(status, 500);
    assert_eq!(err["code"], "persist_failed");
    assert_eq!(err["message"], "registry state persist failed");
    let (_, list) = json_of(&app, get("/api/v1/registry/services/svc/instances")).await;
    assert_eq!(list.as_array().unwrap().len(), 1, "内存注册成功（FR-7）");

    // 注销（另一结构路径）同样失败且内存已移除
    let (status, err) = json_of(
        &app,
        method(
            "DELETE",
            &format!(
                "/api/v1/registry/services/svc/instances/{}",
                list[0]["id"].as_str().unwrap()
            ),
        ),
    )
    .await;
    assert_eq!(status, 500);
    assert_eq!(err["code"], "persist_failed");
    // 注销移除最后一个实例后服务条目随之清理：再查列表为 404 service_not_found
    // （v0.1 语义不变，同时证明内存已移除）
    let (status, err) = json_of(&app, get("/api/v1/registry/services/svc/instances")).await;
    assert_eq!(status, 404, "注销内存已生效");
    assert_eq!(err["code"], "service_not_found");
    let (_, services) = json_of(&app, get("/api/v1/registry/services")).await;
    assert_eq!(services, json!([]), "全空表（TC-17 场景准备）");

    // 撤销注入；**无任何新 mutation**：失败置回的 dirty 由周期任务重试追平
    std::fs::remove_dir(&tmp).unwrap();
    let dirty = registry.take_dirty();
    assert!(
        dirty,
        "写穿失败置回 dirty（G2-1），撤销后无新 mutation 也必须可重试"
    );
    store
        .as_ref()
        .unwrap()
        .flush(|| registry.snapshot())
        .await
        .unwrap();
    let outcome = store.as_ref().unwrap().load();
    assert!(matches!(outcome, LoadOutcome::Restored(_)));
    let raw = std::fs::read(dir_path.join("registry-state.json")).unwrap();
    let file: serde_json::Value = serde_json::from_slice(&raw).unwrap();
    assert_eq!(
        file["services"],
        json!({}),
        "空表状态最终落盘，不留旧实例（TC-17）"
    );

    // 服务继续可用（注册恢复写穿）
    register_one(&app, "svc", "7.7.7.8", 2).await;
    cleanup(&dir);
}

/// TC-05/AC-3：心跳响应不等落盘、不写穿；周期合并刷盘推进文件；TC-05c：
/// 成功 flush 不得清掉快照之后的新 dirty。
#[tokio::test]
async fn heartbeat_flush_is_periodic_and_not_blocking() {
    let dir = temp_dir();
    let store = Arc::new(SnapshotStore::open(&dir).unwrap());
    let registry = Arc::new(Registry::new(Duration::from_secs(60)));
    let (app, _tx) = app_with(Arc::clone(&registry), Some(Arc::clone(&store)));
    let id = register_one(&app, "svc", "6.6.6.6", 1).await;
    let path = store.snapshot_path();
    let file_epoch = |p: &std::path::Path| -> u64 {
        let v: serde_json::Value = serde_json::from_slice(&std::fs::read(p).unwrap()).unwrap();
        v["services"]["svc"][0]["last_heartbeat_epoch"]
            .as_u64()
            .unwrap()
    };
    let e1 = file_epoch(&path);

    // 心跳：响应不 await 落盘（无 flush 路径），且不写穿——文件 epoch 不变
    let start = std::time::Instant::now();
    let (status, _) = json_of(
        &app,
        method(
            "PUT",
            &format!("/api/v1/registry/services/svc/instances/{id}/heartbeat"),
        ),
    )
    .await;
    assert_eq!(status, 200);
    assert!(
        start.elapsed() < Duration::from_millis(500),
        "心跳响应不等落盘"
    );
    assert_eq!(file_epoch(&path), e1, "心跳不写穿（合并刷盘策略）");

    // 周期合并：跨 epoch 秒后心跳，文件在下一次 flush 时推进
    std::thread::sleep(Duration::from_millis(1100));
    let (status, _) = json_of(
        &app,
        method(
            "PUT",
            &format!("/api/v1/registry/services/svc/instances/{id}/heartbeat"),
        ),
    )
    .await;
    assert_eq!(status, 200);
    // 模拟 flusher 一个 tick：take_dirty → flush
    assert!(registry.take_dirty(), "心跳必须置 dirty");
    store.flush(|| registry.snapshot()).await.unwrap();
    let e2 = file_epoch(&path);
    assert!(
        e2 > e1,
        "周期刷盘后文件心跳推进（跨 epoch 秒：{e1} → {e2}）"
    );

    // TC-05c：take（flusher 取走）→ 锁内取快照 → 快照之后并发 mutation 置 dirty →
    // 成功 flush（旧快照）→ 新 dirty 必须仍在
    let (status, _) = json_of(
        &app,
        method(
            "PUT",
            &format!("/api/v1/registry/services/svc/instances/{id}/heartbeat"),
        ),
    )
    .await;
    assert_eq!(status, 200);
    assert!(registry.take_dirty(), "flusher tick 取走本周期脏标记");
    let stale_snapshot = registry.snapshot(); // flush_lock 内取快照
    std::thread::sleep(Duration::from_millis(1100));
    let (status, _) = json_of(
        &app,
        method(
            "PUT",
            &format!("/api/v1/registry/services/svc/instances/{id}/heartbeat"),
        ),
    )
    .await;
    assert_eq!(status, 200, "快照读取后的并发心跳置 dirty");
    store.flush(|| stale_snapshot).await.unwrap(); // 成功落盘（旧快照）
    assert!(
        registry.take_dirty(),
        "成功 flush 不得清除快照之后的新 dirty（下周期重试）"
    );
    store.flush(|| registry.snapshot()).await.unwrap();
    assert!(file_epoch(&path) > e2, "重试后文件追上最新心跳");
    cleanup(&dir);
}

/// G2-4/§9 非阻塞③：恢复丢弃超 TTL 实例 → 置 dirty，一个周期内文件被改写为过滤后状态。
#[tokio::test]
async fn restore_dropped_lazy_cleanup() {
    let dir = temp_dir();
    let now = now_epoch();
    let snapshot = json!({
        "format_version": 1, "saved_at_epoch": now,
        "services": {"svc": [
            {"id": uuid::Uuid::new_v4().to_string(), "address": "1.1.1.1", "port": 1,
             "metadata": {}, "last_heartbeat_epoch": now},
            {"id": uuid::Uuid::new_v4().to_string(), "address": "2.2.2.2", "port": 2,
             "metadata": {}, "last_heartbeat_epoch": now - 100}
        ]}
    });
    std::fs::create_dir_all(dir.join("registry")).unwrap();
    std::fs::write(
        dir.join("registry/registry-state.json"),
        snapshot.to_string(),
    )
    .unwrap();

    let cfg = config(dir.to_str().unwrap(), true, 30, 5);
    let (_app, store, registry, _tx) = app_from_config(&cfg).unwrap();
    assert!(
        registry.take_dirty(),
        "dropped_expired>0 必须置 dirty（延迟清理）"
    );
    store
        .as_ref()
        .unwrap()
        .flush(|| registry.snapshot())
        .await
        .unwrap();
    let v: serde_json::Value =
        serde_json::from_slice(&std::fs::read(dir.join("registry/registry-state.json")).unwrap())
            .unwrap();
    assert_eq!(
        v["services"]["svc"].as_array().unwrap().len(),
        1,
        "文件已改写为过滤后状态"
    );
    cleanup(&dir);
}

/// TC-14/TC-11：data_dir 及子目录均不存在 → 递归创建；快照仅在专属子目录。
#[tokio::test]
async fn recursive_dir_creation_isolated_layout() {
    let dir = temp_dir().join("a").join("b"); // 两级父目录均不存在
    let cfg = config(dir.to_str().unwrap(), true, 30, 5);
    let (app, store, _reg, _tx) = app_from_config(&cfg).unwrap();
    register_one(&app, "svc", "5.5.5.5", 1).await;
    let path = store.as_ref().unwrap().snapshot_path();
    assert!(path.is_file(), "快照文件已创建");
    assert!(
        path.ends_with("registry/registry-state.json"),
        "专属子目录布局：{path:?}"
    );
    assert!(
        !dir.join("registry-state.json").exists(),
        "顶层不得有快照（G2-5 隔离）"
    );
    cleanup(&dir);
}

fn cleanup(dir: &std::path::Path) {
    std::fs::remove_dir_all(dir).ok();
}
