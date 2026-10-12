//! `sys_notice_read`：公告已读记录（全局表，复合主键 `(notice_id, user_id)`）。

use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, DeriveActiveModelBehavior)]
#[sea_orm(table_name = "sys_notice_read")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub notice_id: i64,
    #[sea_orm(primary_key, auto_increment = false)]
    pub user_id: i64,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}
