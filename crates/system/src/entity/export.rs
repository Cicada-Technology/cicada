//! `sys_export_task`：异步导出任务。
//!
//! `query` 是条件快照（含数据范围）；完成后 `file_id` 指向 `sys_file`。
//! 任务归属发起人（`user_id`）：tenant + owner 数据范围，列表只见自己的任务。

use sea_orm::entity::prelude::*;

use cicada_macros::DataScoped;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, DeriveActiveModelBehavior, DataScoped)]
#[sea_orm(table_name = "sys_export_task")]
#[data_scope(tenant = Column::TenantId, owner = Column::UserId, deleted = Column::DeletedAt)]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i64,
    pub tenant_id: i64,
    /// 发起人（非 API Key 的 0 账号：异步导出必须是自然人请求派生）。
    pub user_id: i64,
    /// 导出模块标识（注册表 key，如 `system:user`）。
    pub module: String,
    /// 条件快照：`{"scope": {...}, "filters": {...}}`，保证与请求一致（审计日志表）。
    pub query: Json,
    /// 0 Pending / 1 Running / 2 Done / 3 Failed（审计日志表枚举）。
    pub status: i16,
    /// 完成后指向 `sys_file`。
    pub file_id: Option<i64>,
    pub total: i64,
    pub processed: i64,
    /// 失败原因（可空）。
    pub error: Option<String>,
    pub finish_time: Option<DateTimeUtc>,
    pub create_time: DateTimeUtc,
    pub update_time: Option<DateTimeUtc>,
    pub deleted_at: Option<DateTimeUtc>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}
