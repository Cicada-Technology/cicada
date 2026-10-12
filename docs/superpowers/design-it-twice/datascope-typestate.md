# 方案：数据范围接口 · 编译期强制（typestate / 类型状态）

> 设计约束：**把「漏做数据范围过滤」变成编译错误**。核心机制必须是类型层面的——不是运行期断言、不是日志告警、不是 code review 纪律。合法的不加过滤场景必须**显式可见、可审计**。
>
> 术语按 `codebase-design`：**模块 / 接口 / 缝 / adapter / 深度 / 杠杆 / 局部性**。
> 缝的位置：`service / repository ⇄ 具体实体与查询`。

本方案的核心判断是：**「已经过滤过」应当是一个类型的证据（evidence），而不是一次调用的副作用。**
草图 `Scope::apply(q) -> Select<E>` 的问题是——它加了条件，但没人能证明这件事发生过：`Select<E>` 仍然可以由 `E::find()` 凭空产生，也仍然可以直接 `.all()` 执行。

所以这里不设计「一个更好的 apply」，而是把查询做成一个**状态机**：未经升格的查询在类型上**没有终结算子**；升格到 `Scoped` 的唯一通道是 `Scope`；而那条通道要求实体**先声明自己的归属维度**——声明缺失时通道不存在。「过滤没做」于是不再是运行期行为，而是「`all` 这个方法找不到」。

---

## 1. 接口

### 1.1 领域值：`DataScope` / `Scope`（声明式、可序列化）

```rust
// crates/kernel/src/scope/value.rs
// 纯值：无闭包、无 SQL、无连接。可进 JWT claim、可随跨服务调用传递（见 §4）。

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TenantId(i64);
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct UserId(i64);
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DeptId(i64);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DataScopeKind { All, Custom, Dept, DeptAndChild, SelfOnly, None }

/// 声明式范围对象。**只由 `rbac` 构造**，消费侧只读。
///
/// `DeptAndChild` 的后代已经在 `rbac` 侧展开进 `dept_ids`（见 §4），
/// 因此消费侧不需要跨模块 JOIN，也不需要递归 CTE —— 条件构造是 in-process 的纯计算。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DataScope {
    pub kind: DataScopeKind,
    /// Custom / Dept / DeptAndChild 时生效。
    pub dept_ids: Vec<DeptId>,
    /// DeptAndChild：部门 ancestors 前缀（声明形式，与设计文档 §6.3 一致）。
    pub subtree_paths: Vec<String>,
    /// SelfOnly 时生效。
    pub user_id: UserId,
}

/// 请求级窄接口：持久层只认识「哪个租户、谁、能看多大」，不认识 HTTP。
/// `user_id == None` 表示系统任务（定时任务 / 迁移）；此时 `rbac` 不签发 `SelfOnly`
/// 范围（而是 `None` 或 `All`），因此消费侧无需区分「无主」的 SelfOnly。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Scope {
    pub tenant_id: TenantId,
    pub user_id: Option<UserId>,
    pub data_scope: DataScope,
}

#[cfg(any(test, feature = "testkit"))]
impl Scope {
    /// 测试夹具：直接构造，不经过任何 `RequestContext`（约束 6）。
    pub fn for_test(tenant: TenantId, user: UserId, depts: impl Into<Vec<DeptId>>) -> Self { /* ... */ }
}
```

### 1.2 实体声明：`DataScoped` + 类型级 `Shape`

实体用**一个属性 + 一次 derive**声明自己的归属维度（约束 2：声明成本 ≈ 一行）。关键是声明位置不是一个 `Option<Column>`，而是一个**类型**：

```rust
// crates/kernel/src/scope/entity.rs
use sea_orm::EntityTrait;
use sea_query::Condition;

mod sealed { pub trait Sealed {} }

/// 一次范围判定的结果。把「拒答」做成一个**值**而不是 `None`，
/// 是修草图第一处疑点（`Option` 无法区分「没有这一列」与「有意不过滤」）的关键。
pub enum ShapeOutcome<C> {
    /// 叠加这个条件。
    Cond(C),
    /// 对本实体，该范围不构成限制（整租户可见）。
    TenantWide,
    /// 对本实体，该范围**无法表达** —— 失败关闭，绝不放行。
    Denied,
}

/// 类型即声明：四种实体形态 + 一种显式豁免。sealed，外部不能新增。
pub trait Shape: sealed::Sealed + Send + Sync + 'static {
    /// 给定范围种类与实体的列，判定归属条件。判定逻辑**只写在这里一次**（局部性）。
    fn judge<E: DataScoped>(
        scope: &DataScope,
        dept: Option<E::Column>,
        owner: Option<E::Column>,
    ) -> ShapeOutcome<Condition>;
}

pub struct DeptAndOwner;  // 有部门列 + 有归属人列
pub struct DeptOnly;      // 只有部门列
pub struct OwnerOnly;     // 只有归属人列
pub struct NoOwnership;   // 两者都没有 —— 部门/本人范围一律 Denied
pub struct Exempt;        // **有意放行**归属过滤 —— 部门/本人范围一律 TenantWide（须写明理由）

/// 由 `#[derive(DataScoped)]` 唯一实现（trait 在 kernel 内 sealed）。
/// **没有本 impl 的实体，无法进入任何受控查询路径** —— 见 §1.3 与 §5 的关键考点。
pub trait DataScoped: EntityTrait {
    type Shape: Shape;

    fn tenant_column() -> Self::Column;
    fn soft_delete_column() -> Option<Self::Column>;
    /// 仅当 `Shape ∈ {DeptAndOwner, DeptOnly}` 时返回 `Some`。
    fn dept_column() -> Option<Self::Column>;
    /// 仅当 `Shape ∈ {DeptAndOwner, OwnerOnly}` 时返回 `Some`。
    fn owner_column() -> Option<Self::Column>;
}

impl Shape for DeptAndOwner {
    fn judge<E: DataScoped>(scope: &DataScope, dept: Option<E::Column>, owner: Option<E::Column>)
        -> ShapeOutcome<Condition>
    {
        // 两列由同一个宏按**属性名**生成，`Shape` 与列访问器不可能不一致（边界见 §5）。
        let dept  = dept.expect("DeptAndOwner 由宏保证提供 dept_column");
        let owner = owner.expect("DeptAndOwner 由宏保证提供 owner_column");
        match scope.kind {
            DataScopeKind::All  => ShapeOutcome::TenantWide,
            DataScopeKind::None => ShapeOutcome::Denied,
            DataScopeKind::Custom | DataScopeKind::Dept | DataScopeKind::DeptAndChild =>
                ShapeOutcome::Cond(dept.is_in(scope.dept_ids.iter().map(|d| d.get()))),
            DataScopeKind::SelfOnly =>
                ShapeOutcome::Cond(owner.eq(scope.user_id.get())),
        }
    }
}

// DeptOnly：SelfOnly 没有归属人列可依 => Denied。
// OwnerOnly：Custom/Dept/DeptAndChild 没有部门列可依 => Denied。
// NoOwnership：All => TenantWide，其余 => Denied。
// Exempt：每种 kind => TenantWide。这是「有意放行不过滤」，与 NoOwnership 是**两个不同的类型**。
```

实体的声明语法（宏**只认这四种形态之一**，没有「什么都不写」这个合法输入）：

```rust
// business/src/product/entity.rs  —— 使用者的业务实体
#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "biz_product")]
#[data_scope(tenant = Column::TenantId, dept = Column::DeptId, owner = Column::CreateBy,
             deleted = Column::DeletedAt)]   // ← 归属维度声明：一行
pub struct Model { /* ... */ }

// 派生结果（节选）：type Shape = DeptAndOwner;
// tenant_column() => Column::TenantId; dept_column() => Some(Column::DeptId);
// owner_column() => Some(Column::CreateBy); soft_delete_column() => Some(Column::DeletedAt);
```

`#[derive(DataScoped)]` 的**输入校验**（本方案的关键收口）：

| 属性 | 生成的 `Shape` |
| --- | --- |
| `dept = .., owner = ..` | `DeptAndOwner` |
| `dept = ..` | `DeptOnly` |
| `owner = ..` | `OwnerOnly` |
| 都没有 | **`compile_error!`**：必须改成 `no_ownership` 或 `exempt(reason = "…")` |
| `no_ownership` | `NoOwnership` |
| `exempt(reason = "…")` | `Exempt`（宏把 reason 写进 `Exempt::REASON`） |

「两者都没有」与「有意放行不过滤」因此是**两个不同的类型**（`NoOwnership` / `Exempt`），并且都必须由作者显式写出；什么都不写则直接编译失败。列名写错（`Column::DepID`）同样是编译错误：列的枚举来自实体，运行期字符串列名无处可藏（约束 7）。

### 1.3 查询状态机：`Query<E, S>`

```rust
// crates/kernel/src/scope/query.rs
use std::marker::PhantomData;
use sea_orm::{EntityTrait, Select, ConnectionTrait, QueryFilter, QueryOrder, QuerySelect, PaginatorTrait};
use sea_query::{Condition, Expr, IntoCondition};

mod state { pub trait Sealed {} }

/// 查询的生命周期状态。`Sealed` + 私有字段 => **不可伪造**：外部 crate 造不出 `Scoped`。
pub trait QueryState: state::Sealed + Send + 'static {}
pub struct Unchecked;    // 尚未受控：**没有终结算子**
pub struct Scoped;       // 租户 + 软删 + 归属条件已叠加
pub struct TenantBound;  // 只叠加租户 + 软删：归属过滤被显式豁免
pub struct Global;       // 不叠加任何条件：仅限跨租户管理

impl state::Sealed for Unchecked {}   impl QueryState for Unchecked {}
impl state::Sealed for Scoped {}      impl QueryState for Scoped {}
impl state::Sealed for TenantBound {} impl QueryState for TenantBound {}
impl state::Sealed for Global {}      impl QueryState for Global {}

/// 受控查询。字段私有：只有 `kernel` 内部能构造。
pub struct Query<E: EntityTrait, S: QueryState> {
    select: Select<E>,
    _state: PhantomData<S>,
}

impl<E: EntityTrait> Query<E, Unchecked> {
    /// 查询的**唯一**起点（业务代码不用 `E::find()`）。
    pub fn open() -> Self { Query { select: E::find(), _state: PhantomData } }
}

impl<E: EntityTrait, S: QueryState> Query<E, S> {
    // 非终结算子：保留状态，链式写法不受影响（约束 7）。
    pub fn filter<F: IntoCondition>(mut self, f: F) -> Self { self.select = self.select.filter(f); self }
    pub fn filter_if(mut self, on: bool, f: impl FnOnce(Self) -> Self) -> Self {
        if on { f(self) } else { self }
    }
    pub fn order_by<C: Into<E::Column>>(mut self, c: C) -> Self {
        self.select = self.select.order_by_asc(c); self
    }
    pub fn order_by_desc<C: Into<E::Column>>(mut self, c: C) -> Self {
        self.select = self.select.order_by_desc(c); self
    }
    pub fn limit(mut self, n: u64) -> Self { self.select = self.select.limit(n); self }
    pub fn offset(mut self, n: u64) -> Self { self.select = self.select.offset(n); self }
}

impl<E: DataScoped> Query<E, Unchecked> {
    /// 唯一把 `Unchecked` 升格成 `Scoped` 的通道。
    /// **要求 `E: DataScoped`** —— 未声明归属维度的实体，到这里就没有可调的方法。
    pub fn scoped(mut self, scope: &Scope) -> Query<E, Scoped> {
        self.select = scope.apply::<E>(self.select);
        Query { select: self.select, _state: PhantomData }
    }
}

/// 「确实不加归属过滤」的合法场景。**必须显式表态**，且理由的**类型**决定升格后的状态类型。
pub trait UnscopedReason: Send + Sync + 'static {
    type State: QueryState;
    const LABEL: &'static str;
}
/// 按主键取单条（调用方已持有 id，通常来自一次受控列表）。
pub struct ByPrimaryKey;
impl UnscopedReason for ByPrimaryKey { type State = TenantBound; const LABEL: &'static str = "by_primary_key"; }
/// 定时任务 / 运维脚本：单租户内运行。
pub struct InternalTask;
impl UnscopedReason for InternalTask { type State = TenantBound; const LABEL: &'static str = "internal_task"; }
/// 跨租户管理：**连租户条件都不要**。最危险的一格，因此是**独立类型**、可 grep、可计数。
pub struct CrossTenant;
impl UnscopedReason for CrossTenant { type State = Global; const LABEL: &'static str = "cross_tenant"; }

impl<E: EntityTrait> Query<E, Unchecked> {
    /// 显式豁免。返回类型 `Query<E, R::State>` 由理由类型决定：
    /// 调用点因此同时说出了「豁免了什么」与「保住了什么」。
    pub fn unscoped<R: UnscopedReason>(mut self, _reason: R) -> Query<E, R::State> {
        tracing::warn!(reason = R::LABEL, entity = E::table_name(), "unscoped query");
        Query { select: self.select, _state: PhantomData }
    }
}

/// 终结算子**只**在这三个状态上存在。`Unchecked` 没有 `all`/`one`/`count`/`fetch_page`。
macro_rules! executable {
    ($($S:ty),* $(,)?) => { $(
        impl<E: EntityTrait> Query<E, $S> {
            pub async fn all<C: ConnectionTrait>(self, db: &C) -> Result<Vec<E::Model>, DbErr> {
                self.select.all(db).await
            }
            pub async fn one<C: ConnectionTrait>(self, db: &C) -> Result<Option<E::Model>, DbErr> {
                self.select.one(db).await
            }
            pub async fn count<C: ConnectionTrait>(self, db: &C) -> Result<u64, DbErr> {
                self.select.count(db).await
            }
            pub async fn fetch_page<C: ConnectionTrait>(self, db: &C, page: u64, page_size: u64)
                -> Result<Page<E::Model>, DbErr>
            {
                let p = self.select.paginate(db, page_size);
                let total = p.num_items().await?;
                let items = p.fetch_page(page.saturating_sub(1)).await?;
                Ok(Page { items, total, page, page_size })
            }
        }
    )* };
}
executable!(Scoped, TenantBound, Global);
```

### 1.4 作用域绑定的执行句柄与 RLS 注入点

约束 8 问的是「RLS 会话变量由谁、在什么位置注入」。类型上给它一个明确的归属：只有 `Db::begin_scoped(&scope)` 能造出 `ScopedDb`，那里就是**唯一注入点**；而受控查询只接受 `ScopedDb`。

```rust
// crates/infra/src/db/scoped.rs
use sea_orm::{DatabaseTransaction, ConnectionTrait, DatabaseBackend, Statement, TransactionTrait};

/// 一个事务 + 它所属的 `Scope`。构造入口唯一，RLS 会话变量在此注入。
pub struct ScopedDb { txn: DatabaseTransaction, scope: Scope }
impl ScopedDb {
    pub fn scope(&self) -> &Scope { &self.scope }
}
#[async_trait::async_trait]
impl ConnectionTrait for ScopedDb { /* 全部转发到 self.txn */ }

impl Db {
    /// service 层调用（repository 不自己开事务，见设计文档 §5.1）。
    pub async fn begin_scoped(&self, scope: &Scope) -> Result<ScopedDb, DbErr> {
        let txn = self.conn.begin().await?;
        // 参数化注入，不做字符串拼接（设计文档：全程 sea_query 绑定）。
        // 值来自新建类型 ID 与枚举，不接受用户输入。
        let stmt = Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT set_config('app.tenant_id',    $1, true),
                    set_config('app.current_user_id', $2, true),
                    set_config('app.dept_ids',        $3, true),
                    set_config('app.data_scope',      $4, true)",
            [ scope.tenant_id.get().to_string().into(),
              scope.user_id.map(|u| u.get()).unwrap_or(0).to_string().into(),
              scope.data_scope.dept_ids.iter().map(|d| d.get().to_string())
                   .collect::<Vec<_>>().join(",").into(),
              scope.data_scope.kind.as_str().into() ],
        );
        txn.execute(stmt).await?;
        Ok(ScopedDb { txn, scope: scope.clone() })
    }
}
```

`SET LOCAL` 是事务级 GUC，随事务回滚自动消失，不污染连接池 —— 这正是把注入点放在事务起点而非查询点的原因。

### 1.5 不变量、顺序约束、错误模式

**不变量（由类型保证）**

1. 任何 `Query<E, Scoped>` 值的存在，即证明 `E: DataScoped` 且已叠加「租户 + 软删 + 归属」三类条件。
2. 任何 `Query<E, Global>` 值的存在，即证明调用点显式写了 `CrossTenant`（可 grep、可计数）。
3. `Scoped` / `TenantBound` / `Global` 不可伪造：字段私有，trait sealed，构造点全在 kernel 内。

**顺序约束**

```
Query::<E>::open()  ->  (filter | order_by | limit | offset)*
                    ->   scoped(&scope)  |  unscoped(R)
                    ->  (filter | order_by | limit | offset)*
                    ->  all | one | count | fetch_page
```

`scoped` / `unscoped` 各只能调用一次：`Scoped`、`TenantBound`、`Global` 上没有这两个方法。在升格**之后**仍可继续 `filter`（状态不变），所以「先加业务条件、后套范围」与「先套范围、后加业务条件」都成立。

**范围种类的覆盖（约束 4）**

| `DataScopeKind` | `DeptAndOwner` | `DeptOnly` | `OwnerOnly` | `NoOwnership` | `Exempt` |
| --- | --- | --- | --- | --- | --- |
| `All` | 租户 | 租户 | 租户 | 租户 | 租户 |
| `Custom` | `dept IN ids` | `dept IN ids` | **拒绝** | **拒绝** | 租户 |
| `Dept` | `dept = 本部门` | `dept = 本部门` | **拒绝** | **拒绝** | 租户 |
| `DeptAndChild` | `dept IN ids`（已展开后代） | 同左 | **拒绝** | **拒绝** | 租户 |
| `SelfOnly` | `owner = 我` | **拒绝** | `owner = 我` | **拒绝** | 租户 |
| `None` | 拒绝 | 拒绝 | 拒绝 | 拒绝 | **拒绝** |

「拒绝」= 查询被加上 `WHERE false`，并打一条 `warn`。**失败关闭**，不是放行。

**错误模式**

- DB 失败：`DbErr`（透出 SeaORM 错误）。
- 归属维度缺失（`ShapeOutcome::Denied`）：不是 `Err`，而是 `WHERE false` + `warn`。语义上「这个角色看不到这类数据」是正常结果，但配置错误（例如给 `OwnerOnly` 实体配了 `Dept` 范围）必须在日志里可见 —— 见 §5「静默失效边界」。
- 未升格就执行：**编译错误**，见 §2.5。

---

## 2. 使用示例

### 2.1 repository 的 list 方法

```rust
// business/src/user/repository.rs
use cicada::kernel::scope::Query;
use sea_orm::Condition;

pub async fn page_users(db: &ScopedDb, p: PageUsers) -> Result<Page<UserVo>, DbErr> {
    let q = Query::<sys_user::Entity>::open()
        .filter_if(p.status.is_some(), |q| {
            q.filter(sys_user::Column::Status.eq(p.status.unwrap()))
        })
        .filter_if(p.keyword.is_some(), |q| {
            let kw = format!("%{}%", p.keyword.clone().unwrap());
            q.filter(Condition::any()
                .add(sys_user::Column::UserName.ilike(kw.clone()))
                .add(sys_user::Column::NickName.ilike(kw)))
        })
        .scoped(db.scope())                             // ← Unchecked → Scoped
        .order_by_desc(sys_user::Column::UserId);        // 状态不变，仍可继续拼

    Ok(q.fetch_page(db, p.page, p.page_size).await?.map(UserVo::from))
}
```

调用方要学两件事：**用 `Query::open()` 起手**，**执行前 `.scoped(db.scope())`**。`filter` / `order_by` / `limit` 与 SeaORM 写法一致 —— `Query<E, S>` 只是非终结算子的一层薄封装，返回 `Self`，因此不打断链式调用，也没有 `Select<E>` 的直接终结算子可用。

### 2.2 DB 测试

```rust
#[tokio::test]
async fn dept_and_child_sees_only_subtree() {
    let db = TestDb::new().await;          // testcontainers 起真 PG（local-substitutable，见 §4）
    db.seed_fixture("user_tree").await;    // 部门树 + 若干用户

    // 直接构造 Scope，不构造任何 HTTP / RequestContext（约束 6）。
    let scope = Scope::for_test(TenantId(1), UserId(7), [DeptId(3), DeptId(4)]);
    let tx = db.begin_scoped(&scope).await.unwrap();

    let rows = page_users(&tx, PageUsers::default()).await.unwrap();
    assert_eq!(ids(&rows), [7, 8, 9]);      // 断言结果集，不比对 SQL 字符串
    tx.rollback().await.unwrap();
}
```

测试只需要「一个 `Scope` 值 + 一个 `ScopedDb`」，不需要任何 HTTP 上下文 —— 这正是窄接口 `Scope` 换来的测试面收窄。

### 2.3 RLS 作为兜底防线（约束 8）

```sql
ALTER TABLE sys_user ENABLE ROW LEVEL SECURITY;               -- 默认关闭，按表开启
CREATE POLICY tenant_isolation ON sys_user
  USING (tenant_id = current_setting('app.tenant_id')::bigint);
```

因为受控读只走 `ScopedDb`，而 `ScopedDb` 必然注入过 GUC，所以即使应用层的归属条件算错，跨租户数据也不会外泄。若某张表要按部门兜底，策略可读 `app.dept_ids`：

```sql
CREATE POLICY dept_isolation ON biz_product
  USING (tenant_id = current_setting('app.tenant_id')::bigint
     AND dept_id = ANY (string_to_array(
           NULLIF(current_setting('app.dept_ids', true), ''), ',')::bigint[]));
```

不设 `app.dept_ids` 的策略直接不写部门谓词，绝不写「取不到就当空串匹配」这类默认放行。RLS 的开关是 per-table 的，注入点始终只有 `begin_scoped` 一处。

### 2.4 跨服务的部门目录（remote but owned）

`DeptAndChild` 的后代展开需要部门树，而部门树可能属于另一个服务。按 DEEPENING.md 的依赖分类，这属于「remote but owned」——在缝上给一个**端口**，配多个 adapter：

```rust
// crates/ports/src/dept.rs
#[async_trait::async_trait]
pub trait DeptDirectory: Send + Sync {
    async fn descendants(&self, tenant: TenantId, root: DeptId) -> Result<Vec<DeptId>>;
    async fn path(&self, tenant: TenantId, dept: DeptId) -> Result<String>;
}

pub struct LocalDeptDirectory     { db: Db }                         // 单体：直连本地 sys_dept
pub struct RemoteDeptDirectory    { http: HttpClient, cache: Cache }  // 微服务：HTTP + 本地缓存
pub struct InMemoryDeptDirectory  { /* 测试夹具 */ }                  // 测试
```

**跨服务传递的是已经展开的 `DataScope`，不是「请对方再问一次部门树」。** `rbac` 用 `DeptDirectory::descendants` 把 `DeptAndChild` 算成 `dept_ids` 放进 token / 头；业务服务只消费这张列表，条件构造仍是 in-process 的纯计算。这同时满足约束 5（可序列化、跨服务）与约束 9（远程依赖有端口 + adapter）。

### 2.5 把机制钉住的编译失败测试（约束 1 的证据）

类型保证只有在**测试锁住**时才长期成立。用 `trybuild` 断言两类错误：

```rust
// business/tests/ui.rs
#[test]
fn typestate_holds() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/ui/query_without_scope.rs");       // 未升格就执行
    t.compile_fail("tests/ui/entity_without_data_scope.rs"); // 实体忘了声明
}
```

`tests/ui/query_without_scope.rs`（预期 stderr，可读性本身也是接口的一部分）：

```
error[E0599]: no method named `all` found for struct `Query<sys_user::Entity, Unchecked>`
  --> business/tests/ui/query_without_scope.rs:9:8
   |
 9 |     Query::<sys_user::Entity>::open().all(&tx).await?;
   |                                      ^^^
   |
   = help: 查询必须先升格才能执行：
             `.scoped(db.scope())`                          —— 租户 + 用户数据
             `.unscoped(ByPrimaryKey)` / `.unscoped(InternalTask)`
                                                            —— 显式豁免归属过滤（会记日志）
             `.unscoped(CrossTenant)`                       —— 跨租户（独立类型，可审计）
```

`tests/ui/entity_without_data_scope.rs`（实体忘了声明归属维度）：

```
error[E0277]: the trait bound `sys_dict_data::Entity: DataScoped` is not satisfied
   |
   = note: 实体 `sys_dict_data` 没有声明归属维度，因此不能进入受控查询路径。
           若它确实有部门/归属人列，请写 `#[data_scope(tenant = …, dept = …)]`；
           若它结构上就没有这两种列，请写 `#[data_scope(tenant = …, no_ownership)]`；
           若它有意不做归属过滤，请写 `#[data_scope(tenant = …, exempt(reason = "…"))]`。
   = help: 这是一个安全接口：默认路径是拒绝，而不是放行。
```

---

## 3. 实现藏在缝后面的东西

缝在「service / repository ⇄ 具体实体与查询」之间。调用方与测试跨过的接口面很小
（`Query::open` / `.scoped` / `.unscoped` + 三个终结算子 + 一行 `#[data_scope(..)]`），
但这条缝后面由 kernel 一手包办、并且类型上禁止绕过的机制有：

1. **typestate 状态机本身**：`Query<E, S>` 的 `PhantomData<S>`、sealed 的 `QueryState` /
   `Shape`、私有字段。`Unchecked` 没有终结算子，`Scoped` / `TenantBound` / `Global` 不可伪造——
   外部 crate 既造不出半升格的查询，也拿不到 `Select<E>` 直接执行。
2. **三类条件的构造**：租户 + 软删 + 归属维度，由 `Shape::judge` 只实现一次；调用点永远只写
   `.scoped(&scope)`，不写任何列名、不写任何 `Condition`。
3. **`#[derive(DataScoped)]` 的收口**：`tenant_column` / `dept_column` / `owner_column` /
   `soft_delete_column` 与 `type Shape` 由同一属性名生成，因此「`Shape` 说两列都有」与
   「列访问器返回 `Some`」不可能脱节；`no_ownership` / `exempt(reason)` 是显式的两个不同类型。
4. **范围判定矩阵**：四种/五种 `Shape` × 六种 `DataScopeKind` → `ShapeOutcome`
   （`Cond` / `TenantWide` / `Denied`）。`Denied` 转成 `WHERE false` 并打 `warn`，失败关闭。
5. **RLS 会话变量的唯一注入点**：`Db::begin_scoped(&scope)` 里 `set_config` 写
   `app.tenant_id` / `app.current_user_id` / `app.dept_ids` / `app.data_scope`，用 `SET LOCAL`
   随事务回滚自动消失；受控读只走 `ScopedDb`，应用层即使算错也有数据库层兜底。
6. **分页的两段查询**：`fetch_page` 内部先 `num_items()` 再 `fetch_page`，`page` 从 1 开始、
   对 SeaORM 的 0 基 `fetch_page` 做 `saturating_sub(1)`，把差异藏在缝后。
7. **豁免的记账**：`unscoped(R)` 的三态（`ByPrimaryKey` / `InternalTask` / `CrossTenant`）由
   理由的类型决定升格后的状态，`CrossTenant` 是独立类型，可 grep、可计数、必打 `warn`。
8. **编译失败测试**：`trybuild` 把「未升格就执行」「实体未声明归属维度」钉成编译错误，
   保证这条安全接口的默认路径是拒绝。

用删除测试检验：若删掉这套 `Query<E, S>`，上面 1–4、6、7 的逻辑会重新散落到每个 repository、
每个 service、每个测试里，且「漏过滤」重新变回 review 纪律而不是编译错误——所以它值得留下。

## 4. 依赖策略与 adapter

按 DEEPENING.md 的四类依赖逐条判定：

| 本接口涉及的依赖 | 分类 | 处理 |
| --- | --- | --- |
| 查询状态机与条件构造（`Query<E,S>`、`Shape::judge`） | **in-process** | 纯类型 + 纯计算，无 I/O。直接单测：给一个 `Scope`，断言 `ShapeOutcome` 或最终结果集，不需要任何 adapter |
| 落库执行（迁移 / 种子 / 读查询） | **local-substitutable** | 真实 PostgreSQL（testcontainers）或有替身时用替身；`ScopedDb` 是内部缝，不提升为端口。测试用 `TestDb` + `Db::begin_scoped` 直接打同一接口 |
| 部门目录跨服务取数（`DeptAndChild` 的后代展开） | **remote but owned** | 定义 `DeptDirectory` 端口，配 `LocalDeptDirectory`（单体直连）/ `RemoteDeptDirectory`（微服务 HTTP + 缓存）/ `InMemoryDeptDirectory`（测试）。`rbac` 在展开后把 `dept_ids` 放进 `DataScope`，业务服务只消费结果 |
| 真外部（无） | —— | 本接口不触达任何三方服务（对象存储、SMTP 等属运行时端口，不经过这条查询缝） |

缝纪律结论：这条查询缝本身只有一个形态（受控查询状态机），它不满足「两个 adapter 才是真缝」，
因此 `Query<E, S>` / `DataScoped` / `Shape` 不是端口，留在 `kernel`。真正配得上端口的只有
`DeptDirectory`（remote but owned，有本地/远程/内存三种 adapter），它落在 `ports`，与查询缝正交。
这也正是本方案与设计文档 §6.3 的关系：`DataScope` / `Scope` 的字段与语义对齐 §6.3，但把 §6.3 里
「`Scope::apply(q) -> Select<E>` 加了条件却无人能证明」的缝，替换成「类型上无法跳过」的缝。

## 5. 取舍

**杠杆高在哪。**

一行 `#[data_scope(tenant = …, dept = …, owner = …, deleted = …)]` + 一次 `.scoped(&scope)`，
换来「租户 + 软删 + 归属」三类过滤在编译期被证明已做；`Denied` 失败关闭、RLS 兜底、
`unscoped` 的豁免必须显式表态且可审计。调用方学的接口极小（`open` / `scoped` / `unscoped` +
`all/one/count/fetch_page`），背后的行为很大（状态机 + 判定矩阵 + GUC 注入 + 分页 + 记账）。
默认路径是拒绝，而不是放行——这是它相对设计文档 §6.3 `apply` 的核心增益。

**薄的地方。**

1. `Query<E, S>` 是 `Select<E>` 的一层薄封装：SeaORM 每新增一个非终结算子（新的 `filter` /
   `order_by` 变体、`group_by`、`having`……），这里就要同步补一层 wrapper，否则链式表达力落后于
   上游。这是跟随成本，不是一次性成本。
2. 第二个泛型参数 `S` 会传染：任何把 `Query<E, S>` 当参数/返回值的函数都要带上状态参数，
   签名变长。好在它只在 repository 内部出现，不越出窄接口 `Scope`。
3. `Shape` 是 sealed 的：业务侧只能四选一（`DeptAndOwner` / `DeptOnly` / `OwnerOnly` /
   `NoOwnership`）+ 一个 `Exempt`，不能自定义第六种实体形态。这换来了「判定逻辑只写一次」，
   代价是罕见的多维归属要退回 `unscoped` 逃生通道或手写 `Condition`。

**最容易失败的地方。**

- **`Denied → WHERE false + warn` 是运行期兜底**。给 `OwnerOnly` 实体配了 `Dept` 范围、或给
  `NoOwnership` 实体误配了部门/本人范围时，系统不会报错，只会静默返回空集 + 一条 `warn`。
  语义上「这个角色看不到这类数据」是正常结果，但配置错误必须靠覆盖矩阵测试与告警兜住，
  否则会退化成「静默漏数据」。这是 typestate 方案里唯一一处「编译期保证不了、只能靠检查」的地方。
- **整个保证建立在「`DataScoped` 只能由 derive 实现」上**。一旦有人手写 `impl DataScoped`，
  或经 `DatabaseConnection` 逃到裸 `sqlx::PgPool` 直接 `SELECT`，类型保证即刻失效。因此
  `Shape` / `QueryState` 必须 sealed、`Select<E>` 必须私有、sqlx 逃生必须像设计文档 §9.3 那样被
  CI lint 圈进白名单文件。
- **编译错误可读性是接口的一部分**。§2.5 里 `trybuild` 的 stderr 帮助文本是这条接口对作者
  的「文档」，必须随 SeaORM 版本与 API 演进同步维护，否则「找不到 `all` 方法」这类报错会
  让后来者一头雾水。
