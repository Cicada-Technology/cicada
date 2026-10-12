# 方案：模块注册契约 · 最大灵活性（注册点是一等公民）

> 设计约束：**最大化灵活性与可扩展性**。模块作者要能引入框架事先不知道的注册点（健康检查、
> 后台任务、事件订阅、指标采集、管理命令……），框架自身的注册点必须是同一套机制的自用实例，
> 使用者的业务模块与框架模块走**完全相同**的一条路径。
>
> 术语按 `codebase-design`：**模块 / 接口 / 缝 / adapter / 深度 / 杠杆 / 局部性**。

反面草图（`trait Module` 上列死 `routes / migrations / seed / openapi`）的病根是**把注册点
烧进了契约**：想加第五个注册点就得改 trait，一改就牵动全部实现、全部编译单元、全部下游 fork。
本方案把病根去掉的方式是：**注册点本身变成一个值**。

| 反面草图 | 本方案 |
| --- | --- |
| 注册点 = trait 上的方法 | 注册点 = `&'static Slot<T>` 常量 |
| 新增注册点 ⇒ 改框架 trait ⇒ 全体重编译 | 新增注册点 ⇒ 新声明一个 `Slot` 常量 ⇒ 没人被牵动 |
| 框架注册点走特权方法，业务模块模仿 | 框架注册点就是框架自己声明的 `Slot` 常量，零特权 |
| `&mut OpenApi` 共享可变状态 | 贡献是**值**，合并顺序由装配器决定 |
| `Router<AppState>` 写进 trait 签名 | axum 只出现在一个框架槽的**载荷**里，契约不认识 axum |

---

## 1. 接口

调用方（组装根、测试、CLI）与模块作者合计需要学的概念只有四个：`Slot`（注册点）、
`Module`（模块）、`Assembler`/`Assembled`（装配）、`Phase`（消费注册点的有序步骤）。
**新增注册点不引入任何新概念**——它就是再写一行 `static`。

### 1.1 稳定键

```rust
// crates/ports/src/registry.rs
// 依赖只向下：标准库 + kernel。不依赖任何领域 crate，也不依赖 axum。

/// 注册点的全局唯一键，例如 "http.routes" / "crm.coupon_rules"。
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub struct SlotKey(pub &'static str);

/// 消费步骤的键。约定相位键由它消费的槽键派生，除非一个相位消费多个槽。
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub struct PhaseKey(pub &'static str);

/// 装配产物的键，例如 "http.router" / "db.plan"。
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub struct ProductKey(pub &'static str);

/// 模块的稳定标识。用于重名检测、错误指认、贡献排序。
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub struct ModuleName(pub &'static str);
```

### 1.2 注册点：`Slot<T>`

注册点是一个**带类型载荷的常量**：没有方法要实现，没有 trait 要实现。

```rust
use std::{any::Any, marker::PhantomData, sync::Arc};

/// 同一个槽收到多份贡献时的合并语义，由槽作者声明、由装配器执行。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Merge {
    /// 全部收集，按 (模块装入序, 槽内声明序) 排列。
    Collect,
    /// 去重合并：贡献者提供稳定键，同键必须同体（幂等），否则冲突。
    Keyed,
    /// 至多一份贡献；第二份即冲突。端口绑定用的就是它。
    Exclusive,
}

/// 一个**一等注册点**。声明它只写一行，不需要改任何 trait。
///
/// `T` 必须 `Send + Sync + 'static`：贡献会被搬运进 `Assembled`，
/// 不能借用调用栈上的瞬时数据，也不能被跨线程改动。这条约束把「注册点」
/// 限制在**声明式事实**上——正是我们想要的。
pub struct Slot<T: Send + Sync + 'static> {
    key: SlotKey,
    /// 人类可读的槽名，只用于错误信息：「槽 `HTTP 中间件` 冲突……」。
    kind: &'static str,
    merge: Merge,
    /// `Merge::Keyed` 时用它取稳定键；其余语义为 `None`。
    key_of: Option<fn(&T) -> &'static str>,
    _t: PhantomData<fn() -> T>, // fn 指针保证 Send/Sync 与 T 无关
}

impl<T: Send + Sync + 'static> Slot<T> {
    pub const fn collect(key: SlotKey, kind: &'static str) -> Self {
        Self { key, kind, merge: Merge::Collect, key_of: None, _t: PhantomData }
    }
    pub const fn keyed(key: SlotKey, kind: &'static str, key_of: fn(&T) -> &'static str) -> Self {
        Self { key, kind, merge: Merge::Keyed, key_of: Some(key_of), _t: PhantomData }
    }
    pub const fn exclusive(key: SlotKey, kind: &'static str) -> Self {
        Self { key, kind, merge: Merge::Exclusive, key_of: None, _t: PhantomData }
    }
    pub const fn key(&self) -> SlotKey { self.key }
    pub const fn kind(&self) -> &'static str { self.kind }
}

/// 类型擦除视图：让异构的槽能进同一个 `Vec<Contribution>`。
/// 这是**内部**接口，调用方永远不实现它。
pub trait ErasedSlot: Send + Sync + 'static {
    fn key(&self) -> SlotKey;
    fn kind(&self) -> &'static str;
    fn merge(&self) -> Merge;
    /// 把本槽的全部贡献收成一个产品；收法由槽自己知道（只有它认识 `T`）。
    fn collect_into(
        &self,
        cs: &Contributions,
        product: ProductKey,
        out: &mut Products,
    ) -> Result<(), AssemblyError>;
}
impl<T: Send + Sync + 'static> ErasedSlot for Slot<T> { /* ... */ }
```

声明一个新注册点，就是：

```rust
pub static COUPON_RULES: Slot<CouponRule> =
    Slot::keyed(SlotKey("crm.coupon_rules"), "优惠券规则", |r| r.key);
```

**没有任何地方需要登记这个常量。** 框架不知道它存在，也能装配它。

### 1.3 贡献与 `Registrar`

模块作者与框架之间只有**一个写入方法**：`contribute`。路由、迁移、种子、OpenAPI、
端口、健康检查、任务、事件、指标——全部走它。这正是不变量「使用者模块与框架模块同路」的落点。

```rust
/// 一次贡献：擦除类型后仍记得自己属于哪个槽、来自哪个模块、槽内第几位。
pub struct Contribution {
    slot: &'static dyn ErasedSlot,
    module: ModuleName,
    seq: u32,
    item: Arc<dyn Any + Send + Sync>,
}

/// 模块登记贡献的入口。它的生命周期被绑在一次 `install` 上：**不能越界注册**。
pub struct Registrar<'a> {
    asm: &'a mut Assembler,
    module: ModuleName,
    seq: u32,
}

impl Registrar<'_> {
    /// 唯一需要学的写入方法。
    pub fn contribute<T: Send + Sync + 'static>(&mut self, slot: &'static Slot<T>, item: T) {
        let seq = self.seq;
        self.seq += 1;
        self.asm.push(Contribution::new(slot, self.module, seq, item));
    }

    /// 供贡献者生成稳定键用，例如 `SeedKey(leak(format!("{}:{}", self.module().0, name)))`。
    pub fn module(&self) -> ModuleName { self.module }
}
```

> 端口也是一次普通贡献：`r.contribute(&slots::DEPT_DIRECTORY, Arc::new(LocalDeptDirectory { db }))`。
> 槽的 `Merge::Exclusive` 保证「同一端口只能绑一个 adapter，绑两个即启动失败」。
> 端口没有独立机制——这是本方案「用一个机制覆盖全部注册点」的最强证据。

### 1.4 模块

```rust
/// 装配与测试的最小单元：一个名字 + 一批贡献。
///
/// 只有两个方法，但它们不是「注册点」——注册点是 `Slot`。`register` 是
/// 唯一的写入通道，`name` 只用于诊断与排序。加注册点永远不会碰这个 trait。
pub trait Module: Send + Sync + 'static {
    fn name(&self) -> ModuleName;
    fn register(&self, r: &mut Registrar);
}
```

### 1.5 相位：消费注册点的有序步骤

装配 = 把贡献按槽收进「产品」。收的动作叫**相位**，相位也由组装根声明（框架与使用者同一入口）。

```rust
/// 一个相位的动作。两种形态：通用收集，或自定义逻辑。
pub enum Step {
    /// 把某槽的贡献收成 `SlotCollect<T>` 塞进某产品键。覆盖 90% 的新注册点。
    Collect { slot: &'static dyn ErasedSlot, product: ProductKey },
    /// 自定义装配逻辑。**无捕获**的 fn 指针：装配期不共享可变状态。
    Custom {
        consumes: Vec<SlotKey>,
        run: fn(&Contributions, &mut Products) -> Result<(), AssemblyError>,
    },
}

pub struct Phase {
    pub key: PhaseKey,
    /// 偏序边：本相位必须在这些相位之后。
    pub after: Vec<PhaseKey>,
    pub step: Step,
}

impl Phase {
    /// 常见路径：收集一个槽成一个产品，键由槽派生。
    pub fn collect(slot: &'static dyn ErasedSlot, product: ProductKey) -> Phase { /* ... */ }
    /// 逃生通道：自定义相位。`run` 是 fn 指针，装不进闭包，因此装配期不共享可变状态。
    pub fn custom(
        key: PhaseKey,
        run: fn(&Contributions, &mut Products) -> Result<(), AssemblyError>,
    ) -> Phase { /* ... */ }
    pub fn after(mut self, deps: &[PhaseKey]) -> Phase { self.after.extend_from_slice(deps); self }
}
```

### 1.6 装配器与产物

```rust
pub struct Assembler { /* contributions, slots, phases, modules，全部私有 */ }

impl Assembler {
    pub fn new() -> Self;

    /// 装入一个模块：调用它的 `register`，把贡献按 (装入序, 声明序) 记序。
    /// 模块重名立即失败。`#[cfg(feature = "..")]` 决定要不要调用它——
    /// 被裁掉的模块**根本不进装配器**，于是四处注册点同时缺席。
    pub fn install(&mut self, m: &dyn Module) -> Result<(), AssemblyError>;

    /// 声明一个槽（可无贡献）。用于「本相位消费的槽键必须真实存在」这条检查。
    pub fn slot(&mut self, slot: &'static dyn ErasedSlot) -> Result<(), AssemblyError>;

    /// 声明一个相位。相位键重复、`after` 指向未知相位、成环，都立即失败。
    pub fn phase(&mut self, phase: Phase) -> Result<(), AssemblyError>;

    /// 封口：跑相位、收产品、做全部跨模块校验。之后装配计划不可再改。
    pub fn seal(self) -> Result<Assembled, AssemblyError>;
}

/// 封口后的结果：不是一份要遍历的清单，而是一台已校验、可反复取用的机器。
pub struct Assembled { products: Products, /* + 冻结的槽/相位清单，供 /docs 与 CLI 自省 */ }

impl Assembled {
    pub fn get<P: 'static>(&self, key: ProductKey) -> Option<&P>;
    pub fn require<P: 'static>(&self, key: ProductKey) -> Result<&P, AssemblyError>;
    /// 装配计划的人类可读摘要：模块、槽、相位、顺序。可打日志、可进 /docs。
    pub fn plan(&self) -> PlanReport;
}
```

### 1.7 框架自带的槽与相位：同一机制，零特权

框架不定义额外机制，只是**自己的 `Slot` 常量用得多一点**：

```rust
// crates/web/src/slots.rs —— 框架注册点，写法与第三方业务模块完全一致
pub static ROUTES: Slot<Routes> = Slot::collect(SlotKey("http.routes"), "HTTP 路由");
pub static MIDDLEWARE: Slot<Middleware> =
    Slot::keyed(SlotKey("http.middleware"), "HTTP 中间件", |m| m.key);
pub static MIGRATIONS: Slot<Box<dyn MigrationTrait>> =
    Slot::collect(SlotKey("db.migrations"), "结构迁移");
pub static SEEDS: Slot<SeedOp> = Slot::keyed(SlotKey("db.seeds"), "种子", |s| s.key);
pub static OPENAPI: Slot<OpenApiFragment> =
    Slot::collect(SlotKey("docs.openapi"), "OpenAPI 片段");
pub static HEALTH: Slot<HealthCheck> = Slot::keyed(SlotKey("ops.health"), "健康检查", |h| h.key);
pub static JOBS: Slot<JobSpec> = Slot::keyed(SlotKey("ops.jobs"), "定时任务", |j| j.key);
pub static EVENTS: Slot<Subscription> = Slot::collect(SlotKey("ops.events"), "领域事件订阅");
pub static METRICS: Slot<MetricSpec> = Slot::collect(SlotKey("ops.metrics"), "指标");
pub static COMMANDS: Slot<CommandSpec> =
    Slot::keyed(SlotKey("cli.commands"), "CLI 子命令", |c| c.key);
pub static DEPT_DIRECTORY: Slot<Arc<dyn DeptDirectory>> =
    Slot::exclusive(SlotKey("port.dept_directory"), "部门目录端口");

/// 产品键。
pub mod products {
    use super::ProductKey;
    pub const ROUTER: ProductKey = ProductKey("http.router");
    pub const DB_PLAN: ProductKey = ProductKey("db.plan");
    pub const OPENAPI: ProductKey = ProductKey("docs.openapi");
    pub const HEALTH: ProductKey = ProductKey("ops.health");
    pub const COMMANDS: ProductKey = ProductKey("cli.commands");
}
```

注意 axum 的痕迹只有一处：`Routes` 这个 newtype 的载荷。

```rust
/// 路由工厂。**载荷**里出现 `axum::Router`；`Slot`/`Module`/`Registrar` 三个契约都不认识 axum。
/// 二期要导 gRPC，只需新增 `pub static GRPC_SERVICES: Slot<GrpcServices>` 与一个相位，
/// 本方案的所有类型一个都不用改——反面草图做不到这一点。
pub struct Routes(pub fn(&Products) -> axum::Router<AppState>);
```

框架自带的相位（同样由组装根声明）：

```rust
// crates/web/src/pipeline.rs
pub const STATE:   PhaseKey = PhaseKey("phase.state");
pub const MW:      PhaseKey = PhaseKey("phase.middleware");
pub const ROUTES:  PhaseKey = PhaseKey("phase.routes");
pub const DOCS:    PhaseKey = PhaseKey("phase.docs");
pub const HEALTHP: PhaseKey = PhaseKey("phase.health");

pub fn install(asm: &mut Assembler, deps: Deps) -> Result<(), AssemblyError> {
    // 连基础设施也走同一条路：`Deps` 被包成一个模块。
    asm.install(&FrameworkDeps::new(deps))?;
    for s in [&ROUTES, &MIDDLEWARE, &MIGRATIONS, &SEEDS, &OPENAPI, &HEALTH,
              &JOBS, &EVENTS, &METRICS, &COMMANDS, &DEPT_DIRECTORY] {
        asm.slot(s)?;
    }
    asm.phase(Phase::collect(&HEALTH, products::HEALTH))?;
    asm.phase(Phase::custom(STATE, build_state))?;
    asm.phase(Phase::custom(DB_PLAN, build_db_plan).after(&[STATE]))?;  // 见 1.9
    asm.phase(Phase::custom(MW, build_layers).after(&[STATE]))?;
    asm.phase(Phase::custom(ROUTES, build_router).after(&[MW, STATE]))?;
    asm.phase(Phase::custom(DOCS, build_docs).after(&[ROUTES]))?;
    Ok(())
}
```

### 1.8 不变量

调用方可以依赖、框架必须保证的性质：

1. **组装根不认识模块内部**：`seal` 只读 `SlotKey`/`ProductKey`/`Merge` 与擦除后的载荷；
   模块对组装根的可见面就是「一个名字 + 若干 (槽, 值)」。
2. **feature 即缺席**：被 `#[cfg]` 裁掉的模块不进 `Vec<Module>`，因此路由、迁移、种子、
   文档、健康检查、任务、事件、指标**同时不出现**——缺席是编译期事实，不需要运行期过滤。
3. **槽键全局唯一**：两个 `Slot` 常量用了同一个 `SlotKey`（哪怕载荷类型不同）即
   `DuplicateSlot`，在 `slot()` 期抛出。
4. **模块名全局唯一**：`DuplicateModule`。
5. **端口独占**：`Merge::Exclusive` 的槽收到第二份贡献即 `SlotConflict`，错误里点名两个模块。
6. **每个被贡献的槽都有消费者**：`UnconsumedSlot`。这是开放注册点最危险的失败模式
   （槽键拼错 ⇒ 贡献静默丢失），本方案把它变成启动期硬失败。
7. **`seal` 之后不可变**：`Assembled` 字段全部私有，外部拿不到半成品；`Registrar` 的生命周期
   绑死在一次 `install` 上，模块无法在装配之后偷加东西。

### 1.9 顺序约束

顺序全部由**相位偏序边**表达，不靠约定，也不靠 `Vec` 的偶然次序：

| 约束 | 表达方式 | 违反时 |
| --- | --- | --- |
| 结构迁移先于种子 | `DB_PLAN` 相位消费 `MIGRATIONS` 与 `SEEDS`，产出**一个** `DbPlan`；`DbPlan::apply` 内部先迁移后种子，调用方无从颠倒 | `DbPlan` 不暴露可分步执行的可变入口 |
| 鉴权中间件先于业务路由 | `ROUTES` 相位 `after(&[MW])`，`build_router` 从 `MW` 产品取已排序的层栈再套到路由上 | 顺序颠倒时 `build_router` 拿不到未产出的产品；`after` 写错则 `seal` 报 `UnknownPhase`/`PhaseCycle` |
| 迁移版本号协商 | `build_db_plan` 按 (模块装入序, 槽内序) 排迁移，重名版本即 `MigrationConflict` | 启动期失败 |
| 种子键冲突（两模块插同一权限标识） | `Merge::Keyed` + `SeedOp::key`，同键同体幂等、同键异体 `SlotConflict` | `seal` 期失败 |

因为相位是**数据**（`Phase { key, after, step }`），`seal` 可以先拓扑排序再执行，
并且把整个计划原样吐给 `Assembled::plan()`，`/docs` 页面与 `cicada explain` 共用同一份事实。

### 1.10 错误模式

全部在**启动期**发生，且必须点名肇事者：

```rust
#[derive(Debug, thiserror::Error)]
pub enum AssemblyError {
    #[error("模块重名：{0:?}")]                          DuplicateModule(ModuleName),
    #[error("槽键重复：{key:?} 被 {a:?} 与 {b:?} 同时声明")]
    DuplicateSlot { key: SlotKey, a: &'static str, b: &'static str },
    #[error("槽冲突（{slot}）：键 {key:?} 由模块 {first:?} 与 {second:?} 各自贡献")]
    SlotConflict { slot: &'static str, key: &'static str, first: ModuleName, second: ModuleName },
    #[error("槽无人消费：{slot:?}（模块 {module:?} 的贡献会静默丢失）")]
    UnconsumedSlot { slot: SlotKey, module: ModuleName },
    #[error("未知相位：{phase:?} 依赖了未声明的 {dep:?}")]
    UnknownPhase { phase: PhaseKey, dep: PhaseKey },
    #[error("相位成环：{0:?}")]                          PhaseCycle(Vec<PhaseKey>),
    #[error("槽未声明：相位 {phase:?} 消费了未声明的槽 {slot:?}")]
    UnknownSlot { phase: PhaseKey, slot: SlotKey },
    #[error("端口缺失：模块 {module:?} 需要 {port}，无人绑定")]
    MissingPort { module: ModuleName, port: &'static str },
    #[error("迁移版本冲突：{module:?} 与 {other:?} 都声明了下标 {version}")]
    MigrationConflict { module: ModuleName, other: ModuleName, version: u32 },
    #[error("相位内部错误：{0}")]                        Step(#[from] StepError),
}
```

---

## 2. 使用示例

### 2.1 组装根（单体 / 模板工程）

```rust
// apps/server/src/main.rs —— 唯一允许读环境变量的地方
fn main() -> anyhow::Result<()> {
    let deps = Deps::from_env()?;
    let mut asm = Assembler::new();

    cicada_web::pipeline::install(&mut asm, deps)?;      // 框架自带的槽与相位

    asm.install(&cicada_system::SystemModule)?;
    asm.install(&cicada_auth::AuthModule)?;
    #[cfg(feature = "monitor")]                          // 裁掉即四处缺席
    asm.install(&cicada_monitor::MonitorModule)?;

    asm.install(&business::ProductModule)?;              // 使用者的模块，与框架模块同路
    crm::install_pipeline(&mut asm)?;                    // 业务侧自带的注册点（见 2.5）
    asm.install(&crm::CrmModule)?;

    let app = asm.seal()?;
    serve(app)                                           // 读 products::ROUTER
}
```

### 2.2 只装配一个模块（微服务）

```rust
// apps/authd/src/main.rs —— 与 2.1 是同一条路径，只是装了更少的模块
fn main() -> anyhow::Result<()> {
    let mut asm = Assembler::new();
    cicada_web::pipeline::install(&mut asm, Deps::from_env()?)?;
    asm.install(&cicada_auth::AuthModule)?;              // 就一个
    let app = asm.seal()?;
    serve(app)
}
```

`system` 侧的 `DEPT_DIRECTORY` 换成 `RemoteDeptDirectory`，**只改一行**：把哪个 adapter 贡献进
那个 exclusive 槽。

### 2.3 测试：只装一个模块，测试面才窄

```rust
#[tokio::test]
async fn create_user_without_perm_is_forbidden() {
    let mut asm = Assembler::new();
    cicada_web::pipeline::install(&mut asm, Deps::for_test(testkit::pg().await?))?;
    asm.install(&cicada_system::SystemModule)?;          // 一个模块
    let app = asm.seal()?;

    let resp = testkit::oneshot(app, post("/system/user").json(body)).await?;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}
```

`Assembled` 就是测试夹具：测试与生产**跨同一条缝**。要测自定义注册点，就地
`asm.phase(Phase::collect(&CRM_RULEBOOK, products::CRM_RULEBOOK))` 即可，不需要起 HTTP。

### 2.4 CLI

```rust
// apps/cli/src/main.rs
let app = assemble()?;                                   // 与 server 共用的装配函数
match cli.command {
    Cmd::Migrate => app.require::<DbPlan>(products::DB_PLAN)?.migrate().await?,
    Cmd::Seed    => app.require::<DbPlan>(products::DB_PLAN)?.seed().await?,
    Cmd::Explain => println!("{}", app.plan().render()), // 自省整条装配计划
    Cmd::Serve   => serve(app)?,
    Cmd::Run(extra) => {
        // 管理员/运维命令同样是一个注册点：模块贡献 CommandSpec，CLI 统一发现。
        let cmds = app.require::<SlotCollect<CommandSpec>>(products::COMMANDS)?;
        dispatch(extra, cmds)                            // 每个模块都能挂自己的子命令
    }
}
```

CLI 不维护第二张命令表——`COMMANDS` 槽就是那张表，框架与业务模块都往它里面放。

### 2.5 模块作者新增一个框架事先不知道的注册点

场景：`business/crm` 想让别的模块登记「优惠券规则」，框架完全没有这个概念。

```rust
// business/crm/src/rules.rs
pub struct CouponRule { pub key: &'static str, pub eval: fn(&Cart) -> bool }

/// ① 声明注册点。一行。框架未参与。
pub static COUPON_RULES: Slot<CouponRule> =
    Slot::keyed(SlotKey("crm.coupon_rules"), "优惠券规则", |r| r.key);

pub static RULEBOOK: ProductKey = ProductKey("crm.rulebook");

/// ② 声明消费它的相位。一行。`after` 把顺序钉死。
pub fn install_pipeline(asm: &mut Assembler) -> Result<(), AssemblyError> {
    asm.slot(&COUPON_RULES)?;                       // 声明槽（便于 UnknownSlot 检查）
    asm.phase(Phase::collect(&COUPON_RULES, RULEBOOK).after(&[cicada_web::pipeline::STATE]))
}

/// ③ 自己也要用：把产品取出来交给自己的路由工厂。
fn crm_routes(products: &Products) -> axum::Router<AppState> {
    let book = products.get::<SlotCollect<CouponRule>>(RULEBOOK).expect("相位保证存在");
    Router::new().route("/crm/coupon/eval", post(eval)).with_state(CrmState::new(book.clone()))
}
```

使用者的第三方 crate（例如「公司内部审计扩展包」）也能这样插进装配，
**组装根只需多调一次它导出的 `install_pipeline`**，框架一行不改。
这就是反面草图做不到的事：在那个方案里，加第五个注册点必须去改 trait。

---

## 3. 实现藏在缝后面的东西

缝在「模块 ⇄ 组装根」之间。缝后面藏着的，调用方一件都不用知道：

1. 类型擦除与 `Arc<dyn Any + Send + Sync>` 的存取（`Contribution` / `Contributions` / `Products`）。
2. 每个槽的合并算法：`Collect` 保序、`Keyed` 去重与幂等判定、`Exclusive` 独占。
3. 贡献的确定性排序键：(模块装入序, 槽内声明序)。
4. 相位图的拓扑排序、环检测、未声明依赖检测。
5. 槽键登记表与「被贡献但无人消费」的静态可判定检查。
6. 冲突与诊断消息的组装：把 `SlotKey`、`kind`、`ModuleName` 拼成人类可读的错误。
7. `AppState` 的构造（`STATE` 相位）：框架自身依赖用具体字段（`db`/`cache`），
   业务产品通过 `Products` 交给各自的 `Routes` 工厂。
8. `DbPlan` 的构造与版本号协商（兼容既有系统的 `sys_*` 表结构由 `system` 模块贡献）。
9. feature 裁剪后的模块集合差异——缝这边看到的只是「`Vec<Module>` 少了几项」。
10. `PlanReport` 的渲染（模块、槽、相位、顺序），供 `/docs` 与 `cicada explain`。

---

## 4. 依赖策略与 adapter

按 DEEPENING.md 的四类逐条说明：

| 类别 | 在本方案中的位置 | 处理 |
| --- | --- | --- |
| **in-process** | `Registry`/`Assembler`/`seal` 本身：无 I/O、无网络，纯构造与校验 | 直接加深。组装根、测试、CLI 三处调用同一个 `seal`，测试可用纯内存对象断言装配结果，不必起服务 |
| **local-substitutable** | 迁移与种子：需要真实 PostgreSQL，但可换本地替身 | 缝在装配器背后。`DbPlan` 是**值**，`plan.migrate()/seed()` 需要 `&Db`；测试用 testcontainers/临时 PG，adapter 就是 `Db` 本身 |
| **remote but owned** | 跨模块端口，如 `DEPT_DIRECTORY`（拆服务后是公司自己的另一个服务） | **定义端口**（`trait DeptDirectory`），单体用 `LocalDeptDirectory`、拆分后用 `RemoteDeptDirectory`、测试用内存实现。切换点只有一行：谁往那个 exclusive 槽贡献 adapter |
| **true external** | 文件存储（S3/OSS/MinIO）、SMTP 邮件、验证码服务 | 同样是槽：`Slot<Arc<dyn Storage>>` 等 exclusive 槽，生产用真实 adapter，测试用 mock adapter |

缝纪律：本方案**只有一个缝**（模块 ⇄ 组装根），但缝上有**两类 adapter 角色**——
一类是「贡献的 adapter」（谁往槽里放什么），一类是「消费的 adapter」（相位怎么把槽收成产品）。
每一个 exclusive 端口槽都天然有两个 adapter（生产 / 测试），因此是真缝而非假缝。

---

## 5. 取舍

**杠杆高在哪。** 一份 `Slot` + `Registrar` + `Phase` 覆盖了全部注册点：框架的十一个槽、
第三方业务模块的自定义槽、端口绑定、CLI 子命令，是同一套类型。新增注册点 = 一行 `static`
常量 + 一个 `Phase::collect`，**不碰任何 trait，不牵动任何既有实现**。深度也来自此：
调用方学的接口极小（写入只用 `contribute` 一个方法），背后的行为很大（合并语义、排序、
跨模块校验、文档汇总、CLI 发现）。

**哪里薄。** 三处：

1. `Slot<T>` 要求 `T: Send + Sync + 'static`，贡献必须是**值**。这排除了借用数据与需要
   运行期闭包捕获的注册点。对「声明式事实」（路由工厂、迁移、种子、端口）正好；
   对「真的要闭包」的场景（例如动态中间件），只能退化成 fn 指针或再加一层 trait 对象。
2. 相位是运行期调度的，**阅读者看不出执行顺序**，除非调用 `plan()`。局部性因此有损：
   「什么时候发生」散在若干 `install_pipeline` 里。缓解手段是把整个计划做成可打印产物
   （`Assembled::plan()`、`/docs`、`cicada explain`），但这是补救，不是根治。
3. 槽键是字符串常量，**拼错不会被编译器发现**。`UnconsumedSlot` + `UnknownSlot` 两条检查
   把绝大多数笔误变成启动期失败，但它终究是检查而不是类型——比 typestate 方案弱一档。

**最容易失败的地方。**

- **过度泛化。** 灵活到极致时，第三方会开始用槽「发明第二个框架」，出现
  `http.routes.v2`、`crm.routes` 这类与框架槽地位重叠的自造注册点。对策是硬规则：
  *一个槽只有配上消费相位才有意义*（`UnconsumedSlot` 强制），且框架在文档里公布
  「应当复用哪些框架槽、什么情况下才新开槽」。
- **类型复杂度与编译时间。** 擦除层（`dyn Any`、`Arc`、fn 指针）本身很轻，真正花钱的是
  **单二进制装进所有模块**：feature 组合越多、`AppState` 越大，泛型实例化越多。
  本方案不额外放大它，但也无法抵消它——这是形态 B 的固有成本。
- **错误信息可读性。** 槽冲突若只报 `SlotKey` 会让人抓狂；因此每个 `Slot` 必须带 `kind`
  与贡献模块名，错误消息统一由装配器拼装。**这是本方案唯一不能省的设计纪律**：
  没有 `kind` 和模块归属，这套机制在出问题时不可用。
- **与编译期强制方案的分工。** 漏做数据范围过滤那种「安全必须编译期」的场景，本方案
  只能靠检查而非类型；那种场景应当交给 typestate 方案，而不是硬塞进这里。
