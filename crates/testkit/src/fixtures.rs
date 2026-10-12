//! 测试夹具：样例实体（`DeriveEntityModel` + `DataScoped`），供 typestate 的
//! 编译失败测试与后续组件测试复用。

use cicada_macros::DataScoped;
use sea_orm::entity::prelude::*;

/// 一个带完整归属维度的样例实体：`DeptAndOwner` 形态。
#[derive(Clone, Debug, PartialEq, DeriveEntityModel, DeriveActiveModelBehavior, DataScoped)]
#[sea_orm(table_name = "sample_doc")]
#[data_scope(
    tenant = Column::TenantId,
    dept = Column::DeptId,
    owner = Column::OwnerId,
    deleted = Column::DeletedAt
)]
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
