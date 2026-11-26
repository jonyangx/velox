# velox 工程脚本

workspace 统一入口脚本。所有脚本在仓库根目录（Cargo workspace 根）下执行，
自动解析路径与子系统清单，无需手动传 crate 全名。

| 脚本 | 用途 |
|------|------|
| `build.sh` | 编译（debug / release，可指定子系统） |
| `test.sh` | 检查门槛（fmt + clippy + build + test，支持裁剪模式） |
| `package.sh` | release 构建 + 组装发布目录 + tar.gz 归档 |
| `run.sh` | 服务启停与状态管理（start / stop / restart / status / logs） |
| `common.sh` | 公共库（被上述脚本 source，不直接执行） |

## build.sh —— 编译

```bash
scripts/build.sh                    # debug 构建全部
scripts/build.sh --release          # release 构建全部
scripts/build.sh registry mesh      # 只构建指定子系统（可省略 velox- 前缀）
```

## test.sh —— 测试与检查

CI 门槛与开发规范第 1 节一致：`fmt + clippy + build + test` 全绿。

```bash
scripts/test.sh            # 完整门槛（提交前必跑）
scripts/test.sh --lint     # 只跑静态检查：fmt + clippy
scripts/test.sh --fast     # 快速反馈：fmt + test
scripts/test.sh --unit     # 只跑 cargo test
```

## package.sh —— 打包

```bash
scripts/package.sh          # 打包全部子系统
scripts/package.sh registry # 只打包指定子系统
```

产物：`dist/velox-<版本>-<os>-<arch>.tar.gz`，内含：

```
bin/velox-<name>            # release 可执行文件
config/velox-<name>.toml    # 开发默认配置（生产配置不入库）
README.md / VERSION
```

## run.sh —— 运行管理

```bash
scripts/run.sh start                 # 按依赖顺序启动：registry → config → mesh
scripts/run.sh start mesh --release  # 只启动 mesh（release 产物，需先 build --release）
scripts/run.sh restart               # 重启全部
scripts/run.sh status                # 服务状态一览
scripts/run.sh logs mesh -f          # 跟踪 mesh 日志（tail -F，跟随轮转；Ctrl-C 退出）
scripts/run.sh stop                  # 反向顺序停止全部
```

约定：

- 启动顺序 `registry(:8300) → config(:8200) → mesh(:8100)`，停止时反向；
- 每个服务加载自身 `config/velox-<name>.toml`，文件不存在则用内置默认值；
- 可执行文件缺失时自动触发一次 `build.sh`；`--rebuild` 强制重编；
- 日志：进程自身写 `logs/<crate>.log`（addlog：进程内按大小轮转，含活跃共 3 份、
  单份 200 MiB 硬上限；归档 `.log.1/.log.2`），pid 写入 `.run/<crate>.pid`（均已 gitignore）。
  脚本不再重定向 stdout/stderr（stdout 丢弃、stderr 直达调用端，无无界副本）；
- 启动后自动探测 `/healthz`（约 10s 超时），失败时打印日志尾部辅助排查
  （初始化失败信息在本次启动 stderr 上，无第四个日志文件）；
- 日志级别：默认 `warn`（静默 INFO）。排障用 `RUST_LOG=info`（完整 EnvFilter 语法、
  优先级最高），或 `VELOX_<CRATE>__LOG__LEVEL`（严格语法：裸等级或 `target=level`）；
  也可改配置 `[log].level`；调级须重启。目录覆盖：`VELOX_<CRATE>__LOG__DIR` / `[log].dir`。
