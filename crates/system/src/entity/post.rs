//! `sys_post`：岗位。有 `tenant_id` 与软删，但无部门/归属列（`NoOwnership`）。

use sea_orm::entity::prelude::*;

use cicada_macros::DataScoped;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, DeriveActiveModelBehavior, DataScoped)]
#[sea_orm(table_name = "sys_post")]
#[data_scope(tenant = Column::TenantId, deleted = Column::DeletedAt, no_ownership)]
pub struct Model {
    #[sea_orm(primary_key)]
    pub post_id: i64,
    pub tenant_id: i64,
    pub post_code: String,
    pub post_name: String,
    pub post_sort: i32,
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
