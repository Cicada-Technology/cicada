//! 实体声明：`DataScoped` + 类型级 `Shape`（datascope-typestate.md 项目目标）。
//!
//! `Shape` 是 sealed 的：业务侧只能四选一（`DeptAndOwner` / `DeptOnly` /
//! `OwnerOnly` / `NoOwnership`）加一个显式豁免 `Exempt`，不能自定义第六种形态。
//! 这换来了「判定逻辑只写一次」的局部性。

use sea_orm::{ColumnTrait, EntityTrait};
use sea_query::Condition;

use crate::scope::value::{DataScope, DataScopeKind};

mod sealed {
    pub trait Sealed {}
}

/// 一次范围判定的结果。把「拒答」做成一个**值**而不是 `None`，
/// 以区分「没有这一列」与「有意不过滤」。
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

/// 有部门列 + 有归属人列。
pub struct DeptAndOwner;
/// 只有部门列。
pub struct DeptOnly;
/// 只有归属人列。
pub struct OwnerOnly;
/// 两者都没有 —— 部门/本人范围一律 Denied。
pub struct NoOwnership;
/// **有意放行**归属过滤 —— 部门/本人范围一律 TenantWide（须写明理由）。
pub struct Exempt;

impl sealed::Sealed for DeptAndOwner {}
impl sealed::Sealed for DeptOnly {}
impl sealed::Sealed for OwnerOnly {}
impl sealed::Sealed for NoOwnership {}
impl sealed::Sealed for Exempt {}

impl Shape for DeptAndOwner {
    fn judge<E: DataScoped>(
        scope: &DataScope,
        dept: Option<E::Column>,
        owner: Option<E::Column>,
    ) -> ShapeOutcome<Condition> {
        let dept = dept.expect("DeptAndOwner 由宏保证提供 dept_column");
        let owner = owner.expect("DeptAndOwner 由宏保证提供 owner_column");
        match scope.kind {
            DataScopeKind::All => ShapeOutcome::TenantWide,
            DataScopeKind::None => ShapeOutcome::Denied,
            DataScopeKind::Custom | DataScopeKind::Dept | DataScopeKind::DeptAndChild => {
                ShapeOutcome::Cond(
                    Condition::all().add(dept.is_in(scope.dept_ids.iter().map(|d| d.get()))),
                )
            }
            DataScopeKind::SelfOnly => {
                ShapeOutcome::Cond(Condition::all().add(owner.eq(scope.user_id.get())))
            }
        }
    }
}

impl Shape for DeptOnly {
    fn judge<E: DataScoped>(
        scope: &DataScope,
        dept: Option<E::Column>,
        _owner: Option<E::Column>,
    ) -> ShapeOutcome<Condition> {
        let dept = dept.expect("DeptOnly 由宏保证提供 dept_column");
        match scope.kind {
            DataScopeKind::All => ShapeOutcome::TenantWide,
            DataScopeKind::Custom | DataScopeKind::Dept | DataScopeKind::DeptAndChild => {
                ShapeOutcome::Cond(
                    Condition::all().add(dept.is_in(scope.dept_ids.iter().map(|d| d.get()))),
                )
            }
            // SelfOnly 没有归属人列可依 => 拒答；None => 拒答。
            DataScopeKind::SelfOnly | DataScopeKind::None => ShapeOutcome::Denied,
        }
    }
}

impl Shape for OwnerOnly {
    fn judge<E: DataScoped>(
        scope: &DataScope,
        _dept: Option<E::Column>,
        owner: Option<E::Column>,
    ) -> ShapeOutcome<Condition> {
        let owner = owner.expect("OwnerOnly 由宏保证提供 owner_column");
        match scope.kind {
            DataScopeKind::All => ShapeOutcome::TenantWide,
            DataScopeKind::SelfOnly => {
                ShapeOutcome::Cond(Condition::all().add(owner.eq(scope.user_id.get())))
            }
            // Custom/Dept/DeptAndChild 没有部门列可依 => 拒答；None => 拒答。
            DataScopeKind::Custom
            | DataScopeKind::Dept
            | DataScopeKind::DeptAndChild
            | DataScopeKind::None => ShapeOutcome::Denied,
        }
    }
}

impl Shape for NoOwnership {
    fn judge<E: DataScoped>(
        scope: &DataScope,
        _dept: Option<E::Column>,
        _owner: Option<E::Column>,
    ) -> ShapeOutcome<Condition> {
        match scope.kind {
            DataScopeKind::All => ShapeOutcome::TenantWide,
            _ => ShapeOutcome::Denied,
        }
    }
}

impl Shape for Exempt {
    fn judge<E: DataScoped>(
        scope: &DataScope,
        _dept: Option<E::Column>,
        _owner: Option<E::Column>,
    ) -> ShapeOutcome<Condition> {
        // 有意放行归属过滤：与 NoOwnership 是**两个不同的类型**。
        // 但 `None`（无数据权限）始终失败关闭——即使 exempt，也不能放行（FINAL.md  判定矩阵）。
        match scope.kind {
            DataScopeKind::None => ShapeOutcome::Denied,
            _ => ShapeOutcome::TenantWide,
        }
    }
}

/// 由 `#[derive(DataScoped)]` 唯一实现（见 cicada-macros）。
///
/// **没有本 impl 的实体，无法进入任何受控查询路径** —— `Query::scoped` 的
/// `E: DataScoped` 约束就是这道门。
pub trait DataScoped: EntityTrait {
    type Shape: Shape;

    fn tenant_column() -> Self::Column;
    fn soft_delete_column() -> Option<Self::Column>;
    /// 仅当 `Shape ∈ {DeptAndOwner, DeptOnly}` 时返回 `Some`。
    fn dept_column() -> Option<Self::Column>;
    /// 仅当 `Shape ∈ {DeptAndOwner, OwnerOnly}` 时返回 `Some`。
    fn owner_column() -> Option<Self::Column>;
}
