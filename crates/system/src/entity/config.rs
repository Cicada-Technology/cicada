//! `sys_config`：参数配置（全局表，无 `tenant_id`、无软删）。

use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, DeriveActiveModelBehavior)]
#[sea_orm(table_name = "sys_config")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub config_id: i64,
    pub config_name: String,
    pub config_key: String,
    pub config_value: String,
    /// `Y` 系统内置 / `N` 用户自定义。
    pub config_type: String,
    pub create_by: Option<String>,
    pub create_time: DateTimeUtc,
    pub update_by: Option<String>,
    pub update_time: Option<DateTimeUtc>,
    pub remark: Option<String>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}
