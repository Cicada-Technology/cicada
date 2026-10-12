//! `sys_notify_template`：通知模板（`title` / `body` 用 minijinja 渲染）。

use sea_orm::entity::prelude::*;

use cicada_macros::DataScoped;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, DeriveActiveModelBehavior, DataScoped)]
#[sea_orm(table_name = "sys_notify_template")]
#[data_scope(tenant = Column::TenantId, no_ownership)]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i64,
    pub tenant_id: i64,
    pub code: String,
    pub name: String,
    /// 渠道类型：模板与渠道 kind 对齐（sms / email / webhook / im）。
    pub channel_kind: String,
    pub locale: String,
    pub title: String,
    pub body: String,
    pub enabled: bool,
    pub remark: Option<String>,
    pub create_by: Option<String>,
    pub create_time: DateTimeUtc,
    pub update_by: Option<String>,
    pub update_time: Option<DateTimeUtc>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}
