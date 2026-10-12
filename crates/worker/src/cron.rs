//! 触发侧：`tokio-cron-scheduler` 进程内 cron（默认）。
//!
//! 只负责「到点产出一系列该跑的任务」——产出后交给注入的 [`RunSink`]，
//! 由执行侧决定是就地执行（[`crate::inline::InlineRunSink`]）还是投递进队列
//! （[`crate::apalis::ApalisRunSink`]）。
//!
//! 集群下由 [`crate::runtime::Worker`] 选主，**只有 leader** 启动它；
//! 非 leader 上 [`TriggerSource::sync`] 是空操作，不会误起本地调度。

use std::sync::Arc;

use async_trait::async_trait;

use cicada_jobs::JobStore;
use cicada_kernel::TriggerKind;
use cicada_ports::{JobError, RunSink, TriggerSource};

use crate::schedule::{CronSchedule, FireCallback};

/// cron 生产者：到点把任务交给 [`RunSink`]。
pub struct CronTriggerSource {
    schedule: CronSchedule,
}

impl CronTriggerSource {
    pub fn new(store: JobStore, sink: Arc<dyn RunSink>) -> Self {
        let on_fire: FireCallback = Arc::new(move |def| {
            let sink = sink.clone();
            Box::pin(async move {
                if let Err(e) = sink
                    .submit(def.tenant_id, def.job_id, TriggerKind::Scheduled)
                    .await
                {
                    // 入队/执行失败只记日志：定义与日志都在库里，下一轮仍会触发。
                    tracing::warn!(job = %def.job_name, error = %e, "cron 触发提交失败");
                }
            })
        });
        Self {
            schedule: CronSchedule::new(store, on_fire),
        }
    }
}

#[async_trait]
impl TriggerSource for CronTriggerSource {
    async fn start(&self) -> Result<(), JobError> {
        self.schedule.start().await
    }

    async fn stop(&self) -> Result<(), JobError> {
        self.schedule.stop().await
    }

    async fn sync(&self) -> Result<(), JobError> {
        self.schedule.sync().await
    }
}
