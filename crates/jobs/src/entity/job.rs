//! `sys_job`：定时任务定义。

use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, DeriveActiveModelBehavior)]
#[sea_orm(table_name = "sys_job")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub job_id: i64,
    pub tenant_id: i64,
    pub job_name: String,
    pub job_group: String,
    /// 注册表 key（不是类名 / 表达式）。
    pub invoke_target: String,
    pub cron_expression: String,
    /// 并发策略：0 允许 / 1 禁止 / 2 队列（`cicada_kernel::ConcurrencyPolicy`）。
    pub concurrency: i16,
    /// 单次执行超时（秒），0 表示不限。
    pub timeout_secs: i64,
    /// 失败重试次数（不含首次执行）。
    pub max_retry: i32,
    /// 0 正常 / 1 暂停。
    pub status: i16,
    pub remark: Option<String>,
    pub create_by: Option<String>,
    pub create_time: DateTimeUtc,
    pub update_by: Option<String>,
    pub update_time: Option<DateTimeUtc>,
    pub deleted_at: Option<DateTimeUtc>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}
