# 任务简报：模块注册契约 · 最小接口方案

## 身份

你在为 Rust 后台基座项目 **cicada** 设计一个接口。你是并行设计小组中的一员，只负责**一种**设计约束下的方案。

## 第一步：完整读这三个文件

1. `/home/cci/cicada/cicada/docs/superpowers/specs/2026-09-30-cicada-rust-base-design.md` —— 项目设计文档，重点看 §3 总体架构、§6.3 数据权限、§16 可扩展性/可拆分性/模板化二次开发。
2. `/home/cci/.codex/skills/codebase-design/SKILL.md`
3. `/home/cci/.codex/skills/codebase-design/DEEPENING.md`

后两个文件定义了必须使用的设计词汇（模块 / 接口 / 缝 / adapter / 深度 / 杠杆 / 局部性）。输出要用这套词汇，不要用「组件」「服务」「边界」这类替代词。

## 背景

cicada 是同类 Java 后台管理脚手架的 Rust 重写版基座。技术栈 axum 0.8 + tokio + SeaORM 2.0 + PostgreSQL，Cargo workspace。它必须同时支持三种装配方式：

- **单体**：全部模块同进程运行。
- **模板工程**：使用者的业务模块与框架模块并存于同一个二进制（业务代码在 `business/`，框架通过 git tag 依赖，永不 fork）。
- **微服务**：只装配一个模块，跨模块调用走远程 adapter。

## 设计对象：模块注册契约

一个领域模块如何把自己的**路由**、**数据库迁移**、**种子数据**（菜单/权限点/字典）、**OpenAPI 文档片段**注册到组装根。

缝的位置在「领域模块 ⇄ 组装根」之间。

## 所有方案都必须满足的约束

1. 组装根在不知道模块内部实现的前提下完成装配。
2. feature 裁剪后，被裁掉的模块不出现在任何注册点（路由/迁移/种子/文档都不出现）。
3. 模块之间只依赖端口（trait），不依赖对方的 entity 或内部类型。
4. 顺序约束可表达：迁移先于种子、鉴权中间件先于业务路由。
5. 错误模式 fail fast：模块重名、种子冲突（两个模块插入同一权限标识）、顺序颠倒，必须在启动时失败，而不是运行期诡异。
6. 测试能只装配一个模块，测试面才窄。
7. 依赖分类：装配本身是 in-process；迁移与种子是 local-substitutable（真实 PostgreSQL 或替身）。
8. 组装根、测试、CLI 是仅有的三类调用方。

## 你的设计约束（必须与其它方案截然不同）

**把接口压到最小。** 目标 1–3 个入口点封顶，最大化每个入口点的杠杆。宁可把复杂度全部藏在框架内部，也不让模块作者面对多个方法。如果必须超过 3 个方法，先用一段话论证「最小化在此不可能」。

## 已知的反面草图（不要照抄，它只是用来暴露约束的）

```rust
pub trait Module: Send + Sync {
    fn name(&self) -> &'static str;
    fn routes(&self) -> Router<AppState>;
    fn migrations(&self) -> Vec<Box<dyn MigrationTrait>>;
    fn seed(&self) -> Vec<SeedOp>;
    fn openapi(&self, doc: &mut OpenApi);
}
```

它至少有三处可疑：把四件事硬编码进一个 trait（想加第五个注册点就得改 trait）、`&mut OpenApi` 是共享可变状态、`Router<AppState>` 把 axum 类型泄进契约（二期想导 gRPC 就作废）。

## 输出

用 `apply_patch` 写一个 markdown 文件到 `/home/cci/cicada/cicada/docs/superpowers/design-it-twice/module-minimal.md`，必须包含这 5 节：

1. **接口**：类型、方法、参数、不变量、顺序约束、错误模式。用真实可编译风格的 Rust 代码，不要伪代码；引用具体类型如 `axum::Router`、`AppState`、`sea_orm_migration::MigrationTrait`。
2. **使用示例**：组装根、测试、CLI 三种调用方各自怎么写。
3. **实现藏在缝后面的东西**：清单式列出。
4. **依赖策略与 adapter**：按 DEEPENING.md 的依赖分类（in-process / local-substitutable / remote but owned / true external）说明。
5. **取舍**：杠杆高在哪、哪里薄，以及这个方案最容易失败的地方。

## 纪律

- 只允许写 `module-minimal.md` 这一个文件，不要修改设计文档或仓库里任何其它文件。
- 不要写实现代码，这是接口设计，不是实现。
- 最终回复用 200 字以内概括方案与它的最大代价。
