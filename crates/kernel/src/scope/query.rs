//! 查询状态机 `Query<E, S>`（datascope-typestate.md ）。
//!
//! 「已经过滤过」是类型的证据，而不是一次调用的副作用：未升格的查询
//! `Query<E, Unchecked>` **没有终结算子**，升格到 `Scoped` 的唯一通道是
//! `.scoped(&scope)`，而那条通道要求实体先 `DataScoped` 声明归属维度。

use std::marker::PhantomData;

use sea_orm::{
    ColumnTrait, ConnectionTrait, DbErr, EntityTrait, IntoSimpleExpr, PaginatorTrait, QueryFilter,
    QueryOrder, QuerySelect, Select,
};
use sea_query::{Condition, Expr, IntoCondition};

use crate::page::Page;
use crate::scope::entity::{DataScoped, Shape, ShapeOutcome};
use crate::scope::value::Scope;

mod state {
    pub trait Sealed {}
}

/// 查询的生命周期状态。`Sealed` + 私有字段 => 不可伪造。
pub trait QueryState: state::Sealed + Send + 'static {}

/// 尚未受控：**没有终结算子**。
pub struct Unchecked;
/// 租户 + 软删 + 归属条件已叠加。
pub struct Scoped;
/// 只叠加租户 + 软删：归属过滤被显式豁免。
pub struct TenantBound;
/// 不叠加任何条件：仅限跨租户管理。
pub struct Global;

impl state::Sealed for Unchecked {}
impl QueryState for Unchecked {}
impl state::Sealed for Scoped {}
impl QueryState for Scoped {}
impl state::Sealed for TenantBound {}
impl QueryState for TenantBound {}
impl state::Sealed for Global {}
impl QueryState for Global {}

/// 受控查询。字段私有：只有 `kernel` 内部能构造。
pub struct Query<E: EntityTrait, S: QueryState = Unchecked> {
    select: Select<E>,
    _state: PhantomData<S>,
}

impl<E: EntityTrait> Query<E, Unchecked> {
    /// 查询的**唯一**起点（业务代码不用 `E::find()`）。
    pub fn open() -> Self {
        Self {
            select: E::find(),
            _state: PhantomData,
        }
    }
}

impl<E: EntityTrait, S: QueryState> Query<E, S> {
    /// 非终结算子：保留状态，链式写法不受影响。
    pub fn filter<F: IntoCondition>(mut self, f: F) -> Self {
        self.select = self.select.filter(f);
        self
    }

    /// 条件性地叠加过滤，避免为 `Option` 分支写样板。
    pub fn filter_if(self, on: bool, f: impl FnOnce(Self) -> Self) -> Self {
        if on { f(self) } else { self }
    }

    pub fn order_by<C: IntoSimpleExpr>(mut self, c: C) -> Self {
        self.select = self.select.order_by_asc(c);
        self
    }

    pub fn order_by_desc<C: IntoSimpleExpr>(mut self, c: C) -> Self {
        self.select = self.select.order_by_desc(c);
        self
    }

    pub fn limit<T: Into<Option<u64>>>(mut self, n: T) -> Self {
        self.select = self.select.limit(n);
        self
    }

    pub fn offset<T: Into<Option<u64>>>(mut self, n: T) -> Self {
        self.select = self.select.offset(n);
        self
    }
}

impl<E: DataScoped> Query<E, Unchecked> {
    /// 唯一把 `Unchecked` 升格成 `Scoped` 的通道。**要求 `E: DataScoped`**。
    pub fn scoped(mut self, scope: &Scope) -> Query<E, Scoped> {
        self.select = scope.apply::<E>(self.select);
        Query {
            select: self.select,
            _state: PhantomData,
        }
    }
}

/// 「确实不加归属过滤」的合法场景。**必须显式表态**，理由的类型决定升格后的状态。
pub trait UnscopedReason: Send + Sync + 'static {
    type State: QueryState;
    const LABEL: &'static str;
}

/// 按主键取单条（调用方已持有 id，通常来自一次受控列表）。
pub struct ByPrimaryKey;
impl UnscopedReason for ByPrimaryKey {
    type State = TenantBound;
    const LABEL: &'static str = "by_primary_key";
}

/// 定时任务 / 运维脚本：单租户内运行。
pub struct InternalTask;
impl UnscopedReason for InternalTask {
    type State = TenantBound;
    const LABEL: &'static str = "internal_task";
}

/// 跨租户管理：**连租户条件都不要**。最危险的一格，因此是独立类型、可 grep、可计数。
pub struct CrossTenant;
impl UnscopedReason for CrossTenant {
    type State = Global;
    const LABEL: &'static str = "cross_tenant";
}

impl<E: EntityTrait> Query<E, Unchecked> {
    /// 显式豁免。返回类型 `Query<E, R::State>` 由理由类型决定。
    pub fn unscoped<R: UnscopedReason>(self, _reason: R) -> Query<E, R::State> {
        tracing::warn!(
            reason = R::LABEL,
            entity = E::default().table_name(),
            "unscoped query"
        );
        Query {
            select: self.select,
            _state: PhantomData,
        }
    }
}

/// 终结算子**只**在这三个状态上存在。`Unchecked` 没有 `all` / `one` / `count` / `fetch_page`。
macro_rules! executable {
    ($($S:ty),* $(,)?) => { $(
        impl<E: EntityTrait> Query<E, $S>
        where
            E::Model: Send + Sync + 'static,
        {
            pub async fn all<C: ConnectionTrait>(self, db: &C) -> Result<Vec<E::Model>, DbErr> {
                self.select.all(db).await
            }

            pub async fn one<C: ConnectionTrait>(self, db: &C) -> Result<Option<E::Model>, DbErr> {
                self.select.one(db).await
            }

            pub async fn count<C: ConnectionTrait>(self, db: &C) -> Result<u64, DbErr> {
                self.select.count(db).await
            }

            /// 两段查询：先 `num_items()` 再 `fetch_page`。`page` 从 1 开始，
            /// 对 SeaORM 的 0 基 `fetch_page` 做 `saturating_sub(1)`。
            pub async fn fetch_page<C: ConnectionTrait>(
                self,
                db: &C,
                page: u64,
                page_size: u64,
            ) -> Result<Page<E::Model>, DbErr> {
                let paginator = self.select.paginate(db, page_size);
                let total = paginator.num_items().await?;
                let items = paginator.fetch_page(page.saturating_sub(1)).await?;
                Ok(Page {
                    items,
                    total,
                    page,
                    page_size,
                })
            }
        }
    )* };
}

executable!(Scoped, TenantBound, Global);

/// 仅供测试：把封装好的查询取出来做 SQL 断言（不触碰数据库）。
#[cfg(test)]
impl<E: EntityTrait, S: QueryState> Query<E, S> {
    fn into_select(self) -> Select<E> {
        self.select
    }
}

impl Scope {
    /// 叠加「租户 + 软删 + 归属」三类条件。只被 `Query::scoped` 调用。
    fn apply<E: DataScoped>(&self, select: Select<E>) -> Select<E> {
        let mut cond = Condition::all().add(E::tenant_column().eq(self.tenant_id.get()));

        if let Some(deleted) = E::soft_delete_column() {
            cond = cond.add(deleted.is_null());
        }

        match E::Shape::judge::<E>(&self.data_scope, E::dept_column(), E::owner_column()) {
            ShapeOutcome::Cond(ownership) => cond = cond.add(ownership),
            ShapeOutcome::TenantWide => {}
            ShapeOutcome::Denied => {
                tracing::warn!(
                    entity = E::default().table_name(),
                    scope = self.data_scope.kind.as_str(),
                    "data scope denied; filtering to empty set"
                );
                cond = cond.add(Expr::val(false));
            }
        }

        select.filter(cond)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::id::{DeptId, TenantId, UserId};
    use crate::scope::value::{DataScope, DataScopeKind};
    use crate::scope::{DeptAndOwner, DeptOnly, Exempt, NoOwnership, OwnerOnly, Shape};
    use sea_orm::entity::prelude::*;
    use sea_orm::{DeriveActiveModelBehavior, DeriveEntityModel, QueryTrait};

    #[derive(Clone, Debug, PartialEq, DeriveEntityModel, DeriveActiveModelBehavior)]
    #[sea_orm(table_name = "test_doc")]
    pub struct Model {
        #[sea_orm(primary_key)]
        pub id: i64,
        pub tenant_id: i64,
        pub dept_id: i64,
        pub owner_id: i64,
        pub deleted_at: Option<i64>,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl DataScoped for Entity {
        type Shape = DeptAndOwner;

        fn tenant_column() -> Self::Column {
            Column::TenantId
        }

        fn soft_delete_column() -> Option<Self::Column> {
            Some(Column::DeletedAt)
        }

        fn dept_column() -> Option<Self::Column> {
            Some(Column::DeptId)
        }

        fn owner_column() -> Option<Self::Column> {
            Some(Column::OwnerId)
        }
    }

    fn scope(kind: DataScopeKind, depts: &[i64], user: i64) -> Scope {
        Scope {
            tenant_id: TenantId::new(1),
            user_id: Some(UserId::new(user)),
            data_scope: DataScope {
                kind,
                dept_ids: depts.iter().copied().map(DeptId::new).collect(),
                subtree_paths: Vec::new(),
                user_id: UserId::new(user),
            },
        }
    }

    fn sql(scope: &Scope) -> String {
        Query::<Entity>::open()
            .scoped(scope)
            .into_select()
            .build(sea_orm::DatabaseBackend::Postgres)
            .to_string()
    }

    #[test]
    fn scoped_always_applies_tenant_and_soft_delete() {
        let s = sql(&scope(DataScopeKind::All, &[], 7));
        assert!(
            s.contains(r#""test_doc"."tenant_id" = 1"#),
            "缺少租户条件：{s}"
        );
        assert!(
            s.contains(r#""test_doc"."deleted_at" IS NULL"#),
            "缺少软删条件：{s}"
        );
        // All：不叠加归属条件。SELECT 投影里本就有 dept_id/owner_id 列，因此只看 WHERE 子句。
        let where_clause = s.split_once(" WHERE ").map(|(_, w)| w).unwrap_or("");
        assert!(
            !where_clause.contains("dept_id"),
            "All 不应叠加部门条件：{s}"
        );
        assert!(
            !where_clause.contains("owner_id"),
            "All 不应叠加归属人条件：{s}"
        );
    }

    #[test]
    fn custom_scope_filters_by_dept() {
        let s = sql(&scope(DataScopeKind::Custom, &[3, 4], 7));
        assert!(
            s.contains(r#""test_doc"."dept_id" IN (3, 4)"#),
            "缺少部门 IN 条件：{s}"
        );
    }

    #[test]
    fn self_only_scope_filters_by_owner() {
        let s = sql(&scope(DataScopeKind::SelfOnly, &[], 7));
        assert!(
            s.contains(r#""test_doc"."owner_id" = 7"#),
            "缺少归属人条件：{s}"
        );
    }

    #[test]
    fn none_scope_denies_everything() {
        let s = sql(&scope(DataScopeKind::None, &[], 7));
        assert!(s.contains("WHERE"), "None 应拒绝一切：{s}");
        assert!(
            s.to_ascii_uppercase().contains("FALSE"),
            "None 应包含 FALSE：{s}"
        );
    }

    #[test]
    fn filter_can_be_added_before_or_after_scoping() {
        let scope = scope(DataScopeKind::All, &[], 7);
        let before = Query::<Entity>::open()
            .filter(Column::Id.eq(5))
            .scoped(&scope)
            .into_select()
            .build(sea_orm::DatabaseBackend::Postgres)
            .to_string();
        let after = Query::<Entity>::open()
            .scoped(&scope)
            .filter(Column::Id.eq(5))
            .into_select()
            .build(sea_orm::DatabaseBackend::Postgres)
            .to_string();
        // 两种顺序都成立：业务条件与范围条件只是排列先后不同，语义等价。
        for sql in [&before, &after] {
            assert!(
                sql.contains(r#""test_doc"."id" = 5"#),
                "缺少业务条件：{sql}"
            );
            assert!(
                sql.contains(r#""test_doc"."tenant_id" = 1"#),
                "缺少租户条件：{sql}"
            );
            assert!(
                sql.contains(r#""test_doc"."deleted_at" IS NULL"#),
                "缺少软删条件：{sql}"
            );
        }
    }

    fn outcome_tag(outcome: ShapeOutcome<Condition>) -> &'static str {
        match outcome {
            ShapeOutcome::Cond(_) => "cond",
            ShapeOutcome::TenantWide => "wide",
            ShapeOutcome::Denied => "denied",
        }
    }

    fn judge(shape: &'static str, kind: DataScopeKind) -> &'static str {
        let data_scope = DataScope {
            kind,
            dept_ids: vec![DeptId::new(3)],
            subtree_paths: vec![],
            user_id: UserId::new(7),
        };
        let (dept, owner) = match shape {
            "dept_and_owner" => (Some(Column::DeptId), Some(Column::OwnerId)),
            "dept_only" => (Some(Column::DeptId), None),
            "owner_only" => (None, Some(Column::OwnerId)),
            "no_ownership" | "exempt" => (None, None),
            _ => unreachable!(),
        };
        let outcome = match shape {
            "dept_and_owner" => DeptAndOwner::judge::<Entity>(&data_scope, dept, owner),
            "dept_only" => DeptOnly::judge::<Entity>(&data_scope, dept, owner),
            "owner_only" => OwnerOnly::judge::<Entity>(&data_scope, dept, owner),
            "no_ownership" => NoOwnership::judge::<Entity>(&data_scope, dept, owner),
            "exempt" => Exempt::judge::<Entity>(&data_scope, dept, owner),
            _ => unreachable!(),
        };
        outcome_tag(outcome)
    }

    #[test]
    fn judge_matrix_matches_final_spec() {
        use DataScopeKind::*;

        // (shape, kind) -> 预期：cond=叠加条件，wide=整租户，denied=失败关闭。
        let cases: &[(&str, DataScopeKind, &str)] = &[
            ("dept_and_owner", All, "wide"),
            ("dept_and_owner", Custom, "cond"),
            ("dept_and_owner", Dept, "cond"),
            ("dept_and_owner", DeptAndChild, "cond"),
            ("dept_and_owner", SelfOnly, "cond"),
            ("dept_and_owner", None, "denied"),
            ("dept_only", All, "wide"),
            ("dept_only", Custom, "cond"),
            ("dept_only", Dept, "cond"),
            ("dept_only", DeptAndChild, "cond"),
            ("dept_only", SelfOnly, "denied"),
            ("dept_only", None, "denied"),
            ("owner_only", All, "wide"),
            ("owner_only", Custom, "denied"),
            ("owner_only", Dept, "denied"),
            ("owner_only", DeptAndChild, "denied"),
            ("owner_only", SelfOnly, "cond"),
            ("owner_only", None, "denied"),
            ("no_ownership", All, "wide"),
            ("no_ownership", Custom, "denied"),
            ("no_ownership", Dept, "denied"),
            ("no_ownership", DeptAndChild, "denied"),
            ("no_ownership", SelfOnly, "denied"),
            ("no_ownership", None, "denied"),
            ("exempt", All, "wide"),
            ("exempt", Custom, "wide"),
            ("exempt", Dept, "wide"),
            ("exempt", DeptAndChild, "wide"),
            ("exempt", SelfOnly, "wide"),
            ("exempt", None, "denied"),
        ];

        for (shape, kind, expected) in cases {
            assert_eq!(
                judge(shape, *kind),
                *expected,
                "shape={shape} kind={kind:?}"
            );
        }
    }
}
