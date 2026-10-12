//! `JobStore`：定时任务定义 / 执行日志的读写门面（深模块）。
//!
//! 调度运行时（`cicada-worker`）只依赖这里，不直接碰 sea-orm 查询构造；
//! HTTP 面的 CRUD 仍可直接用 [`crate::entity`] 的实体（与 system 域其它模块一致）。

use chrono::Utc;
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, QueryOrder, Set};

use cicada_infra::Db;
use cicada_kernel::{ConcurrencyPolicy, JobId, TenantId};

use crate::entity::{
    JobColumn, JobEntity, JobLogActiveModel, JobLogColumn, JobLogEntity, JobModel,
};

#[derive(Debug, thiserror::Error)]
pub enum JobStoreError {
    #[error("定时任务数据库操作失败：{0}")]
    Db(#[from] sea_orm::DbErr),
}

/// 运行状态：`0` 正常（参与调度）/ `1` 暂停。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JobStatus {
    Normal,
    Paused,
}

impl JobStatus {
    pub const fn code(self) -> i16 {
        match self {
            Self::Normal => 0,
            Self::Paused => 1,
        }
    }

    pub const fn from_code(code: i16) -> Self {
        if code == 0 {
            Self::Normal
        } else {
            Self::Paused
        }
    }

    pub const fn is_normal(self) -> bool {
        matches!(self, Self::Normal)
    }
}

/// 调度运行时的任务定义（从 `sys_job` 行投影出的纯数据）。
#[derive(Clone, Debug)]
pub struct JobDefinition {
    pub job_id: JobId,
    pub tenant_id: TenantId,
    pub job_name: String,
    pub job_group: String,
    pub invoke_target: String,
    pub cron_expression: String,
    pub concurrency: ConcurrencyPolicy,
    pub timeout_secs: u64,
    pub max_retry: u32,
}

impl From<JobModel> for JobDefinition {
    fn from(m: JobModel) -> Self {
        Self {
            job_id: JobId::new(m.job_id),
            tenant_id: TenantId::new(m.tenant_id),
            job_name: m.job_name,
            job_group: m.job_group,
            invoke_target: m.invoke_target,
            cron_expression: m.cron_expression,
            concurrency: ConcurrencyPolicy::from_code(m.concurrency),
            timeout_secs: m.timeout_secs.max(0) as u64,
            max_retry: m.max_retry.max(0) as u32,
        }
    }
}

/// 一次执行的待写日志（`sys_job_log` 一行）。
#[derive(Clone, Debug)]
pub struct NewJobLog {
    pub tenant_id: TenantId,
    pub job_id: JobId,
    pub job_name: String,
    pub job_group: String,
    pub invoke_target: String,
    pub instance_id: String,
    pub trigger_kind: i16,
    pub start_time: chrono::DateTime<Utc>,
    pub end_time: chrono::DateTime<Utc>,
    pub duration_ms: i64,
    /// 0 成功 / 1 失败。
    pub status: i16,
    pub result: Option<String>,
    pub trace_id: Option<String>,
}

/// 定义与日志的只读 + 追加写门面。
#[derive(Clone)]
pub struct JobStore {
    db: Db,
}

impl JobStore {
    pub fn new(db: Db) -> Self {
        Self { db }
    }

    pub fn db(&self) -> &Db {
        &self.db
    }

    /// 按主键取一个未删除且属于该租户的任务定义。
    pub async fn get(
        &self,
        tenant: TenantId,
        job_id: JobId,
    ) -> Result<Option<JobDefinition>, JobStoreError> {
        let row = JobEntity::find_by_id(job_id.get())
            .filter(JobColumn::TenantId.eq(tenant.get()))
            .filter(JobColumn::DeletedAt.is_null())
            .one(self.db.conn())
            .await?;
        Ok(row.map(JobDefinition::from))
    }

    /// 全部租户里处于「正常」状态的任务（调度器启动 / 重载用）。
    pub async fn list_schedulable(&self) -> Result<Vec<JobDefinition>, JobStoreError> {
        let rows = JobEntity::find()
            .filter(JobColumn::Status.eq(JobStatus::Normal.code()))
            .filter(JobColumn::DeletedAt.is_null())
            .order_by_asc(JobColumn::JobId)
            .all(self.db.conn())
            .await?;
        Ok(rows.into_iter().map(JobDefinition::from).collect())
    }

    /// 某租户全部未删除的任务（测试与人工触发用）。
    pub async fn list_tenant(&self, tenant: TenantId) -> Result<Vec<JobDefinition>, JobStoreError> {
        let rows = JobEntity::find()
            .filter(JobColumn::TenantId.eq(tenant.get()))
            .filter(JobColumn::DeletedAt.is_null())
            .order_by_asc(JobColumn::JobId)
            .all(self.db.conn())
            .await?;
        Ok(rows.into_iter().map(JobDefinition::from).collect())
    }

    /// 追加一条执行日志。
    pub async fn insert_log(&self, log: NewJobLog) -> Result<(), JobStoreError> {
        JobLogActiveModel {
            tenant_id: Set(log.tenant_id.get()),
            job_id: Set(log.job_id.get()),
            job_name: Set(log.job_name),
            job_group: Set(log.job_group),
            invoke_target: Set(log.invoke_target),
            instance_id: Set(log.instance_id),
            trigger_kind: Set(log.trigger_kind),
            start_time: Set(log.start_time),
            end_time: Set(Some(log.end_time)),
            duration_ms: Set(Some(log.duration_ms)),
            status: Set(log.status),
            result: Set(log.result),
            trace_id: Set(log.trace_id),
            create_time: Set(Utc::now()),
            ..Default::default()
        }
        .insert(self.db.conn())
        .await?;
        Ok(())
    }

    /// 清空某任务的全部执行日志，返回删除行数（任务删除时级联清理）。
    pub async fn purge_logs(&self, tenant: TenantId, job_id: JobId) -> Result<u64, JobStoreError> {
        let res = JobLogEntity::delete_many()
            .filter(JobLogColumn::TenantId.eq(tenant.get()))
            .filter(JobLogColumn::JobId.eq(job_id.get()))
            .exec(self.db.conn())
            .await?;
        Ok(res.rows_affected)
    }
}
