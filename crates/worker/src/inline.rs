//! 进程内执行侧（默认）：触发后直接交给 [`JobExecutor`] 执行。
//!
//! 与队列执行侧（[`crate::apalis`]）互斥，二者都实现 [`cicada_ports::RunSink`]。

use std::sync::Arc;

use async_trait::async_trait;

use cicada_kernel::{JobId, TenantId, TriggerKind};
use cicada_ports::{JobError, RunSink, TriggerOutcome};

use crate::executor::JobExecutor;

/// 不经过队列：`submit` 就地执行一次，并返回 [`TriggerOutcome::Executed`]。
///
/// 并发策略（禁止并发 / 队列 / 允许并发）仍由 [`JobExecutor`] 统一把关。
pub struct InlineRunSink {
    executor: Arc<JobExecutor>,
}

impl InlineRunSink {
    pub fn new(executor: Arc<JobExecutor>) -> Self {
        Self { executor }
    }
}

#[async_trait]
impl RunSink for InlineRunSink {
    async fn submit(
        &self,
        tenant: TenantId,
        job_id: JobId,
        kind: TriggerKind,
    ) -> Result<TriggerOutcome, JobError> {
        let def = self
            .executor
            .store()
            .get(tenant, job_id)
            .await
            .map_err(|e| JobError::Infra(e.to_string()))?
            .ok_or_else(|| JobError::Invalid(format!("任务不存在：{job_id}")))?;
        Ok(self.executor.execute(&def, kind).await)
    }
}
