# 方案：模块注册契约 · 最小接口

> 设计约束：**把接口压到最小**。目标 1–3 个入口点封顶，最大化每个入口点的杠杆；宁可把复杂度全部藏进框架，也不让模块作者面对多个方法。
>
> 术语按 `codebase-design`：**模块 / 接口 / 缝 / adapter / 深度 / 杠杆 / 局部性**。

本方案的核心判断是：**注册契约不需要是一个 trait。** trait 会把「几件要注册的事」变成几个方法，方法数随注册点增长而增长——这正是反面草图的第一个病根。这里让模块作者只产出一个**值**，让组装根只调用**一个函数**：入口点数从「trait 的方法数」降到「1 个构造函数 + 1 个装配函数」。

---

## 1. 接口

### 1.1 类型

契约只有**一个值得学的东西**：`Module` 这个值。它是平凡数据（plain data），没有方法要学；字段即注册点，`Option`/`Vec` 的空值即「该模块不提供这一类」。每个字段都是**可选**的：纯库模块可以只给迁移，纯 API 模块可以只给路由。

```rust
// crates/web/src/registration.rs
// 依赖只向下：kernel 的错误/ID、axum::Router、sea_orm_migration、utoipa。不依赖任何领域 crate。

use axum::Router;
use sea_orm_migration::MigrationTrait;
use utoipa::openapi::OpenApi;

/// 组装根持有的运行时状态：只含 Arc 共享的只读依赖（Db / Cache / 各端口）。
/// 契约只提到这个类型，模块作者不构造它，只在自己的 handler 里用 State<AppState> 抽取。
pub struct AppState {
    pub db: Db,
    pub cache: Cache,
    pub dept_directory: Arc<dyn DeptDirectory>, // 例：跨模块端口，见 §4
    /* ... 其余只读依赖 ... */
}

/// 稳定标识。用于重名检测、feature 门控、冲突报错时指认「谁和谁撞了」。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct ModuleName(pub &'static str);

/// 框架拥有的 HTTP 路由载体。**内部**是 axum 的 Router，**契约**认的是这个 newtype。
/// 反面草图直接暴露 `Router<AppState>`，会让 axum 类型渗进契约、二期导 gRPC 时作废；
/// 这里把 axum 关在框架里：`Routes` 未来可以并列一个 `Grpc` 字段而不改本类型的地位。
pub struct Routes(Router<AppState>);

impl Routes {
    pub fn new(router: Router<AppState>) -> Self { Routes(router) }
    pub(crate) fn into_router(self) -> Router<AppState> { self.0 }
}

/// 种子项的稳定键，冲突判定的域就是它。例："perm:system:user:add" / "menu:system" / "dict:sys_normal_disable"。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SeedKey(pub &'static str);

/// 一条幂等种子操作：菜单 / 权限点 / 字典 / 参数。语义是 upsert，不是 insert。
pub struct SeedOp {
    pub key: SeedKey,
    pub body: SeedKind, // Menu {..} | Perm {..} | DictType {..} | DictData {..} | Config {..}
}

/// OpenAPI 片段：**一个值**，不是 `&mut OpenApi`。
/// 反面草图的第二个病根是共享可变引用；改成值时，文档合并顺序与去重由框架决定。
pub struct OpenApiFragment(pub OpenApi);

/// 模块作者唯一需要产出的东西。
///
/// 不做 `#[non_exhaustive]`（它会禁止字面量构造）。改用 `#[derive(Default)]` +
/// `..Module::new(..)` 的**函数式更新**：新增一个注册点只是加一个带默认值的字段，
/// 既有模块一个字都不用改——这修掉了反面草图「加第五个注册点就得改 trait」的病根。
#[derive(Default)]
pub struct Module {
    pub name: ModuleName,
    pub routes: Option<Routes>,
    pub migrations: Vec<Box<dyn MigrationTrait>>,
    pub seeds: Vec<SeedOp>,
    pub openapi: Vec<OpenApiFragment>,
}

impl Module {
    /// 基线值：只设名字，其余字段取默认（空）。
    pub fn new(name: &'static str) -> Self {
        Module { name: ModuleName(name), ..Default::default() }
    }
}
```

### 1.2 组装根的入口点（唯一）

```rust
// crates/web/src/assemble.rs

/// 组装根 / 测试 / CLI 三类调用方共用的**唯一装配入口**。
/// 这一步是 in-process：无 I/O、无网络，纯构造与校验（见 §4 依赖分类）。
pub fn assemble(deps: Deps, modules: Vec<Module>) -> Result<Assembled, AssemblyError>;

/// 装配结果：把「冻结后的模块集合」按相位暴露出去。它不是一个要遍历的列表，
/// 而是一台已经校验过、可反复取用的机器。
impl Assembled {
    /// 合并后的完整路由，已 `.with_state(AppState)`，可直接交给 axum 监听。
    pub fn router(&self) -> Router;
    /// 结构迁移（相位 1）。按 (模块序, 模块内序) 排列，版本号已协商。
    pub fn migrate(&self) -> impl std::future::Future<Output = Result<(), DbError>> + Send;
    /// 种子 upsert（相位 2）。幂等；与迁移分相位，见 1.4。
    pub fn seed(&self) -> impl std::future::Future<Output = Result<(), DbError>> + Send;
    /// 汇总后的 OpenAPI 文档。
    pub fn openapi(&self) -> OpenApi;
}
```

### 1.3 不变量

调用方（组装根、测试、CLI）可以依赖、且**框架必须保证**的性质：

1. **组装根不认识模块内部**：`assemble` 只读 `Module` 的字段，从不读领域 crate 的 entity 或内部类型。模块对组装根的可见面就是这一个值。
2. **`name` 全局唯一**：同一批 `modules` 里两个 `ModuleName` 相同即失败（`DuplicateModule`）。
3. **`SeedKey` 全局唯一**：两个模块给出同一个 `key` 即失败（`SeedConflict`），并报出两个肇事模块名。这是「两模块插入同一权限标识」的 fail-fast 点。
4. **feature 即缺席**：被裁掉的模块根本不在 `modules` 这个 `Vec` 里（`#[cfg(feature = ...)]` 决定是否 push）。因此它在路由、迁移、种子、文档**四处都不出现**——缺席是编译期事实，不需要运行期过滤。
5. **`Module` 是纯值**：不含闭包、不含对可变全局的引用、不做 I/O。构造它是廉价且可重复的；测试可以就地 `Module::new("x")` 造一个最小模块。
6. **`assemble` 是唯一装配路径**：`Assembled` 的字段全部私有，外部拿不到半成品状态。

### 1.4 顺序约束

顺序**不在接口里表达**，而是被框架的结构吸收——这正是接口变小的来源：

- **迁移先于种子**：不是作者写出来的，是 `Assembled::migrate()` 与 `Assembled::seed()` 两个相位天然分开；CLI 只会先调前者再调后者，没有交错的可能。
- **鉴权中间件先于业务路由**：中间件链属于 `assemble` 的内部实现，先 `.layer(..)` 再 `.merge(模块路由)`，模块作者根本无法把顺序搞反。
- **模块之间**：迁移按 `(模块在 Vec 中的次序, 模块内 Vec 次序)` 排列；需要更强次序时给 `SeedOp` / 迁移附一个可选序号，由框架稳定排序。作者平时不需要碰它。

因此「顺序颠倒」不是靠作者自律，而是靠**类型与相位**消除的。

### 1.5 错误模式

全部 fail fast：`assemble` 在绑定端口**之前**返回错误；迁移/种子在服务监听之前执行。

```rust
#[derive(Debug, thiserror::Error)]
pub enum AssemblyError {
    #[error("模块重名：{0:?}")]                         DuplicateModule(ModuleName),
    #[error("种子冲突：{key:?} 同时由 {first:?} 与 {second:?} 提供")]
    SeedConflict { key: SeedKey, first: ModuleName, second: ModuleName },
    #[error("路由冲突：{path} 同时由 {first:?} 与 {second:?} 注册")]
    RouteConflict { path: String, first: ModuleName, second: ModuleName },
    #[error("OpenAPI 冲突：{path} 被两个模块定义且不可合并")]
    OpenApiConflict { path: String },
    #[error("迁移版本冲突：{version}")]                 MigrationVersion { version: String },
    #[error("依赖构造失败：{0}")]                       Deps(String),
}
```

「重名」「种子冲突」「路由冲突」都在启动时抛出，而不是运行期给出诡异行为。

---

## 2. 使用示例

### 2.1 模块作者（唯一入口点）

每个模块在自己的 crate 里写**一个普通函数**，不实现任何 trait、不重写任何方法：

```rust
// crates/system/src/registration.rs
use cicada_web::{Module, OpenApiFragment, Routes, SeedOp};

pub fn module() -> Module {
    Module {
        routes: Some(Routes::new(system_router())),        // Router<AppState>，只含本模块路由
        migrations: vec![
            Box::new(m20260201_create_sys_user::Migration),
            Box::new(m20260202_create_sys_dept::Migration),
        ],
        seeds: vec![
            SeedOp { key: SeedKey("menu:system"),          body: SeedKind::Menu(system_menu()) },
            SeedOp { key: SeedKey("perm:system:user:add"), body: SeedKind::Perm("system:user:add") },
            SeedOp { key: SeedKey("dict:sys_normal_disable"), body: SeedKind::Dict(..) },
        ],
        openapi: vec![OpenApiFragment(system_openapi())],
        ..Module::new("system")
    }
}
```

要新增第五类注册点时，框架给 `Module` 加一个带默认值的字段，上面这段代码不动。

### 2.2 组装根（`apps/server`）

```rust
fn modules() -> Vec<Module> {
    let mut ms = vec![cicada_system::module(), cicada_monitor::module()];
    #[cfg(feature = "codegen")]                        // 裁掉的模块压根不 push
    ms.push(cicada_codegen::module());
    ms
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cfg  = Config::load()?;
    let deps = Deps::build(&cfg).await?;               // db / cache / 各 adapter

    let app = assemble(deps, modules())?;              // 唯一一次装配，fail fast
    app.migrate().await?;                              // 相位 1
    app.seed().await?;                                 // 相位 2
    axum::serve(listener, app.router()).await?;        // 相位 3：起服务
    Ok(())
}
```

### 2.3 测试（窄测试面）

测试走**同一个** `assemble`，只装配一个模块，因此测试面就是单模块：

```rust
#[tokio::test]
async fn add_user_requires_perm_and_respects_data_scope() {
    let deps = Deps::test().await;                     // 临时 PG（local-substitutable，见 §4）
    let app  = assemble(deps, vec![cicada_system::module()]).unwrap(); // 只装 system
    app.migrate().await.unwrap();
    app.seed().await.unwrap();

    let resp = app.router()                            // 用内存里的 router 直接发请求
        .oneshot(Request::post("/system/user").json(..))
        .await.unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}
```

### 2.4 CLI（`apps/cli`）

CLI 复用同一批模块与同一个 `assemble`，只是取不同的相位：

```rust
match cmd {
    Cmd::Migrate => app.migrate().await?,
    Cmd::Seed    => app.seed().await?,
    Cmd::Openapi => print!("{}", app.openapi().to_pretty_json()?),
}
```

三类调用方共用一条装配路径——`assemble` 是它们唯一的"接线员"。

---

## 3. 实现藏在缝后面的东西

缝在「领域模块 ⇄ 组装根」之间。缝后面由框架一手包办，模块作者与调用方都看不见：

1. **模块集合的冻结与校验**：重名检测、`SeedKey` 去重、路由路径冲突、OpenAPI 冲突——全部在 `assemble` 内一次跑完。
2. **相位调度**：迁移→种子的固定次序、CLI 与 server 共用同一份相位机。
3. **路由合并与状态注入**：`merge` 各模块 `Routes`，先挂中间件链（auth/rbac/审计/限流），最后 `.with_state(AppState)`。author 不接触 `AppState` 的构造。
4. **迁移版本协商**：模块各带自己的迁移，框架保证全局版本单调、append-only，并检出重复版本号。
5. **种子的幂等 upsert 与键命名空间**：`SeedKey` 前缀（`perm:` / `menu:` / `dict:`）划分域；同键冲突在装配期报错，不同键在 DB 上做 upsert（可重入）。
6. **OpenAPI 值的合并**：多个 `OpenApiFragment` 合并成一份文档，路径去重，不靠共享可变引用。
7. **feature 门控点**：裁剪模块 = 不把它的 `Module` 放进 `Vec`，四处注册点一并不出现。
8. **单模块测试夹具**：测试直接向 `assemble` 传一个 `Module` 即可获得窄测试面，无需启动整套应用。

用删除测试检验：若删掉 `assemble`，上面这份「合并 / 排序 / 冲突检测 / 相位」的逻辑会重新出现在**每个**模块、每个测试、每个 CLI 分支里——所以它值得留下。

---

## 4. 依赖策略与 adapter

按 `DEEPENING.md` 的四类归类注册缝上的依赖：

| 依赖 | 类别 | 策略 |
| --- | --- | --- |
| **装配本身**（合并路由、排序、校验、门控） | **in-process** | 纯计算、无 I/O。直接按接口测：给 `assemble` 一组 `Module`，断言 `Result` 或 `Assembled::router()`。无需 adapter。 |
| **迁移与种子** | **local-substitutable** | 有本地替身（临时 PostgreSQL / PGLite / testcontainers）。缝在模块内部，**不带端口**：测试用真 PG 跑迁移+种子，`Deps::test()` 即可。 |
| **模块间取数**（如 `DeptDirectory`） | **remote but owned** | 与注册契约**正交**：它在运行时端口上，不在注册缝上。故定义 trait 端口，生产用 `LocalDeptDirectory`（单体）或 `RemoteDeptDirectory`（微服务），测试用内存 adapter；切换点只在 `Deps` 一行。注册契约本身不引入这个端口。 |
| **真外部**（SMTP、S3、RabbitMQ） | **true external** | 同样在运行时端口上，不进注册契约。模块通过注入的 adapter 使用，注册时只提供一个 `Module` 值。 |

关键结论：**注册缝上只有 in-process 与 local-substitutable 两类依赖**。因此 `Module` 是纯值、`assemble` 是纯函数——这也是它能被极简化的根因。端口与 adapter 属于运行时依赖，被安置在 `ports` crate 与 `Deps` 里，不污染注册契约。

---

## 5. 取舍

**杠杆高在哪。**

- 模块作者面对**一个函数、一个值**：不实现 trait、不写方法、不排顺序、不碰 `AppState`、不手写文档合并。认知面被压到最小。
- 三类调用方共用**一个 `assemble`**：排序、去重、冲突检测、feature 门控只实现一次（局部性），所有调用方与测试共享同一份行为（杠杆）。
- 修掉了反面草图三处病根：`Router<AppState>` 被 `Routes` newtype 关住；`&mut OpenApi` 变成 `OpenApiFragment` 值；「四件事写进 trait」变成「一个可扩展的值结构」（`..Module::new(..)` 让新增注册点非破坏）。

**哪里薄。**

- **`Routes` 是不透明树**：框架拿不到逐路由的元数据（如 `#[require_perm]` 声明的权限标识），无法在装配期校验「权限点是否已注册」。这点需要模块在构建 `Routes` 时顺带产出一份元数据旁路（例如 `Module` 增加一个字段承载「路由→权限标识」表）。契约本身不会因为缺这份数据而崩，但装配期的完整性校验会因此弱一档。
- **纯值没有「行为」的落点**：`Module` 能声明"有什么"，不能声明"启动时做什么"。需要后台常驻任务的模块（如定时任务 runner）超出本契约——它们要么进 `worker` 的独立装配点，要么给 `Module` 加一个 `on_start` 字段。这是本方案**最容易失败的地方**：一旦允许 `Module` 携带运行期副作用，它就不再是纯值，`assemble` 也不再是纯函数，"最小"的根基松动。
- **新增注册点需要框架发版**：加第五类注册点是 `ports` crate 的改动，不是模块作者能自行扩展的。这是「最小接口」用**可扩展性**换来的代价；用 `..Default::default()` 把它降到「加一个带默认值的字段」，但跨不出 crate 边界。
- **顺序约束只能表达结构性的那些**：迁移先于种子、中间件先于路由都很干净；但「A 模块的种子依赖 B 模块先建的表」这类**跨模块数据依赖**表达不了，只能靠同一模块内解决或人工约定序号。

**一句话**：把入口点从「trait 的 N 个方法」压到「1 个值 + 1 个函数」，代价是模块无法用契约表达**运行期行为**与**跨模块数据依赖**——前者是真风险，后者是已知边界。
