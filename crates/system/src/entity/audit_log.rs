//! `sys_audit_log`：字段级变更留痕。
//!
//! 由 `cicada-audit` 产生 `audit.change_log` 事件、relay 投递后由 `system` 落地；
//! 本实体只服务 `monitor:auditlog:*` 的只读接口。
//!
//! 表按月分区，主键是 `(audit_id, oper_time)`——分区键必须进唯一约束，
//! 因此 `audit_id` 关掉自增（默认值来自序列，见迁移）。

use sea_orm::entity::prelude::*;

use cicada_macros::DataScoped;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, DeriveActiveModelBehavior, DataScoped)]
#[sea_orm(table_name = "sys_audit_log")]
#[data_scope(tenant = Column::TenantId, no_ownership)]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub audit_id: i64,
    pub tenant_id: i64,
    /// 被改动的实体（表名）。
    pub table_name: String,
    /// 主键值，字符串化以兼容复合键。
    pub record_id: String,
    /// `insert` / `update` / `delete`。
    pub action: String,
    /// 只含变化字段：`{field: [old, new]}`；凭据类字段生产端已滤除。
    #[sea_orm(column_type = "JsonBinary")]
    pub changed: Json,
    /// 操作人。
    pub oper_id: Option<i64>,
    #[sea_orm(primary_key, auto_increment = false)]
    pub oper_time: DateTimeUtc,
    /// 关联链路。
    pub trace_id: Option<String>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}
