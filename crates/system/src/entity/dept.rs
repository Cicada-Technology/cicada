//! `sys_dept`：部门（树）。自身无「归属部门/归属人」语义，租户隔离 + 软删
//! （`NoOwnership`）；部门树展开经 `LocalDeptDirectory`。

use sea_orm::entity::prelude::*;

use cicada_macros::DataScoped;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, DeriveActiveModelBehavior, DataScoped)]
#[sea_orm(table_name = "sys_dept")]
#[data_scope(tenant = Column::TenantId, deleted = Column::DeletedAt, no_ownership)]
pub struct Model {
    #[sea_orm(primary_key)]
    pub dept_id: i64,
    pub tenant_id: i64,
    pub parent_id: i64,
    pub ancestors: String,
    pub dept_name: String,
    pub order_num: i32,
    pub leader: Option<String>,
    pub phone: Option<String>,
    pub email: Option<String>,
    pub status: i16,
    pub version: i32,
    pub deleted_at: Option<DateTimeUtc>,
    pub remark: Option<String>,
    pub create_by: Option<String>,
    pub create_time: DateTimeUtc,
    pub update_by: Option<String>,
    pub update_time: Option<DateTimeUtc>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}
