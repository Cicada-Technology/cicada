# 模块注册契约 · 常见路径优先方案（common path）

> 问题：一个领域模块，怎么把自己的路由、迁移、种子（菜单/权限点/字典）、OpenAPI 片段
> 注册进组装根。三种装配形态都要成立：单体、模板工程（框架模块与业务模块同进程）、微服务（只装一个模块）。
>
> 本方案的取舍轴：**为最常见的调用方优化**。最常见的是两类——(a) 使用者的模板工程
> `main.rs`，只想「一行接入框架全部模块 + 自己的业务模块」；(b) 测试，只想「装一个模块、
> 连一个临时库、跑一次断言」。所以默认路径要短到接近零，非常规需求（自定义注册点、
> 特殊顺序、条件注册）退化成一条更啰嗦但不常用的**逃生通道**。
>
> 与草图（`Module` trait 上列 5 个方法）的分歧：本方案把 trait 压成**一个方法**，
> 返回值是一个数据清单（`Manifest`），把「怎么装配」的策略全部搬进组装根。模块作者
> 声明事实，框架决定顺序、校验与失败。

---

## 1. 接口

### 1.1 唯一入口点

```rust
// crates/web/src/module.rs（为什么放这里而不是 ports，见 §4）
use axum::Router;
use sea_orm_migration::MigrationTrait;

/// 模块唯一必须实现的东西。除它之外没有第二个方法。
pub trait Module: Send + Sync + 'static {
    /// 交出本模块的清单。
    ///
    /// 取 `self: Box<Self>` 而不是 `self`，是为了保住 `dyn Module` 的可用性：
    /// 框架模块（feature 裁剪后是不同集合）与使用者的业务模块必须能进同一个
    /// `Vec<Box<dyn Module>>`。实现者自己看不到这个细节——`declare_module!` 宏
    /// 会生成它（§2.1 的默认路径只有 6 行）。
    fn manifest(self: Box<Self>) -> Manifest;
}
```

### 1.2 清单：事实的容器

```rust
use utoipa::openapi::OpenApi;

/// 模块对框架的全部声明。所有字段都可缺省，只有 name 必填。
#[derive(Debug, Clone)]
pub struct Manifest {
    /// 必填。装配期的键：重名检测、`after`、依赖推导都用它，例如 `business:product`。
    pub name: ModuleName,
    /// 本模块**拥有**的表。用来推导跨模块顺序、校验种子属主；默认空。
    pub owns: Vec<TableName>,
    /// 本模块的 HTTP 路由。默认 `no_routes`（空 Router）。
    pub routes: RoutesFn,
    /// sea-orm 迁移，按声明顺序 up。默认空。
    pub migrations: Vec<Box<dyn MigrationTrait>>,
    /// 权限点（`system:user:add` 形式）。见 §3「不直写别人的表」。
    pub perms: Vec<Permission>,
    /// 菜单节点（M 目录 / C 菜单）。与 perms 一起交给菜单属主，不自己插 sys_menu。
    pub menus: Vec<MenuNode>,
    /// 种子数据。只允许写 `owns` 里的表，默认空。
    pub seeds: Vec<SeedOp>,
    /// OpenAPI 片段，默认空。
    pub openapi: OpenApi,
    /// 非数据原因的附加偏序（例如「我的路由要挂在审计桥之后」）。默认空。
    pub after: Vec<ModuleName>,
}

/// 路由工厂。接收已构造好的 AppState，返回只含自己路由的 Router。
pub type RoutesFn = fn(AppState) -> Router<AppState>;

fn no_routes(_: AppState) -> Router<AppState> { Router::new() }
```

默认路径的糖：一个 builder 与一个声明宏。

```rust
impl Manifest {
    pub fn new(name: impl Into<ModuleName>) -> ManifestBuilder { /* 全字段取默认值 */ }
}

impl ManifestBuilder {
    pub fn routes(self, f: RoutesFn) -> Self { /* ... */ }
    pub fn owns(self, t: impl IntoIterator<Item = TableName>) -> Self { /* ... */ }
    pub fn migrations(self, m: impl IntoIterator<Item = Box<dyn MigrationTrait>>) -> Self { /* ... */ }
    pub fn perms(self, p: impl IntoIterator<Item = Permission>) -> Self { /* ... */ }
    pub fn menus(self, m: impl IntoIterator<Item = MenuNode>) -> Self { /* ... */ }
    pub fn seeds(self, s: impl IntoIterator<Item = SeedOp>) -> Self { /* ... */ }
    pub fn openapi(self, o: OpenApi) -> Self { /* ... */ }
    pub fn after(self, m: impl IntoIterator<Item = ModuleName>) -> Self { /* ... */ }
    pub fn build(self) -> Manifest { /* ... */ }
}

/// 默认路径：一个模块 = 一个声明块。生成的 `Module` 类型供组装根按名引用。
#[macro_export]
macro_rules! declare_module {
    (
        name: $name:literal,
        routes: $routes:path,
        $(owns: [$($table:literal),* $(,)?],)?
        $(migrations: [$($mig:path),* $(,)?],)?
        perms: [$($perm:literal),* $(,)?] $(,)?
    ) => {
        pub struct Module;

        impl $crate::Module for Module {
            fn manifest(self: Box<Self>) -> $crate::Manifest {
                $crate::Manifest::new($name)
                    .routes($routes)
                    $(.owns([$($crate::TableName::from($table)),*]))?
                    $(.migrations(vec![$(Box::new($mig)
                        as Box<dyn ::sea_orm_migration::MigrationTrait>),*]))?
                    .perms([$($crate::Permission::new($perm)),*])
                    .build()
            }
        }
    };
}
```

### 1.3 组装根

```rust
pub struct AppBuilder { /* modules: Vec<Box<dyn Module>> */ }

impl AppBuilder {
    pub fn new() -> Self { /* ... */ }

    /// 框架自身的模块。被 feature 裁掉的不在编译结果里，也不出现在任何注册点。
    pub fn with_all_enabled(self) -> Self {
        #[allow(unused_mut)]
        let mut b = self;
        #[cfg(feature = "system")]  { b = b.with(cicada_system::SystemModule); }
        #[cfg(feature = "rbac")]    { b = b.with(cicada_rbac::RbacModule); }
        #[cfg(feature = "audit")]   { b = b.with(cicada_audit::AuditModule); }
        #[cfg(feature = "monitor")] { b = b.with(cicada_monitor::MonitorModule); }
        /* ... 其余框架模块 */
        b
    }

    pub fn with<M: Module>(self, m: M) -> Self { self.with_dyn(Box::new(m)) }
    pub fn with_dyn(self, m: Box<dyn Module>) -> Self { /* ... */ }

    /// 使用者的业务模块批量接入：`fn register(b: AppBuilder) -> AppBuilder`
    /// （代码生成器把新模块追加进这个函数，使用者不需要改动别的文件）。
    pub fn with_business<F>(self, f: F) -> Self
    where F: FnOnce(AppBuilder) -> AppBuilder { f(self) }

    /// 逃生通道之一：条件注册。
    pub fn with_if(self, cond: bool, m: Box<dyn Module>) -> Self {
        if cond { self.with_dyn(m) } else { self }
    }

    /// 装配。校验 → 迁移 → 种子 → 合并路由 → 合并 OpenAPI；任何一步失败都不启动服务。
    pub async fn build(self, infra: Infra) -> Result<App, AssembleReport> { /* ... */ }

    /// CLI 用：只解析，不连接任何东西，不构造 AppState。
    pub fn resolve(self, cfg: &Config) -> Result<Assembly, AssembleReport> { /* ... */ }
}

impl App {
    pub async fn serve(self) -> Result<()> { /* ... */ }
    pub fn router(&self) -> Router { /* 测试直接拿它打 oneshot */ }
    pub fn assembly(&self) -> &Assembly { /* migrations / seeds / perms / openapi */ }
}
```

### 1.4 不变量（调用方必须知道的事实）

1. `name` 全局唯一。重名不是警告，是启动失败。
2. 种子只能写 `owns` 声明的表。写别人的表会失败，错误信息指向应走哪个端口。
3. `perms` / `menus` 不直插 `sys_menu`，而是交给该表属主注册的端口（`MenuRegistry`）。
   没有属主在场 = 这些权限点没人落库 = 启动失败。
4. 迁移是**前置阶段**：全部模块的迁移都 up 成功之后，才跑任何种子。
5. 中间件顺序由 builder 固定（request_id → trace → cors → auth → rbac → 业务路由），
   模块无法插入、无法改动。模块只贡献路由，不贡献中间件。
6. 模块间顺序 =（所有权/种子属主推导出的边）∪（`after` 声明的边），必须无环。
7. `build()` 是 fail fast 的：一次报出全部装配错误，然后退出，不留半个可用服务。

### 1.5 顺序约束怎么表达

| 约束 | 表达方式 | 谁来写 |
| --- | --- | --- |
| 迁移先于种子 | 框架固定阶段序 | 不用写 |
| 鉴权中间件先于业务路由 | 框架固定 Layer 序 | 不用写 |
| 我依赖别人的表 | 声明 `owns` + 种子的目标表，框架自动加边 | 只声明事实 |
| 非数据原因的顺序 | `after: [ModuleName]` | 逃生通道 |
| 覆盖框架默认顺序 | 不支持 | —— |

### 1.6 错误模式（fail fast）

```rust
#[derive(Debug, thiserror::Error)]
pub enum AssembleError {
    #[error("模块重名：{0}")]
    DuplicateModule(ModuleName),
    #[error("权限点被两个模块声明：{perm}（{first} 与 {second}）")]
    DuplicatePermission { perm: Permission, first: ModuleName, second: ModuleName },
    #[error("路由声明了权限点 {perm}，但没有任何模块 seed 它（来自路由 {route}）")]
    UnseededPermission { perm: Permission, route: String },
    #[error("模块 {module} 的种子写了自己不拥有的表 {table}；该表属 {owner:?}")]
    ForeignTableSeed { module: ModuleName, table: TableName, owner: Option<ModuleName> },
    #[error("未知的装配依赖：{0}")]
    UnknownDependency(ModuleName),
    #[error("装配依赖成环：{0:?}")]
    DependencyCycle(Vec<ModuleName>),
    #[error("迁移冲突：{0}")]
    MigrationConflict(String),
    #[error("OpenAPI 冲突：{method} {path} 被 {first} 与 {second} 同时声明")]
    OpenApiConflict { method: String, path: String, first: ModuleName, second: ModuleName },
}

/// 一次报全部，避免「修一个报一个」。
pub struct AssembleReport { pub errors: Vec<AssembleError> }
```

---

## 2. 使用示例

### 2.1 默认路径：业务模块作者（代码生成器产出，6 行）

```rust
// business/src/product/mod.rs
cicada::declare_module! {
    name: "business:product",
    routes: product::routes,                       // fn(AppState) -> Router<AppState>
    owns: ["biz_product", "biz_product_sku"],
    migrations: [product::migrations::M20261001CreateProduct],
    perms: [
        "business:product:list", "business:product:query", "business:product:add",
        "business:product:edit", "business:product:remove", "business:product:export",
    ],
}
```

六个权限点只在这里声明一次；`#[require_perm("business:product:add")]` 标在 handler 上，
装配期两者对账（§3）。作者不需要知道迁移与种子谁先跑，也不需要知道 `sys_menu` 归谁。

### 2.2 组装根：模板工程 `main.rs`（4 行）

```rust
// my-app/src/main.rs
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    cicada::bootstrap(env!("CARGO_PKG_NAME"))
        .with_all_enabled()                  // 框架模块；feature 裁掉的不在编译结果里
        .with_business(business::register)   // 使用者的业务模块，生成器追加，这里永不手改
        .serve()                             // 迁移 → 种子 → 起监听 → 优雅停机
        .await
}
```

### 2.3 测试：只装一个模块（核心 3 行）

```rust
#[tokio::test]
async fn product_list_only_shows_own_tenant() -> anyhow::Result<()> {
    // 临时 PG（testcontainers）+ 只跑 ProductModule 的迁移与种子
    let t = cicada_testkit::TestApp::new(ProductModule).await?;
    let token = t.login_admin().await?;
    t.get("/api/v1/business/products")
        .bearer(&token)
        .expect_page(1, 20)
        .rows(2)
        .await?;
    Ok(())
}
```

`TestApp::new(M)` 内部就是 `AppBuilder::new().with(M).build(临时 Infra)`，测试与生产
走**同一个装配器**——这正是「接口即测试面」。

### 2.4 CLI：只要迁移与种子，不要 HTTP

```rust
// apps/cli
let assembly = cicada::assemble()
    .with_all_enabled()
    .with_business(business::register)
    .resolve(&cfg)?;          // 不连库、不建 AppState、不监听端口

match cmd {
    Cmd::Migrate  => assembly.migrate(&db).await?,
    Cmd::Seed     => assembly.seed(&db).await?,
    Cmd::InitAdmin=> assembly.init_admin(&db, &args).await?,
    Cmd::Perms    => println!("{}", assembly.permission_report()), // 导出给前端/OpenAPI
}
```

### 2.5 默认路径 vs 逃生通道：代码量

| 场景 | 默认路径 | 逃生通道 |
| --- | --- | --- |
| 声明一个业务模块 | `declare_module!` **6 行** | 手写 `impl Module` + `Manifest::new(..)` 链式，**18–40 行** |
| 组装根接入全部业务模块 | `.with_business(business::register)` **1 行** | 逐个 `.with(...)`，每模块 1 行 |
| 测试装一个模块 | `TestApp::new(M)` **1 行** | `TestApp::builder().only(M).ports(..).build()` **≈8 行** |
| 条件注册 | —— | `.with_if(cfg.mq.enabled, Box::new(NotifyModule))` |
| 特殊顺序 | —— | `.after([module_name!("cicada:audit")])` |

逃生通道长这样（只有少数模块会用到）：

```rust
impl cicada::Module for AuditBridgeModule {
    fn manifest(self: Box<Self>) -> cicada::Manifest {
        cicada::Manifest::new("cicada:audit-bridge")
            .after([cicada::module_name!("cicada:audit")])          // 显式顺序
            .owns(["biz_audit_bridge"])
            .routes(|st: cicada::AppState| {
                axum::Router::new()
                    .route("/internal/audit/health", axum::routing::get(health))
                    .with_state(st)
            })
            .migrations(vec![Box::new(M20261001Bridge)
                as Box<dyn sea_orm_migration::MigrationTrait>])
            .seeds(vec![cicada::seed::dict("audit_status", [("0", "成功"), ("1", "失败")])])
            .build()
    }
}
```

差异不在能力，而在**谁承担显式性**：默认路径把「顺序、属主、落库」交给框架推导，
逃生通道要求作者逐条写出来，作为交换，它可以表达框架没预料到的组合。

---

## 3. 实现藏在缝后面的东西

模块作者与调用方都看不见、但由这套接口承诺的机制：

- **阶段调度**：固定五阶段（校验 → 迁移 → 种子 → 路由 → OpenAPI），阶段内才做拓扑排序。
- **依赖推导**：由 `owns` 与 `SeedOp.target_table` 推出「谁的种子要等谁的迁移」，
  与 `after` 合并成一张有向图，跑环检测。
- **权限点双向对账**：`#[require_perm]` 宏在编译期把权限标识写进路由元数据；
  装配期取「路由声明的集合」与「`Manifest.perms` 声明的集合」求双向差集——
  路由声明了却没人 seed（漏注册）与 seed 了却没人用（孤儿权限点）都要报。
- **菜单/权限点落库的端口化**：属主模块（`cicada:system`）注册 `MenuRegistry` 端口，
  装配期把各模块的 `perms` / `menus` 交过去；端口缺席即失败，属主不直连别人的库。
- **种子冲突检测**：同表同主键、同权限标识、同字典键由两个模块写入 → 冲突。
- **迁移冲突检测**：版本号重复、同表 DDL 冲突在装配期（连库前）就报，而不是跑到一半炸。
- **OpenAPI 合并**：paths / schemas / components 三处冲突检测，冲突时给出两侧模块名。
- **feature 裁剪**：`with_all_enabled()` 由 `#[cfg]` 展开，被裁模块连依赖都不存在，
  因此不可能出现在任何注册点；`all` 门面 crate 只做 feature 转发。
- **中间件链装配**：request_id / trace / cors / auth / rbac 的顺序与参数由 builder 固定，
  模块无权介入；`#[require_perm]` 的元数据在这里被消费。
- **AppState 构造**：先装配清单、再建 `AppState`、再调用各模块的 `RoutesFn`——
  模块拿到的是完整依赖图，不是半成品。
- **Assembly 与 App 的分离**：`resolve()` 只产出 `Assembly`（迁移/种子/权限/OpenAPI），
  CLI、代码生成器、CI 的对账步骤都复用它，不启动 HTTP。
- **启动清单日志**：结构化打印「模块 → 顺序 → 表 → 权限点数量」，装配结果可审计。
- **测试装配的最小化**：`TestApp` 只跑被选模块的迁移与种子，不跑全量。

---

## 4. 依赖策略与 adapter

按 DEEPENING.md 的四类依赖逐条判定：

| 本接口涉及的依赖 | 分类 | 处理 |
| --- | --- | --- |
| 装配本身（校验、排序、合并、对账） | **in-process** | 纯计算 + 内存状态，直接通过接口测，不需要任何 adapter |
| 迁移与种子（要落 PostgreSQL） | **local-substitutable** | 真实 PG（testcontainers）或有替身时用替身；缝在实现内部，**不**提升为端口 |
| 模块间取数（`DeptDirectory` 等） | **remote but owned** | 走 `ports` crate 的端口 + 本地/远程两种 adapter——但这是**跨模块调用**的契约，不是本接口的一部分 |
| 真外部（无） | —— | 本接口不触达任何三方服务 |

**缝纪律结论**：本接口只有一个 adapter——the assembler 本身（`AppBuilder::build`）。
按「一个 adapter 是假缝，两个 adapter 才是真缝」，`Module` / `Manifest` **不应该**进
`ports` crate，而应留在 `cicada-web`（或 `kernel`）里做普通 trait。

本接口确实有**内部缝**，但保持私有：迁移/种子的执行器在「真库」与「测试替身」之间切换，
`TestApp` 用同一个装配器先 `resolve()` 再 `migrate()`，所以测试打到的是接口而非实现细节。
这是 local-substitutable 的正确形态——有替身，但不把替身暴露到接口上。

逃生通道里作者接触到的 `Manifest` builder 也不是新端口，它只是同一个数据结构的长写形式。

---

## 5. 取舍

**杠杆（leverage）高的地方**

一个方法 + 一份清单，换来：路由挂载、权限点对账、菜单落库、迁移排序、种子冲突检测、
OpenAPI 合并、feature 裁剪、测试装配。90% 的模块作者只写 6 行，组装根 4 行，测试 3 行。
代码生成器产出的模块天生适配默认路径，因此「生成即可编译、编译即可运行」不需要额外约定。

**薄（thin）的地方**

- `Manifest` 字段会缓慢膨胀。一旦有人开始往 `Manifest` 上加业务专属字段，它就在退化成
  「trait 上列死 10 个方法」的另一副面孔——需要一条硬规则：新字段必须是**每个模块都可能
  有的横切事实**，业务差异一律走逃生通道。
- 顺序只能用 `after` 表达偏序，无法插入到别人中间，也无法替换框架固定阶段。
  真正需要「在 auth 之前挂我的中间件」的模块只能放弃这一层，走另一条路。
- `.with_if(...)` 是运行期条件；编译期条件仍要靠 `#[cfg]`，两套并存需要文档说清楚。

**最容易失败的地方：默认值把必要的显式性吃掉**

1. **漏注册权限点**（最危险）。`perms` 默认为空，模块作者忘了声明按钮权限点，系统照常
   启动、迁移照跑、接口能调，只是前端的 `v-hasPermi` 恒为 false、菜单里没有按钮——
   一个「静默成功」的故障。对策是 §3 的**双向对账**：路由上 `#[require_perm]` 声明的权限
   必须在某个模块的 `perms` 里出现，否则 `build()` 直接失败。这是本方案唯一一处
   「默认路径必须带强制显式性」的地方，不能用配置关掉。
2. **顺序被默认值静默吃掉**。`after` 默认空，两个模块都往同一父菜单挂子节点时，顺序
   不确定、结果可能波动。对策：种子冲突检测 + 所有权推导出的边；检测到「不同模块写同一
   张表的同一语义槽位」时警告并建议显式 `after`。
3. **清单与实现漂移**。`owns` 里写的表和实体真正用的表不一致时，顺序推导就错了。
   对策：`owns` 与迁移产生的表名在装配期交叉校验（迁移是 Rust 代码，可静态提取）。

一句话概括这个方案的立场：**默认路径要短到「不用读文档也能写对」，但凡是会静默出错的
地方——权限点、表属主、跨模块顺序——都不给默认值，宁可让作者多写一行，也不让系统的
正确性依赖读者的记忆力。**
