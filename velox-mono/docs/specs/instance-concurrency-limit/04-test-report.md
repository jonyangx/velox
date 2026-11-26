# 测试报告：velox-config 单实例最大并发请求限流

> Feature slug：`instance-concurrency-limit` | 工序：F-TA-E（MODE=EXECUTE，常规执行轮）| 日期：2026-11-26
> **覆盖提交：`4b24a20`**（`fix(config): 并发上限技术上界无损校验`；当时仓库 HEAD=`4b24a20`，已包含 `a5b75e5` addlog 接入、`90268ca` 既有实现、`dff2006` 既有单测）
> **输入基准**：`01-requirements.md`（冻结 SHA `abcf44b1…`）、`02-architecture.md`（定稿 SHA `27cba700…`）、`03-test-cases.md`（v1 定稿 SHA `e48455f6…`，21TC/7AC）、`02-architecture-review.md`（GATE2 PASS）
> **测试基线声明**：当前工作树存在既有在途改动——`M velox-config/src/limit.rs`（经 `git diff` 逐行复核为 2 处纯 rustfmt 换行，无行为变化，DA 按 P3-B1 保留未提交）及未跟踪规格/状态文件（`_pipeline-state.md`、F2 验收/测试报告、`velox-mono/docs/brd/`），全部原样保留未提交；本轮所有静态门槛与测试均在此基线上运行，全部运行期临时数据位于系统临时目录且已清理，未向仓库写入运行数据。

---

## 1. 测试结论

**通过**

- **冻结 AC 达成度**：7/7 条冻结 AC（AC-1～AC-7）与 Q1～Q6 / G1-1～G1-5 决议全部获实跑验证通过。
- **核心修复验证（4b24a20）**：`config.rs` 的 `MAX_CONCURRENT_REQUESTS_CAP`（64 位 `U = 2305843009213693951`）u128 无损上界校验、超界普通可读错误（含字段名/原始值/`U=<数值>`）、真实进程 fail-fast（非 0 退出、无 panic、监听与日志/存储初始化前终止）、空 env 不覆盖文件、文件非法+合法 env 组合优先级、405 边界实跑留痕及 addlog 默认 `warn` 下日志观察前提均实测通过。
- **缺陷与阻塞**：本 feature 新增产品缺陷 **0 项**、测试资产缺陷 **0 项**、阻塞项 **0 项**；workspace 全量回归仅复现 1 项已知跨包既有 flaky（`demo_consumer::config::tests::defaults_match_contract`，隔离 11/11 通过，非本 feature 引入）。

---

## 2. 档位执行说明（T0~T3 触发判定与耗时）

| 档位 | 执行状态 | 触发条件判定理由 | 执行内容与结果 | 实际耗时 |
|---|---|---|---|---|
| **T0 基础** | **已执行** | 必跑政策；验证 `velox-config` 既有与新增 7 个边界/优先级单测 | `cargo test -p velox-config`：**23/23 全绿**；`cargo test -p velox-config -- --test-threads=8` ×3 轮：**23/23 ×3 全绿**（ENV_LOCK 并发稳定） | < 1s |
| **T1 冒烟** | **已执行** | **触发成立**：4b24a20 变更直接涉及 `config.rs` 配置加载与启动校验装配，且 TC-10/12/13 要求真实二进制进程级验证退出码、错误通道、监听前终止与日志观察前提 | 启动真实 `target/debug/velox-config`（隔离端口 18251～18278、隔离 `/tmp` data/logs、清空残留 `RUST_LOG`/`VELOX_CONFIG__*`）：①16 项非法值（文件+env 路径含 `0/-1/1.5/abc/U+1/i64::MAX`/23 位溢出整数/非法 TOML/文件5+env0）全部 `rc=1`、无 panic、监听前终止；②4 项合法边界（`1/1000/100000/U`）全部启动健康检查 200 并 SIGINT `rc=0`；③9 组配置来源（7 种标准来源+空 env 变体+文件0+env3 组合）在显式 `RUST_LOG=info` 与默认 `warn` 双模式下全部通过 | ~6s |
| **T2 全量回归** | **已执行** | **触发成立**：任务显式要求 workspace 全量回归 + 静态门槛 + `acceptance.sh`，且本 feature 与 `a5b75e5`（addlog）共享 `velox-config` 启动链路需核验跨 feature 兼容 | `cargo fmt --all --check`、`cargo clippy --workspace --all-targets -- -D warnings`、`cargo build --workspace` 三项全绿；`cargo test --workspace --no-fail-fast`（共 130 例）：除已知 `demo_consumer::config::tests::defaults_match_contract` flaky 外 **129/129 全绿**（`demo-consumer --test-threads=1` 隔离 **11/11 全绿**）；`bash velox-demo/scripts/acceptance.sh`（零修改运行）**PASS**（步骤 5/5 全部断言通过）；TC-20 文档/README/TOML 手工逐行核对通过 | ~33s（静态+WS 5s，acceptance 27.2s） |
| **T3 e2e** | **已执行** | **触发成立**：P0 用例 TC-15 要求真实 TCP/HTTP 活体链路验证立即 429、错误体、每拒 WARN、零副作用；TC-17 需真二进制核验 axum 0.8.9 405 语义；TC-19 需真实 SIGINT 优雅关停 | 以真实 `target/debug/velox-config` + TCP 半包 PUT 占名额夹具（夹具 E，隔离端口与目录）实跑 7 组活体场景（C1～C7）：饱和立即 429（1.3～2.6ms）、错误契约、零副作用、默认 warn 每拒一 WARN（精确 3/3）与 error 过滤预期、跨 5 条命中业务路由全局共享、`/healthz` 豁免（连续 5 次 200）、404/405 未饱和与饱和对照、200/400/404/500 响应释放、客户端断开取消释放、N=2 10 线程 barrier 3 轮并发竞态（`[(2,8),(2,8),(2,8)]` 零超卖）、SIGINT 在途请求完成并 `rc=0` 优雅退出——**全部通过** | ~6s |

> 说明：本 feature 需求无 TC-38 类性能比值专项（NFR-1 为 O(1) `try_acquire_owned` 非等待入口，架构结构保证且本次实测 429 响应延迟 1.3～2.6ms）；T0～T3 均已针对最终代码提交 `4b24a20` 实跑完成，无跳档、无降级。

---

## 3. 用例执行结果（对照 03-test-cases.md 21TC / 7AC 矩阵）

| TC 编号 | 功能点 / 对应 AC | 执行层级 | 结果 | 实测证据摘录 |
|---|---|---|---|---|
| TC-01 | 边界 / AC-1 | T0 + T3 | **通过** | T0 `limit::tests::third_rejected_while_two_in_flight` 通过；T3 C1（N=1）与 Batch B（N=3/5/10）均实证 N-1=200、满额时第 N+1 立即 429（1.6～2.6ms）、放行后恢复 200 |
| TC-02 | 全局共享 / AC-2 | T3（+T0 辅助） | **通过** | T3 C3（N=2，两路半包 PUT 占住 `ns-a/h1` 与 `ns-b/h2`）：5 条**全部命中路由与方法**的业务探针（`GET /ns-a/k1`、`PUT /ns-c/k3`、`DELETE /ns-a/k1`、`GET /ns-a`、`GET /api/v1/config`）全部返回 429，放行后全恢复 200。**注**：既有单测 `shared_counter_across_routes_methods` 使用 `GET /b`（405 路径），不作为 AC-2 独立证明，AC-2 由 T3 C3 命中方法探针完备证明 |
| TC-03 | 错误契约 / AC-3、Q3 | T0 + T3 | **通过** | T0 `limit::tests::reject_body_shape`（N=3）+ T3 C1（N=1）实测：HTTP `429`、`content-type: application/json`、`retry-after: 1`、JSON 仅含 `{"code":"too_many_requests","message":"并发请求已达上限 1，请稍后重试"}`（精确含上限数值） |
| TC-04 | 无副作用 / AC-4 | T3 + T0 + 白盒 | **通过** | T3 C1 饱和期间发起 `GET /ns-a/keep`、`PUT /ns-a/blocked`、`DELETE /ns-a/keep` 均 429；`data/ns-a.json` 字节快照完全不变，放行后 `keep` 值仍为 `{"v":"orig"}`、`blocked` 仍 404；结合 `limit.rs:35-52` `guard` 在 `try_acquire_owned` 失败时直接短路返回、不调用 `next.run(req)` 的白盒结构，确证无读/写/删副作用与名额泄漏。（备注：既有单测 `third_rejected_while_two_in_flight` 未内嵌 handler 进入计数，不影响实测结论） |
| TC-05 | healthz 豁免 / AC-2、Q1 | T0 + T3 | **通过** | T0 `limit::tests::exempt_route_stays_available` + T3 C3 真实生产 router（N=2 饱和期间）：连续 5 次 `GET /healthz` 均返回 `200` 且 body 为 `{"status":"ok","service":"velox-config"}`，同刻业务路由仍 429（不占名额、不放宽业务上限） |
| TC-06 | 正常与失败释放 / AC-4、§5 | T3 | **通过** | T3 C5a（N=1 真实二进制）：依次实测 `200`（合法 PUT）、`400`（非法 namespace `bad$ns`）、`400`（非法 JSON body）、`404`（GET 不存在 key）、`404`（DELETE 不存在 key）、`500`（`data/ioerr.json` 同名目录诱发真实 `StoreError::Io`），全部 6 路响应后立即发 `GET /api/v1/config` 均 `200`，无 permit 泄漏 |
| TC-07 | 取消释放 / AC-4、FR-2 | T3 | **通过** | T3 C5b（N=1）：半包 PUT 占满名额并确认业务路由返回 429 后，客户端直接关闭 TCP socket 不补齐 body；服务端检测到断开并 drop 请求 future，随后 `GET /api/v1/config` 迅速恢复 `200`，且被取消的 `ns-a/cancel` 仍 404（零写入、名额归还） |
| TC-08 | 默认与 env 优先级 / AC-5、G1-3 | T0 + T1 | **通过** | T0 覆盖 7 项 config 单测（含新增 `file_value_kept_without_env`、`file_value_kept_with_empty_env`、`empty_env_when_key_missing_defaults_to_10`）；T1 Batch B 全部 7 种启动方式（①无文件=10、②文件缺键=10、③仅 env=3、④文件5+env3=3、⑤文件5+无env=5、⑥文件5+空env=5、⑦无文件/缺键+空env=10）逐一在真实二进制上验证 `N-1` 返回 200、`N+1` 返回 429、放行后恢复 200 |
| TC-09 | 空 env 与非法/合法组合 / AC-5、RQ-B-1 | T0 + T1 | **通过** | T0 `illegal_file_with_valid_env_selects_env` + `file_value_kept_with_empty_env` 通过；T1 Batch A1/B 真实二进制实证：①文件=5+空 env → 生效 5；②文件=0（非法）+非空合法 env=3 → 选中 env 启动成功生效 3（显式 info 与默认 warn 饱和双证）；③文件=5（合法）+非空非法 env=0 → `rc=1` 报错 `server.max_concurrent_requests：0`，不回落文件值 5 |
| TC-10 | 非法配置 fail fast / AC-6、Q4 | T0 + T1 | **通过** | T0 `invalid_zero_negative_fail_fast`、`invalid_float_string_fail`、`parse_domain_overflow_keeps_raw_value` 通过；T1 Batch A1 真实进程实证（预先占用监听端口 `18251`）：文件 `0/-1/1.5/"abc"/99999999999999999999999`/非法 TOML 与 env `0/-1/1.5/abc/99999999999999999999999` 全部 `rc=1`、无 panic、stderr 含配置项/env 全名与原始值、未创建 data/logs 目录、在监听端口绑定前即终止 |
| TC-11 | 合法下界与大值 / AC-6 | T0 + T1 | **通过** | T0 `valid_extremes`（1、100000）+ `valid_capacity_boundary_u` 通过；T1 Batch A2 真实进程启动 `1`、`1000`、`100000` 全部 `/healthz` 200、启动 INFO 含对应 `max_concurrent_requests=`、SIGINT `rc=0`；`N=1` 饱和行为另由 C1/C4/C5/C7 实证 |
| TC-12 | 技术容量边界 1..=U / AC-6、Q6 | T0 + T1 | **通过** | 64 位平台实测 `U = 2305843009213693951`：T0 `valid_capacity_boundary_u`（验证 `Semaphore::new(U).available_permits() == U`）+ `over_technical_cap_fails_with_bound` 全绿；T1 Batch A1/A2 真实进程：①`U` 启动 healthz 200、日志 `max_concurrent_requests=2305843009213693951`、SIGINT `rc=0`；②`U+1`（`2305843009213693952`）与 `i64::MAX`（`9223372036854775807`）文件与 env 路径均 `rc=1`、无 panic、监听前终止，stderr 输出 `Error: 非法配置 server.max_concurrent_requests：<原值>（超过本平台信号量技术上界 U=2305843009213693951）` |
| TC-13 | 装配默认与日志观察前提 / AC-5、AC-7、G1-1 | T1 | **通过** | addlog（`a5b75e5`）已入库，默认 warn 分支**完整实测**（不再缺依赖）：对 9 组启动来源逐一双跑——①显式 `RUST_LOG=info`：`logs/velox-config.log` 出现 `velox-config started` 且 `max_concurrent_requests=` 精确匹配（10/10/3/3/5/5/10/10/3）；②默认 `warn`（剥离 `RUST_LOG`/`LOG__LEVEL`）：日志文件无启动 INFO 行（静默符合预期），并以精确 `N-1=200` + `N+1=429` + 每拒 WARN（含 `limit=N`）证明上限生效 |
| TC-14 | 每拒一 WARN 与过滤预期 / AC-7 | T3 | **通过** | T3 C1（默认 `warn`）：制造 3 次 429（`GET /ns-a/keep`、`PUT /ns-a/blocked`、`DELETE /ns-a/keep`），`logs/velox-config.log` 精确落盘 **3 条** `WARN`（含 `limit=1` 与对应 `path=`，无采样省略、成功请求无误报）；T3 C2（`VELOX_CONFIG__LOG__LEVEL=error`）：3 次 429 正常返回且日志含 0 条 WARN，验证过滤预期 |
| TC-15 | 立即拒绝真实链路 / AC-1、3、4、7 | T3 | **通过** | T3 C1（真实 `velox-config`，N=1，半包 PUT 占名额）：持有者未放行前，3 次业务请求在 `1.3～2.0ms` 内立即返回 429（不排队），错误契约与每拒一 WARN 均满足；补齐 body 后 PUT 返回 200，读回 `ns-a/hold` 值一致，无永久饱和 |
| TC-16 | 竞态不超卖 / AC-1、NFR-3 | T3 | **通过** | T3 C6（真实 `velox-config`，N=2，10 线程 `threading.Barrier` 同时发送半包 PUT，连跑 3 轮）：三轮结果精确为 `(200=2, 429=8)`、`[(2,8),(2,8),(2,8)]`，每轮实际写入 key 数均精确等于 2（零超卖、被拒者零写入），放行后业务立即恢复 200 |
| TC-17 | 404/405 边界实跑留痕 / FR-2 | T0 + T3 | **通过** | T0 `unmatched_route_bypasses_limit` 绿；T3 C4（真实 `velox-config`，axum 0.8.9）独立实跑留痕：**未饱和**时 `GET /not-exist`=`404`、`POST /api/v1/config/ns-a/k1`=`405`、`PUT /api/v1/config/ns-a`=`405`（且不泄漏名额）；**饱和（N=1）**时 `GET /not-exist`=`404`（绕过限流层），`POST /api/v1/config/ns-a/k1`=`429`、`PUT /api/v1/config/ns-a`=`429`、命中对照 `PUT /api/v1/config/ns-a/k2`=`429`。**实跑完全印证架构 §2.3/§4.4 与 `api.rs:16-24` 注释定论**（405 先过限流层，饱和 429、未饱和 405） |
| TC-18 | 生产 router 装配 / AC-1、2、Q1 | T0 + T3 | **通过** | T0 `api::tests::smoke_router_assembly` 绿；T3 C3 在真实 `api::router(store, 2)` 装配下验证 5 条业务路由共用单一限流入口、饱和时 429、`/healthz` 返回 `200 {"status":"ok","service":"velox-config"}`、放行后业务全部恢复 |
| TC-19 | SIGINT 优雅退出 / §5 | T3 | **通过** | T3 C7（真实 `velox-config`，N=1）：半包 PUT 占满名额并确认 429 后发送 `SIGINT` → 等待 0.5s 进程仍存活（未强断在途请求）→ 补齐 PUT body 收到 `200 OK` → 进程随后以 `rc=0` 干净退出，`data/ns-grace.json` 持久化完整 |
| TC-20 | 文档审查与锚点核对 / FR-4、NFR-4、Q5、RQ-B-2 | T2/手工 | **通过** | 逐行核对 `velox-config/config/velox-config.toml:5`（默认 10 + `1..=U` 公式）及 `velox-config/README.md:44,51-53`：覆盖单实例进程级、合法技术域 `1..=U`、超界报错含 `U`、空 env 不覆盖文件、需重启生效、默认 10 行为收紧声明、429 `code=too_many_requests` + `Retry-After: 1`、调优指引、`/healthz` 豁免、默认 `warn` 静默与 `RUST_LOG=info`/`VELOX_CONFIG__LOG__LEVEL=info` 核验前提，无误称速率/集群配额。**锚点行号留痕（RQ-B-2）**：`01-requirements.md` §8 历史引 `README.md:40`，经 addlog 与 4b24a20 更新后实际位置为配置节 `:39`、配置表 `:41-47`（项在 `:44`）、说明与收紧段 `:51-53`，内容全部齐备属实 |
| TC-21 | 统一回归 / AC-1～7 关联 | T2 + T3 | **本轮常规通过**（统一终验待 F-TA-F 确认） | 已对提交 `4b24a20` 完整执行：受影响套件 23/23、静态三门槛全绿、workspace 129/129（除已知 demo flaky 隔离 11/11 绿）、`acceptance.sh` PASS（27.2s）、TC-15/19 等专项 e2e 全绿；若 `4b24a20` 为最终提交，F-TA-F 终验轮可核对后复核/复用 |

### 冻结 AC 覆盖汇总

| 冻结 AC | 关联 TC | 实测结论 |
|---|---|---|
| AC-1（上限生效且不超卖） | TC-01、TC-15、TC-16、TC-18 | **通过** |
| AC-2（全局共享跨路由/方法 + Q1 healthz 豁免） | TC-02、TC-05、TC-18 | **通过** |
| AC-3（429 错误体与 Retry-After: 1） | TC-03、TC-15 | **通过** |
| AC-4（零副作用、失败/取消均释放） | TC-04、TC-06、TC-07、TC-15 | **通过** |
| AC-5（默认 10、优先级、空 env 不覆盖、7 种来源） | TC-08、TC-09、TC-13 | **通过** |
| AC-6（非法值 fail-fast、合法域 1..=U 无损比较） | TC-10、TC-11、TC-12 | **通过** |
| AC-7（可观测性 INFO/WARN 与过滤预期） | TC-13、TC-14、TC-15 | **通过** |

---

## 4. 回归结果

- **静态门槛**（`4b24a20` 工作树基线）：
  - `cargo fmt --all --check`：通过
  - `cargo clippy --workspace --all-targets -- -D warnings`：通过（0 warnings）
  - `cargo build --workspace`：通过
- **workspace 单元测试与集成测试**（`cargo test --workspace --no-fail-fast`，总计 130 例，耗时 4.9s）：
  - `velox-config`：**23/23**（含新增 7 例上界/优先级单测；`--test-threads=8` 连续 3 轮全绿）
  - `velox-log`：**28/28**
  - `velox-registry`：**59/59**（lib 38 + compat 2 + persistence 8 + watch 11）
  - `velox-mesh`：**4/4**
  - `demo-provider`：**5/5**
  - `demo-consumer`：多线程并发下 10/11，仅已知既有 env 竞态 flaky `config::tests::defaults_match_contract` 失败；隔离复测 `cargo test -p demo-consumer -- --test-threads=1` **11/11 全绿**（非本 feature 引入，不计入本次回归失败）。
- **端到端协作回归**：
  - `bash velox-demo/scripts/acceptance.sh`（零修改直接运行，耗时 27.2s）：**PASS**（步骤 5/5 全部断言通过，退出后无残留进程）。
- **新增回归失败清单**：**无**。

---

## 5. 缺陷清单

**无**（本轮未发现产品缺陷或阻塞性测试资产缺陷）。

> 非缺陷观察项留痕（不阻断验收）：
> 1. 既有单测 `velox-config/src/limit.rs` 中 `third_rejected_while_two_in_flight` 未内嵌 handler 调用计数断言、`shared_counter_across_routes_methods` 探针为 `GET /b`（405 路径）——均属 `dff2006` 既有测试资产细节，本轮已通过 T3 活体 C1/C3 命中方法探针与 `limit::guard` 白盒短路获得完整 AC-2/AC-4 证据，不构成缺陷。
> 2. `demo_consumer::config::tests::defaults_match_contract` 为仓库既有跨包 env 竞态 flaky，已在历史轮次登记，隔离必绿，非本 feature 引入。
> 3. `01-requirements.md` §8 引 `README.md:40` 的行号随文档演进微移至 `:39/:44/:51-53`（QA P3 RQ-B-2），内容全部属实齐备。

---

## 6. 未执行/无法验证用例及原因

**无**（21/21 条 TC 均已执行并取得明确结论；原 03-test-cases.md 标注的"默认 warn 进程级场景依赖 addlog 合并"因 `a5b75e5` 已入库，本轮全部完整实测通过，无遗留无法验证项）。
