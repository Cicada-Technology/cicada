//! cron 调度底座:维护 `tokio-cron-scheduler` 的登记集合,并在到点时回调消费方。
//!
//! 触发来源与执行方式解耦:本模块只负责「到点」,到点之后交给谁由执行侧决定 ——
//! [`crate::inline::InlineRunSink`]（回调 = 就地执行）或
//! [`crate::apalis::ApalisRunSink`]（回调 = 投递进 apalis 队列,由 [`crate::apalis::ApalisConsumer`] 消费）。
//!
//! 只有 leader 会 `start`;`sync` 做增量对账(新增 / 删除 / cron 变更),未启动时是空操作,
//! 因此 follower 上的 `sync` 不会误起本地调度。

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use tokio::sync::Mutex;
use tokio_cron_scheduler::{Job, JobScheduler};
use uuid::Uuid;

use cicada_jobs::{JobDefinition, JobStore};
use cicada_ports::JobError;

/// 到点回调:拿到完整任务定义,自行决定「执行」还是「入队」。
pub type FireCallback =
    Arc<dyn Fn(JobDefinition) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync>;

/// 已登记的一条调度:调度器 guid + 当时的 cron(用于检测变更)。
#[derive(Clone, Debug)]
struct Scheduled {
    uuid: Uuid,
    cron: String,
}

fn infra(e: impl std::fmt::Display) -> JobError {
    JobError::Infra(e.to_string())
}

pub struct CronSchedule {
    store: JobStore,
    on_fire: FireCallback,
    scheduler: Mutex<Option<JobScheduler>>,
    ids: Mutex<HashMap<i64, Scheduled>>,
}

impl CronSchedule {
    pub fn new(store: JobStore, on_fire: FireCallback) -> Self {
        Self {
            store,
            on_fire,
            scheduler: Mutex::new(None),
            ids: Mutex::new(HashMap::new()),
        }
    }

    async fn register_one(
        sched: &JobScheduler,
        def: &JobDefinition,
        on_fire: &FireCallback,
    ) -> Result<Uuid, JobError> {
        let captured = def.clone();
        let on_fire = on_fire.clone();
        let job = Job::new_async(def.cron_expression.as_str(), move |_uuid, _lock| {
            let def = captured.clone();
            let on_fire = on_fire.clone();
            on_fire(def)
        })
        .map_err(|e| JobError::Invalid(format!("cron 表达式非法:{e}")))?;
        sched.add(job).await.map_err(infra)
    }

    /// 启动调度(幂等)。仅 leader 调用。
    pub async fn start(&self) -> Result<(), JobError> {
        let mut guard = self.scheduler.lock().await;
        if guard.is_some() {
            return Ok(());
        }
        let sched = JobScheduler::new().await.map_err(infra)?;
        let mut ids = HashMap::new();
        for def in self.store.list_schedulable().await.map_err(infra)? {
            let uuid = Self::register_one(&sched, &def, &self.on_fire).await?;
            ids.insert(
                def.job_id.get(),
                Scheduled {
                    uuid,
                    cron: def.cron_expression.clone(),
                },
            );
        }
        sched.start().await.map_err(infra)?;
        let count = ids.len();
        *self.ids.lock().await = ids;
        *guard = Some(sched);
        tracing::info!(count, "cron 调度已启动");
        Ok(())
    }

    /// 停止调度(幂等)。
    pub async fn stop(&self) -> Result<(), JobError> {
        let mut guard = self.scheduler.lock().await;
        if let Some(mut sched) = guard.take() {
            sched.shutdown().await.map_err(infra)?;
            tracing::info!("cron 调度已停止");
        }
        self.ids.lock().await.clear();
        Ok(())
    }

    /// 增量对账当前任务定义;未启动(非 leader)时为空操作。
    pub async fn sync(&self) -> Result<(), JobError> {
        let guard = self.scheduler.lock().await;
        let Some(sched) = guard.as_ref() else {
            return Ok(());
        };
        let defs = self.store.list_schedulable().await.map_err(infra)?;
        let mut current = self.ids.lock().await.clone();

        let desired: HashMap<i64, &JobDefinition> =
            defs.iter().map(|d| (d.job_id.get(), d)).collect();

        // 删除:不再调度,或 cron 已变更(改 cron 需重建 job)。
        let stale: Vec<(i64, Uuid)> = current
            .iter()
            .filter(|(id, s)| match desired.get(id) {
                Some(d) => d.cron_expression != s.cron,
                None => true,
            })
            .map(|(id, s)| (*id, s.uuid))
            .collect();
        for (id, uuid) in stale {
            if let Err(e) = sched.remove(&uuid).await {
                tracing::warn!(error = %e, "移除旧调度失败");
            }
            current.remove(&id);
        }

        // 新增 / 变更后重建。
        for def in &defs {
            let id = def.job_id.get();
            let need = current
                .get(&id)
                .map(|s| s.cron != def.cron_expression)
                .unwrap_or(true);
            if need {
                let uuid = Self::register_one(sched, def, &self.on_fire).await?;
                current.insert(
                    id,
                    Scheduled {
                        uuid,
                        cron: def.cron_expression.clone(),
                    },
                );
            }
        }

        *self.ids.lock().await = current;
        Ok(())
    }
}
