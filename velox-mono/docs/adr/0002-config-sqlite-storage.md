# ADR 0002：velox-config 存储改用 SQLite 单文件

- 状态：已接受
- 日期：2025-11
- 决策人：velox 初始团队

## 背景

velox-config 的存储最初实现为「内存索引 + 写穿 `{namespace}.json` 文件」：写穿落盘需要额外的串行锁与快照时点论证，内存与文件可能状态分裂；文件重写也不具备跨记录的原子性。访问模式是单写多读、配置低频写入，同时需要原子性作为后续演进的地基——v0.2 的版本号/etag 与 v0.3 的审计日志都要求数据一落地就自洽可查。本次只替换存储层，HTTP API 契约不变。

## 决策

1. **存储引擎采用 SQLite 单文件**：经 rusqlite 接入，启用 `bundled` 特性将 SQLite 编译进二进制，构建不依赖系统 libsqlite3；承载 KV 表 `config(namespace, key, value, updated_at)`，以复合主键 `(namespace, key)` 加 `WITHOUT ROWID` 聚簇；
2. **SQLite 为唯一真相**：内存只保留可失效的 namespace 快照读缓存——TTL 默认 300 秒，`cache.ttl_secs` 可配，置 0 禁用；写操作即时驱逐对应快照，保证读己之写；
3. **旧数据一次性迁移**：目录中旧版 `{namespace}.json` 在首次启动时导入 SQLite，随后改名 `{namespace}.json.migrated`；
4. HTTP API 契约（路径、方法、JSON 结构）保持不变，本次仅替换存储层。

## 理由

- 事务与原子性：写操作天然原子，消除写穿模式下的串行锁与快照时点论证；
- 崩溃安全：WAL 模式下已提交写入不留半写状态，崩溃后库文件自洽；
- 单文件易备份：`data/velox-config.db` 即完整备份单元，拷贝文件即完成备份/恢复；
- 生态工具成熟：`sqlite3` CLI 可直接检视数据，排障无需专门工具；
- 依赖树小，与现有 tokio + axum 技术栈契合——rusqlite 经 `spawn_blocking` 接入即可，无需引入异步数据库驱动；
- `bundled` 特性保证跨机构建一致，不受目标机 libsqlite3 版本影响。

## 后果

- 正面：消除写穿并发正确性论证与内存/文件状态分裂；`updated_at` 列为 v0.2 版本号/etag 与 v0.3 审计日志预留了直接的数据基础；
- 负面：二进制体积与编译时间增加；数据库目录多出 `-wal`/`-shm` 副文件；读缓存引入最多一个 TTL 的陈旧窗口——单实例部署下可接受，集群路线（v0.4+）再做主动失效；
- 中立：SQL 文件可直接作为备份单元，备份策略从「目录快照」收敛为「单文件拷贝」。

## 替代方案（已否决）

- **sqlx**：依赖树庞大，编译期查询校验（`query!` 宏）对固定几条 SQL 收益低；
- **tokio-rusqlite**：多一层包装，收益仅是省掉手写 `spawn_blocking` 封装；
- **sled**：嵌入式 KV 生态与调试工具链弱于 SQLite；
- **维持 JSON 文件存储**：无法提供原子写，也没有版本号/审计演进的基础；
- **PostgreSQL 等外部数据库**：v0.1 单实例场景下运维成本过高。
