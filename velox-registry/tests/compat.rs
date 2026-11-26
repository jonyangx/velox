//! v0.1 兼容回归（架构 §6.2 compat.rs，AC-12/TC-35）：
//! 六端点路径/方法/JSON/状态码/错误 code 全部不变；持久化开启下模拟 mesh
//! 拉取序列（list services → instances?healthy=true）照常工作。

mod common;

use common::*;
use serde_json::json;
use tower::ServiceExt;
use uuid::Uuid;

/// TC-35：v0.1 六端点全签名回归（setup 适配 AppState；原断言语义保留）
#[tokio::test]
async fn v01_endpoint_contracts_unchanged() {
    let dir = temp_dir();
    let cfg = config(dir.to_str().unwrap(), true, 30, 5);
    let (app, _store, _reg, _tx) = app_from_config(&cfg).unwrap();

    // 1. healthz（结构不变）
    let (status, v) = json_of(&app, get("/healthz")).await;
    assert_eq!(status, 200);
    assert_eq!(v["status"], "ok");
    assert_eq!(v["service"], "velox-registry");

    // 2. 注册：200 + 返回完整 InstanceView JSON（id/address/port/metadata/...）
    let (status, v) = json_of(
        &app,
        post_json(
            "/api/v1/registry/services/svc-a/instances",
            json!({"address": "10.0.0.1", "port": 9000, "metadata": {"version": "0.1.0"}}),
        ),
    )
    .await;
    assert_eq!(status, 200);
    let id_a = v["id"].as_str().unwrap().to_string();
    assert_eq!(v["address"], "10.0.0.1");
    assert_eq!(v["port"], 9000);
    assert_eq!(v["metadata"]["version"], "0.1.0");
    assert_eq!(v["healthy"], true);

    // metadata 缺省（serde default）仍合法（v0.1 行为）
    let (status, v) = json_of(
        &app,
        post_json(
            "/api/v1/registry/services/svc-b/instances",
            json!({"address": "10.0.0.2", "port": 9001}),
        ),
    )
    .await;
    assert_eq!(status, 200);
    let id_b = v["id"].as_str().unwrap().to_string();
    assert_eq!(v["metadata"], json!({}));

    // 3. 心跳：200 {"ok":true}
    let (status, v) = json_of(
        &app,
        method(
            "PUT",
            &format!("/api/v1/registry/services/svc-a/instances/{id_a}/heartbeat"),
        ),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(v, json!({"ok": true}));

    // 4. 服务列表（排序）
    let (status, v) = json_of(&app, get("/api/v1/registry/services")).await;
    assert_eq!(status, 200);
    assert_eq!(v, json!(["svc-a", "svc-b"]));

    // 5. 实例列表：全量 + ?healthy=true 过滤
    let (status, v) = json_of(&app, get("/api/v1/registry/services/svc-a/instances")).await;
    assert_eq!(status, 200);
    assert_eq!(v.as_array().unwrap().len(), 1);
    let (status, v) = json_of(
        &app,
        get("/api/v1/registry/services/svc-a/instances?healthy=true"),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(v.as_array().unwrap().len(), 1, "健康实例在列");

    // 6. 错误语义：未知服务 / 未知实例 → 404 + 统一错误结构（v0.1 code 不变）
    let (status, v) = json_of(&app, get("/api/v1/registry/services/nope/instances")).await;
    assert_eq!(status, 404);
    assert_eq!(v["code"], "service_not_found");
    let (status, v) = json_of(
        &app,
        method(
            "PUT",
            &format!(
                "/api/v1/registry/services/svc-a/instances/{}/heartbeat",
                Uuid::new_v4()
            ),
        ),
    )
    .await;
    assert_eq!(status, 404);
    assert_eq!(v["code"], "instance_not_found");
    let (status, v) = json_of(
        &app,
        method(
            "DELETE",
            &format!(
                "/api/v1/registry/services/nope/instances/{}",
                Uuid::new_v4()
            ),
        ),
    )
    .await;
    assert_eq!(status, 404);
    assert_eq!(v["code"], "service_not_found");

    // 7. 注销：200 {"ok":true}；最后一个实例移除后服务条目清理（v0.1 行为）
    let (status, v) = json_of(
        &app,
        method(
            "DELETE",
            &format!("/api/v1/registry/services/svc-b/instances/{id_b}"),
        ),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(v, json!({"ok": true}));
    let (status, v) = json_of(&app, get("/api/v1/registry/services")).await;
    assert_eq!(status, 200);
    assert_eq!(v, json!(["svc-a"]));

    // 8. watch 新端点对旧路由零影响：POST 到 watch → 405（方法不支持），旧端点不受新路由干扰
    let res = app
        .clone()
        .oneshot(method("POST", "/api/v1/registry/watch"))
        .await
        .unwrap();
    assert_eq!(res.status().as_u16(), 405, "TC-40：watch 仅接受 GET");

    cleanup(&dir);
}

/// AC-12 后半：模拟 mesh 拉取序列（list services → instances?healthy=true）
/// 在持久化开启下照常工作（discovery.rs 消费形态：服务名数组 + 实例数组）
#[tokio::test]
async fn mesh_pull_sequence_with_persistence_enabled() {
    let dir = temp_dir();
    let cfg = config(dir.to_str().unwrap(), true, 30, 5);
    let (app, store, _reg, _tx) = app_from_config(&cfg).unwrap();

    // 注册两个服务各一实例（写穿落盘）
    let (status, _) = json_of(
        &app,
        post_json(
            "/api/v1/registry/services/provider-a/instances",
            json!({"address": "172.17.0.2", "port": 9000}),
        ),
    )
    .await;
    assert_eq!(status, 200);
    let (status, _) = json_of(
        &app,
        post_json(
            "/api/v1/registry/services/consumer-a/instances",
            json!({"address": "172.17.0.3", "port": 9100}),
        ),
    )
    .await;
    assert_eq!(status, 200);

    // mesh 周期拉取序列（discovery.rs：GET services → GET instances?healthy=true）
    let (status, services) = json_of(&app, get("/api/v1/registry/services")).await;
    assert_eq!(status, 200);
    for svc in services.as_array().unwrap() {
        let svc = svc.as_str().unwrap();
        let (status, instances) = json_of(
            &app,
            get(&format!(
                "/api/v1/registry/services/{svc}/instances?healthy=true"
            )),
        )
        .await;
        assert_eq!(status, 200);
        assert_eq!(
            instances.as_array().unwrap().len(),
            1,
            "{svc} 健康实例可拉取"
        );
    }

    // 落盘已生效：重启（重 open）后 mesh 拉取序列继续工作
    drop(app);
    let cfg = config(dir.to_str().unwrap(), true, 30, 5);
    let (app, _store2, _reg, _tx) = app_from_config(&cfg).unwrap();
    assert!(store.is_some());
    let (status, services) = json_of(&app, get("/api/v1/registry/services")).await;
    assert_eq!(status, 200);
    assert_eq!(
        services,
        json!(["consumer-a", "provider-a"]),
        "重启后恢复，mesh 拉取不中断"
    );
    let (status, instances) = json_of(
        &app,
        get("/api/v1/registry/services/provider-a/instances?healthy=true"),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(instances[0]["address"], "172.17.0.2");
    cleanup(&dir);
}

fn cleanup(dir: &std::path::Path) {
    std::fs::remove_dir_all(dir).ok();
}
