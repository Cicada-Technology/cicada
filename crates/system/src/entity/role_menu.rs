//! `sys_role_menu`：角色-菜单关联（复合主键）。

use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, DeriveActiveModelBehavior)]
#[sea_orm(table_name = "sys_role_menu")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub role_id: i64,
    #[sea_orm(primary_key)]
    pub menu_id: i64,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}
