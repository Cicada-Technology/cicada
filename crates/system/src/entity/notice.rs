//! `sys_notice`：通知公告（全局表，无 `tenant_id`、无软删）。

use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, DeriveActiveModelBehavior)]
#[sea_orm(table_name = "sys_notice")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub notice_id: i64,
    pub notice_title: String,
    /// 1 通知 / 2 公告（`sys_notice_type`）。
    pub notice_type: i16,
    pub notice_content: Option<String>,
    pub status: i16,
    pub create_by: Option<String>,
    pub create_time: DateTimeUtc,
    pub update_by: Option<String>,
    pub update_time: Option<DateTimeUtc>,
    pub remark: Option<String>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}
