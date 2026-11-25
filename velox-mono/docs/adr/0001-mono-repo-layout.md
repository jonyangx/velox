# ADR 0001：monorepo 布局与 workspace 根的位置

- 状态：已接受
- 日期：2025-11（项目初始化）
- 决策人：velox 初始团队

## 背景

velox 由多个 Rust 子系统组成（mesh / config / registry / demo），需要决定代码组织方式与各仓库（目录）关系。初始团队规模小、子系统间共享依赖与规范、需原子性跨子系统变更。

## 决策

1. **采用 monorepo**：全部子系统置于单一 git 仓库 `velox/`；
2. **git 仓库根 = Cargo workspace 根**：根 `Cargo.toml` 以 `members` 声明 `velox-mesh`、`velox-config`、`velox-registry`；
3. **velox-mono 为文档目录（非 crate）**：作为项目统一入口，承载架构设计、子系统定位、开发规范与 ADR；
4. 依赖版本统一收敛在根 `[workspace.dependencies]`，工具链由 `rust-toolchain.toml` 锁定。

## 理由

- workspace 根放在 git 根是 Cargo monorepo 的标准形态（参考 tokio / bevy），避免 `../` 相对路径成员带来的工具链兼容问题；
- `velox-mono` 无运行时代码，做成 crate 只会产生无意义的编译目标与维护负担；
- 小团队跨子系统联调频繁（如 mesh 消费 registry 的 API），monorepo 可单 PR 原子变更并统一 CI 门禁。

## 后果

- 正面：统一版本管理、原子变更、规范集中、新人单点入口（velox-mono）；
- 负面：仓库膨胀后 CI 时长上升（以 path-filtered CI 缓解）；子系统无法独立授权（当前团队规模下可接受）；
- 中立：未来若某子系统需要独立发布节奏/权限，可平滑拆库——crate 边界已在目录层面划定。

## 替代方案（已否决）

- **多仓库 + path 依赖**：版本漂移风险高，联调成本大，小团队无收益；
- **velox-mono 作为 workspace 根**：成员目录在其兄弟位置，需 `../` 外部成员或整体迁入 velox-mono 下，前者兼容性差，后者使 git 根与工程根分离、目录语义混乱。
