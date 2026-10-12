//! `sys_tenant_package`：租户套餐与限额。
//!
//! `-1` 表示不限；`features` 是不强约束的开关集合，供业务侧自查。

use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, DeriveActiveModelBehavior)]
#[sea_orm(table_name = "sys_tenant_package")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i64,
    pub code: String,
    pub name: String,
    /// 用户数上限，`-1` 不限。
    pub user_limit: i32,
    /// 存储上限（MB），`-1` 不限。
    pub storage_limit_mb: i64,
    /// 接口 QPS 上限，`-1` 不限。
    pub api_qps_limit: i32,
    pub features: Json,
    pub status: i16,
    pub remark: Option<String>,
    pub create_by: Option<String>,
    pub create_time: DateTimeUtc,
    pub update_by: Option<String>,
    pub update_time: Option<DateTimeUtc>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}
