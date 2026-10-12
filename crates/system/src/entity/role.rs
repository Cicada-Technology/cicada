//! `sys_role`：角色。无部门/归属列，租户隔离 + 软删（`NoOwnership`）。

use sea_orm::entity::prelude::*;

use cicada_macros::DataScoped;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, DeriveActiveModelBehavior, DataScoped)]
#[sea_orm(table_name = "sys_role")]
#[data_scope(tenant = Column::TenantId, deleted = Column::DeletedAt, no_ownership)]
pub struct Model {
    #[sea_orm(primary_key)]
    pub role_id: i64,
    pub tenant_id: i64,
    pub role_name: String,
    pub role_key: String,
    pub role_sort: i32,
    pub data_scope: i16,
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
