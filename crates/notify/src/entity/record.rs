//! `sys_notify_record`：一次投递的留痕（不可变，无软删）。

use sea_orm::entity::prelude::*;

use cicada_macros::DataScoped;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, DeriveActiveModelBehavior, DataScoped)]
#[sea_orm(table_name = "sys_notify_record")]
#[data_scope(tenant = Column::TenantId, no_ownership)]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i64,
    pub tenant_id: i64,
    pub channel_id: i64,
    pub template_code: String,
    /// 收件人：手机号 / 邮箱 / webhook 地址（审计日志表的 `to`，避开 SQL 关键字改名）。
    pub recipient: String,
    pub payload: Json,
    /// 0 待发 / 1 已发 / 2 失败。
    pub status: i16,
    pub error: Option<String>,
    pub create_by: Option<String>,
    pub create_time: DateTimeUtc,
    pub sent_at: Option<DateTimeUtc>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}
