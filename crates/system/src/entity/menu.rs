//! `sys_menu`：菜单/权限点（全局，无 tenant_id、无软删）。
//!
//! 因此**不**实现 `DataScoped`；查询走 `unscoped(CrossTenant)` 这类显式豁免，
//! 或直接按 `menu_id` / `perms` 过滤（菜单是全局字典，不参与租户/数据范围）。

use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, DeriveActiveModelBehavior)]
#[sea_orm(table_name = "sys_menu")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub menu_id: i64,
    pub menu_name: String,
    pub parent_id: i64,
    pub order_num: i32,
    pub path: Option<String>,
    pub component: Option<String>,
    pub query: Option<String>,
    pub is_frame: i16,
    pub is_cache: i16,
    pub menu_type: String,
    pub visible: i16,
    pub status: i16,
    pub perms: Option<String>,
    pub icon: Option<String>,
    pub remark: Option<String>,
    pub create_by: Option<String>,
    pub create_time: DateTimeUtc,
    pub update_by: Option<String>,
    pub update_time: Option<DateTimeUtc>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}
