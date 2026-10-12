//! `sys_tenant`：租户注册表。
//!
//! 这是**租户自身的登记表**，不带 `tenant_id` 列，也不参与 RLS 行过滤；
//! 管理接口仅超级管理员可见（跨租户的平台级操作）。

use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, DeriveActiveModelBehavior)]
#[sea_orm(table_name = "sys_tenant")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub tenant_id: i64,
    pub tenant_name: String,
    pub status: i16,
    /// 套餐 id（可空 = 无套餐 = 不限额）。
    pub package_id: Option<i64>,
    pub remark: Option<String>,
    pub create_by: Option<String>,
    pub create_time: DateTimeUtc,
    pub update_by: Option<String>,
    pub update_time: Option<DateTimeUtc>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}
