//! `sys_notify_channel`：通知渠道（kind = sms / email / webhook / im）。

use sea_orm::entity::prelude::*;

use cicada_macros::DataScoped;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, DeriveActiveModelBehavior, DataScoped)]
#[sea_orm(table_name = "sys_notify_channel")]
#[data_scope(tenant = Column::TenantId, no_ownership)]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i64,
    pub tenant_id: i64,
    pub code: String,
    pub name: String,
    pub kind: String,
    /// 驱动配置（webhook url / email 网关 / sms 网关参数），结构见 `crate::channel`。
    pub config: Json,
    pub enabled: bool,
    pub remark: Option<String>,
    pub create_by: Option<String>,
    pub create_time: DateTimeUtc,
    pub update_by: Option<String>,
    pub update_time: Option<DateTimeUtc>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}
