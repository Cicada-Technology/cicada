//! `sys_job_log`：定时任务执行日志。

use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, DeriveActiveModelBehavior)]
#[sea_orm(table_name = "sys_job_log")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub job_log_id: i64,
    pub tenant_id: i64,
    pub job_id: i64,
    pub job_name: String,
    pub job_group: String,
    pub invoke_target: String,
    /// 触发实例标识（分布式下用于排查重复触发）。
    pub instance_id: String,
    /// 0 调度触发 / 1 手动触发。
    pub trigger_kind: i16,
    pub start_time: DateTimeUtc,
    pub end_time: Option<DateTimeUtc>,
    pub duration_ms: Option<i64>,
    /// 0 成功 / 1 失败。
    pub status: i16,
    /// 成功摘要或异常摘要（截断）。
    pub result: Option<String>,
    pub trace_id: Option<String>,
    pub create_time: DateTimeUtc,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}
