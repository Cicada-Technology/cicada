//! `sys_logininfor`：登录日志（租户隔离、无软删、无归属维度 → `NoOwnership`）。

use sea_orm::entity::prelude::*;

use cicada_macros::DataScoped;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, DeriveActiveModelBehavior, DataScoped)]
#[sea_orm(table_name = "sys_logininfor")]
#[data_scope(tenant = Column::TenantId, no_ownership)]
pub struct Model {
    #[sea_orm(primary_key)]
    pub info_id: i64,
    pub tenant_id: i64,
    pub user_name: String,
    pub ipaddr: Option<String>,
    pub login_location: Option<String>,
    pub browser: Option<String>,
    pub os: Option<String>,
    /// 0 成功 / 1 失败。
    pub status: i16,
    pub msg: Option<String>,
    pub login_time: DateTimeUtc,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}
