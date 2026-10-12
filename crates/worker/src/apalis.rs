//! 执行侧：`apalis` + PostgreSQL 队列。
//!
//! 三层职责分清，别把队列当成调度器：
//! - **生产者**：cron（[`crate::cron`]）或人工/外部触发，只做入队。
//! - **队列**：`apalis-sql` 的 [`PostgresStorage`] 把任务落在 `apalis.jobs`
//!   （`FOR UPDATE SKIP LOCKED` 取活 + `NOTIFY` 唤醒）。重启不丢、多实例可竞争消费。
//! - **消费者**：[`ApalisConsumer`] 在**每个实例**都跑，取到信封后读任务定义，
//!   交给共享的 [`JobExecutor`] 执行（并发策略 / 超时 / 重试 / 执行日志仍只有一份）。
//!
//! 基座不使用 `apalis` 的 `MemoryStorage`：进程内队列重启即丢，不能作为生产默认实现。
//! 队列的可靠性由数据库保证，与「谁来触发」正交——因此触发侧选主与消费者无关。

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use apalis::prelude::{
    Data, Error as ApalisError, Monitor, Request, Storage, WorkerBuilder, WorkerFactoryFn,
};
use apalis_sql::Config as SqlConfig;
use apalis_sql::postgres::{PgPool, PostgresStorage};
use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex, oneshot};

use cicada_jobs::JobStore;
use cicada_kernel::{JobId, TenantId, TriggerKind};
use cicada_ports::{JobError, RunSink, TriggerOutcome};

use crate::executor::JobExecutor;

/// 入队负载：只带定位信息与触发来源，真正的定义在执行前从库里读（改任务即时生效）。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct JobEnvelope {
    pub tenant_id: i64,
    pub job_id: i64,
    /// `TriggerKind` 的编码；入队时记下，执行日志才能区分调度触发与人工触发。
    pub trigger_kind: i16,
}

impl JobEnvelope {
    fn of(tenant: TenantId, job_id: JobId, kind: TriggerKind) -> Self {
        Self {
            tenant_id: tenant.get(),
            job_id: job_id.get(),
            trigger_kind: kind.code(),
        }
    }
}

/// PostgreSQL 队列句柄：生产者入队、消费者取活共用。
#[derive(Clone)]
pub struct ApalisQueue {
    storage: PostgresStorage<JobEnvelope>,
}

impl ApalisQueue {
    /// 连库并绑定队列命名空间。
    ///
    /// `namespace` 是 apalis 的 `job_type`，用于在同一个 `apalis.jobs` 表里隔离
    /// 不同应用与不同测试；取活时只认自己的命名空间。
    pub async fn connect(database_url: &str, namespace: &str) -> Result<Self, JobError> {
        let pool = PgPool::connect(database_url)
            .await
            .map_err(|e| JobError::Infra(format!("连接任务队列库失败：{e}")))?;
        Ok(Self {
            storage: PostgresStorage::new_with_config(pool, SqlConfig::new(namespace)),
        })
    }

    /// 建表 / 升级 apalis 自身的 `apalis` schema（幂等，走 sqlx migrations）。
    pub async fn migrate(&self) -> Result<(), JobError> {
        PostgresStorage::setup(self.storage.pool())
            .await
            .map_err(|e| JobError::Infra(format!("初始化任务队列表失败：{e}")))
    }

    pub fn pool(&self) -> &PgPool {
        self.storage.pool()
    }

    /// 入队一次运行，返回 apalis 任务 id（排障用）。
    async fn enqueue(
        &self,
        tenant: TenantId,
        job_id: JobId,
        kind: TriggerKind,
    ) -> Result<String, JobError> {
        // `Storage::push` 需要 `&mut self`，但底层只是一个连接池，克隆代价极低。
        let mut storage = self.storage.clone();
        let parts = storage
            .push_request(Request::new(JobEnvelope::of(tenant, job_id, kind)))
            .await
            .map_err(|e| JobError::Infra(format!("任务入队失败：{e}")))?;
        Ok(parts.task_id.to_string())
    }
}

/// 队列执行侧：`submit` 只入队，立即返回 [`TriggerOutcome::Enqueued`]。
pub struct ApalisRunSink {
    queue: ApalisQueue,
}

impl ApalisRunSink {
    pub fn new(queue: ApalisQueue) -> Self {
        Self { queue }
    }
}

#[async_trait::async_trait]
impl RunSink for ApalisRunSink {
    async fn submit(
        &self,
        tenant: TenantId,
        job_id: JobId,
        kind: TriggerKind,
    ) -> Result<TriggerOutcome, JobError> {
        self.queue.enqueue(tenant, job_id, kind).await?;
        Ok(TriggerOutcome::Enqueued)
    }
}

/// 注入到 apalis worker 的共享依赖。
#[derive(Clone)]
struct JobShared {
    store: JobStore,
    executor: Arc<JobExecutor>,
}

/// apalis 服务函数：把队列里的信封还原成一次真实执行。
async fn handle(env: JobEnvelope, data: Data<JobShared>) -> Result<(), ApalisError> {
    let tenant = TenantId::new(env.tenant_id);
    let job_id = JobId::new(env.job_id);
    let kind = TriggerKind::from_code(env.trigger_kind);
    match data.store.get(tenant, job_id).await {
        Ok(Some(def)) => {
            data.executor.execute(&def, kind).await;
        }
        // 任务在入队后被删除：消费时按「已不存在」跳过，不重试。
        Ok(None) => tracing::warn!(job_id = env.job_id, "队列中的任务已不存在，跳过"),
        Err(e) => tracing::error!(error = %e, job_id = env.job_id, "读取任务定义失败"),
    }
    Ok(())
}

/// 队列消费者：**每个实例都应启动**（队列模式下的多消费者）。
pub struct ApalisConsumer {
    queue: ApalisQueue,
    shared: JobShared,
    shutdown: Mutex<Option<oneshot::Sender<()>>>,
    started: AtomicBool,
}

impl ApalisConsumer {
    pub fn new(queue: ApalisQueue, executor: Arc<JobExecutor>) -> Self {
        let shared = JobShared {
            store: executor.store().clone(),
            executor,
        };
        Self {
            queue,
            shared,
            shutdown: Mutex::new(None),
            started: AtomicBool::new(false),
        }
    }

    /// 后台启动消费者，返回句柄。进程存活期间不断取活。
    pub fn spawn(self: &Arc<Self>) -> tokio::task::JoinHandle<()> {
        let this = self.clone();
        tokio::spawn(async move { this.run().await })
    }

    async fn run(&self) {
        if self.started.swap(true, Ordering::SeqCst) {
            return;
        }
        let worker = WorkerBuilder::new("cicada-jobs")
            .data(self.shared.clone())
            .backend(self.queue.storage.clone())
            .build_fn(handle);
        let (tx, rx) = oneshot::channel::<()>();
        *self.shutdown.lock().await = Some(tx);
        let signal = async move {
            let _ = rx.await;
            Ok::<(), std::io::Error>(())
        };
        tracing::info!(
            namespace = %self.queue.storage.config().namespace(),
            "apalis 队列消费者已启动"
        );
        if let Err(e) = Monitor::new()
            .register(worker)
            .run_with_signal(signal)
            .await
        {
            tracing::error!(error = %e, "apalis worker 退出");
        }
    }

    /// 停止消费者（幂等）。
    pub async fn stop(&self) {
        if !self.started.swap(false, Ordering::SeqCst) {
            return;
        }
        if let Some(tx) = self.shutdown.lock().await.take() {
            let _ = tx.send(());
        }
    }
}
