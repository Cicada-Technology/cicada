# 最终设计：模块注册契约 + 数据范围接口（design-it-twice 裁决）

> 日期：2026-10-08
> 本文是对 `design-it-twice/` 下备选方案的最终裁决，裁决后即成为设计文档的修订基线。

## 0. 裁决摘要

| 候选 | 结论 | 一句话理由 |
| --- | --- | --- |
| 模块注册契约 | **common-path 打底，吸收 flexible 的 `Slot` 做底层与逃生通道** | 默认路径短（6 行），同时「注册点是一等概念」，新增注册点不改 trait |
| 数据范围接口 | **采用 typestate（`datascope-typestate.md`）** | 把「漏做数据范围过滤」从 review 纪律升级为编译错误 |

## 1. 模块注册契约（最终版）

一句话立场：**默认路径要短到不用读文档，但底层只有一套机制——`Slot` 注册点。**
`Manifest` 的常见字段只是 `Slot` 贡献的语法糖，框架与第三方、常见与非常见注册点走同一条路。

### 1.1 底层统一机制：`Slot`（注册点是一等概念）

```rust
// crates/web/src/registry.rs
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct SlotKey(pub &'static str);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Merge { Collect, Keyed, Exclusive }

pub struct Slot<T: Send + Sync + 'static> {
    key: SlotKey,
    kind: &'static str,                 // 错误信息里的人类可读名
    merge: Merge,
    key_of: Option<fn(&T) -> &'static str>,
    _t: PhantomData<fn() -> T>,
}

impl<T: Send + Sync + 'static> Slot<T> {
    pub const fn collect(key: SlotKey, kind: &'static str) -> Self { /* ... */ }
    pub const fn keyed(key: SlotKey, kind: &'static str, key_of: fn(&T) -> &'static str) -> Self { /* ... */ }
    pub const fn exclusive(key: SlotKey, kind: &'static str) -> Self { /* ... */ }
}
```

框架自带的注册点也是同一机制的自用实例，无特权路径：

```rust
// crates/web/src/slots.rs
pub mod slots {
    pub static ROUTES: Slot<Routes> = Slot::collect(SlotKey("http.routes"), "HTTP 路由");
    pub static MIGRATIONS: Slot<Box<dyn MigrationTrait>> = Slot::collect(SlotKey("db.migrations"), "结构迁移");
    pub static PERMS: Slot<Permission> = Slot::keyed(SlotKey("rbac.perms"), "权限点", |p| p.as_str());
    pub static MENUS: Slot<MenuNode> = Slot::keyed(SlotKey("rbac.menus"), "菜单", |m| m.key());
    pub static SEEDS: Slot<SeedOp> = Slot::keyed(SlotKey("db.seeds"), "种子", |s| s.key());
    pub static OPENAPI: Slot<OpenApiFragment> = Slot::collect(SlotKey("docs.openapi"), "OpenAPI 片段");
    pub static HEALTH: Slot<HealthCheck> = Slot::keyed(SlotKey("ops.health"), "健康检查", |h| h.key());
    pub static JOBS: Slot<JobSpec> = Slot::keyed(SlotKey("ops.jobs"), "定时任务", |j| j.key());
    pub static EVENTS: Slot<Subscription> = Slot::collect(SlotKey("ops.events"), "领域事件订阅");
    pub static METRICS: Slot<MetricSpec> = Slot::collect(SlotKey("ops.metrics"), "指标");
    pub static COMMANDS: Slot<CommandSpec> = Slot::keyed(SlotKey("cli.commands"), "CLI 子命令", |c| c.key());
    pub static DEPT_DIRECTORY: Slot<Arc<dyn DeptDirectory>> = Slot::exclusive(SlotKey("port.dept_directory"), "部门目录端口");
}
```

### 1.2 对外形状：`Manifest`（默认路径，作者只看这个）

```rust
// crates/web/src/module.rs
use axum::Router;
use sea_orm_migration::MigrationTrait;
use utoipa::openapi::OpenApi;

pub type RoutesFn = fn(AppState) -> Router<AppState>;

pub struct Manifest {
    pub name: ModuleName,                    // 必填；装配键
    pub owns: Vec<TableName>,                // 本模块拥有的表
    pub routes: RoutesFn,                    // 语法糖 = contribute(slots::ROUTES, Routes(routes))
    pub migrations: Vec<Box<dyn MigrationTrait>>,
    pub perms: Vec<Permission>,              // 语法糖 = contribute(slots::PERMS, …)
    pub menus: Vec<MenuNode>,                // 语法糖 = contribute(slots::MENUS, …)
    pub seeds: Vec<SeedOp>,                  // 语法糖 = contribute(slots::SEEDS, …)
    pub openapi: OpenApi,                    // 语法糖 = contribute(slots::OPENAPI, …)
    pub after: Vec<ModuleName>,              // 非数据原因的附加偏序
    pub extensions: Vec<Extension>,          // 逃生通道：框架未知的自定义注册点
}

pub struct Extension { slot: SlotKey, item: Arc<dyn Any + Send + Sync> }

impl Extension {
    pub fn contribute<T: Send + Sync + 'static>(slot: &'static Slot<T>, item: T) -> Self {
        Extension { slot: slot.key(), item: Arc::new(item) }
    }
}

pub trait Module: Send + Sync + 'static {
    fn manifest(self: Box<Self>) -> Manifest;
}
```

`declare_module!` 宏生成默认路径（6 行）；需要 `seeds`/`menus`/`openapi`/`after`/`extensions` 时
用 `Manifest::new(name).routes(..).seeds(..).extensions(..).build()` 的 builder 逃生通道。

### 1.3 逃生通道：新增框架事先不知道的注册点

```rust
// business/crm/src/rules.rs —— 框架完全没有「优惠券规则」这个概念
pub struct CouponRule { pub key: &'static str, pub eval: fn(&Cart) -> bool }
pub static COUPON_RULES: Slot<CouponRule> =
    Slot::keyed(SlotKey("crm.coupon_rules"), "优惠券规则", |r| r.key);

// 在 crm 模块的 manifest 里：
Manifest::new("business:crm")
    .routes(crm_routes)
    .extensions(vec![Extension::contribute(&COUPON_RULES, CouponRule { /* ... */ })])
    .build()
```

装配器对任意 `Slot` 都通过类型擦除 + `Merge` 语义收集；框架不需要知道这个槽存在。

### 1.4 装配器与 fail-fast 错误

```rust
pub struct AppBuilder { /* modules / contributions / phases，全部私有 */ }

impl AppBuilder {
    pub fn with_all_enabled(self) -> Self;              // #[cfg(feature=..)] 展开框架模块
    pub fn with<M: Module>(self, m: M) -> Self;
    pub fn with_business<F: FnOnce(AppBuilder) -> AppBuilder>(self, f: F) -> Self;
    pub fn with_if(self, cond: bool, m: Box<dyn Module>) -> Self;
    pub async fn build(self, infra: Infra) -> Result<App, AssembleReport>;
    pub fn resolve(self, cfg: &Config) -> Result<Assembly, AssembleReport>;
}

#[derive(Debug, thiserror::Error)]
pub enum AssembleError {
    DuplicateModule(ModuleName),
    DuplicateSlot { key: SlotKey, a: &'static str, b: &'static str },
    SlotConflict { slot: &'static str, key: &'static str, first: ModuleName, second: ModuleName },
    UnconsumedSlot { slot: SlotKey, module: ModuleName },
    UnseededPermission { perm: Permission, route: String },
    OrphanPermission { perm: Permission, module: ModuleName },
    ForeignTableSeed { module: ModuleName, table: TableName, owner: Option<ModuleName> },
    UnknownPhase { phase: PhaseKey, dep: PhaseKey },
    PhaseCycle(Vec<PhaseKey>),
    MissingPort { module: ModuleName, port: &'static str },
    MigrationConflict { module: ModuleName, other: ModuleName, version: u32 },
}

pub struct AssembleReport { pub errors: Vec<AssembleError> }  // 一次报全部
```

### 1.5 不变量与顺序（调用方必须知道）

1. `name` 全局唯一；重名即启动失败。
2. feature 裁剪后，被裁模块不进装配器，路由/迁移/种子/文档/健康检查等**同时缺席**。
3. 种子只写 `owns` 声明的表；写别人的表即失败。
4. `perms`/`menus` 不直插 `sys_menu`，交菜单属主注册的端口落库；属主缺席即失败。
5. 迁移先于种子、鉴权中间件先于业务路由由固定相位保证；模块只声明事实，不排顺序。
6. 每个被贡献的槽必须有消费者，槽键拼错即 `UnconsumedSlot`，杜绝静默丢失。
7. `build()` 一次报出全部装配错误，不留半个可用服务。

## 2. 数据范围接口（最终版 = typestate）

**决策：采用 `datascope-typestate.md`，取代设计文档 §6.3 的 `Scope::apply(q) -> Select<E>`。**

核心是把「已经过滤过」变成**类型的证据**，而不是一次调用的副作用：

- 查询唯一起点是 `Query::<E>::open()`，未经升格的状态 `Unchecked` **没有终结算子**；
  只有 `.scoped(&scope)` 升格到 `Scoped`（或 `.unscoped(R)` 显式豁免到 `TenantBound`/`Global`）
  之后才有 `all/one/count/fetch_page`。
- 实体用 `#[data_scope(tenant=.., dept=.., owner=.., deleted=..)]` 声明归属维度，
  由 `#[derive(DataScoped)]` 唯一实现（`DataScoped`/`Shape` sealed），未声明的实体进不了受控查询。
- `Denied` 一律 `WHERE false` + `warn`，失败关闭；RLS 会话变量只在 `Db::begin_scoped` 一处注入。
- `trybuild` 把「未升格就执行」「实体未声明」钉成编译错误。

完整类型、判定矩阵、不变量与取舍见 [`datascope-typestate.md`](datascope-typestate.md)。
采纳后对设计文档的改动：

- §6.3 的 `DataScoped` 保留（作为实体声明），但删除 `Scope::apply(q)->Select<E>` 用法，
  改为 `Query<E,S>` 状态机；`DataScope`/`Scope` 结构维持 §6.3 不变。
- §5.3 示例里的 `scope.apply::<SysUser>(q)` 改为 `Query::<SysUser>::open().scoped(&scope)`。

## 3. 与设计文档的落点对照

| 本次裁决 | 落点 | 动作 |
| --- | --- | --- |
| 模块注册契约 = Manifest + Slot 逃生通道 | §16.1、§3.2、§9.4 | 以本节 §1 为准；§16.1 的 `Manifest` 增加 `extensions` 字段，`crates/web` 新增 `registry.rs`/`slots.rs` |
| 数据范围接口 = typestate | §6.3、§5.3、§16.1 | 以本节 §2 为准；`kernel` 新增 `scope/query.rs`、`scope/entity.rs` |
| 三份备选方案文档 | `module-common-path.md` / `module-minimal.md` / `module-flexible.md` | 归档为备选，不再并列作为候选 |
