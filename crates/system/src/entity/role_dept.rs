//! `sys_role_dept`：角色-部门关联（`Custom` 数据范围的自定部门，复合主键）。

use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, DeriveActiveModelBehavior)]
#[sea_orm(table_name = "sys_role_dept")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub role_id: i64,
    #[sea_orm(primary_key)]
    pub dept_id: i64,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}
