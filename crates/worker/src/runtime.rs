//! 调度运行时与集群选主。
//!
//! 触发与执行是两条**正交**的轴，分别可配：
//! - **触发侧**：[`TriggerMode::Cron`]（进程内 cron 生产）或 [`TriggerMode::Manual`]
//!   （只接受人工 / 外部触发）。集群多实例时用 Redis 分布式锁选主，
//!   **只有 leader** 启动 cron 生产，避免重复触发。
//! - **执行侧**：[`ExecutionMode::Inline`]（触发即执行）或 [`ExecutionMode::Apalis`]
//!   （投递到 PostgreSQL 队列）。执行侧**每个实例都启用**——队列模式下即多消费者。
//!
//! 锁 token 绑定 `instance_id` 与随机数，续租失败即退位并停止触发。

use std::sync::Arc;
use std::time::Duration;

use cicada_infra::JobConfig;
use cicada_ports::{DistributedLock, JobError, JobRuntime, RunSink, TriggerSource};

use crate::apalis::{ApalisConsumer, ApalisQueue, ApalisRunSink};
use crate::cron::CronTriggerSource;
use crate::executor::JobExecutor;
use crate::inline::InlineRunSink;

/// 选主锁的稳定键。
pub const LEADER_KEY: &str = "cicada:job:leader";

/// 触发方式（`jobs.trigger`）：谁决定「现在该跑任务 X」。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TriggerMode {
    /// 进程内 cron 生产（集群下仅 leader）。
    Cron,
    /// 不自动产出，只接受人工（`PUT /monitor/jobs/{id}/run`）或外部触发。
    Manual,
}

impl TriggerMode {
    pub fn from_key(s: &str) -> Self {
        if s.eq_ignore_ascii_case("manual") {
            Self::Manual
        } else {
            Self::Cron
        }
    }
}

/// 执行方式（`jobs.execution`）：这一跑去哪执行。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExecutionMode {
    /// 触发即执行（默认）。
    Inline,
    /// 投递到 PostgreSQL 队列，由消费者执行（可靠投递 / 多消费者）。
    Apalis,
}

impl ExecutionMode {
    pub fn from_key(s: &str) -> Self {
        if s.eq_ignore_ascii_case("apalis") {
            Self::Apalis
        } else {
            Self::Inline
        }
    }
}

/// 运行时可调参数（从 `cicada_infra::JobConfig` 派生）。
#[derive(Clone, Debug)]
pub struct JobRuntimeConfig {
    /// 是否启用后台运行时（触发侧选主 + 执行侧消费者）。
    pub enabled: bool,
    pub trigger: TriggerMode,
    pub execution: ExecutionMode,
    /// apalis 队列命名空间（`execution = apalis` 时生效）。
    pub queue: String,
    pub instance_id: String,
    pub leader_ttl_secs: u64,
    pub follower_retry_secs: u64,
    pub default_timeout_secs: u64,
}

impl JobRuntimeConfig {
    /// 用配置派生，`instance_id` 缺省时生成一个进程唯一值。
    pub fn from_config(cfg: &JobConfig) -> Self {
        let instance_id = cfg
            .instance_id
            .clone()
            .unwrap_or_else(|| format!("cicada-{}", uuid::Uuid::new_v4()));
        Self {
            enabled: cfg.enabled,
            trigger: TriggerMode::from_key(&cfg.trigger),
            execution: ExecutionMode::from_key(&cfg.execution),
            queue: cfg.queue.clone(),
            instance_id,
            leader_ttl_secs: cfg.leader_ttl_secs.max(5),
            follower_retry_secs: cfg.follower_retry_secs.max(1),
            default_timeout_secs: cfg.default_timeout_secs,
        }
    }
}

/// 组装结果：HTTP 面共享的 [`JobRuntime`] + 需保活的后台任务句柄。
pub struct JobRuntimeHandle {
    pub runtime: Arc<JobRuntime>,
    pub tasks: Vec<tokio::task::JoinHandle<()>>,
}

/// 按配置组装触发侧与执行侧。
///
/// 执行侧总是构造（人工 `/run` 不依赖后台任务是否启用）；只有当 `cfg.enabled`
/// 时才真正起消费者与 leader 选主。
pub async fn build_job_runtime(
    cfg: &JobRuntimeConfig,
    executor: Arc<JobExecutor>,
    leader_lock: Arc<dyn DistributedLock>,
    database_url: &str,
) -> Result<JobRuntimeHandle, JobError> {
    let mut tasks = Vec::new();

    let sink: Arc<dyn RunSink> = match cfg.execution {
        ExecutionMode::Inline => Arc::new(InlineRunSink::new(executor.clone())),
        ExecutionMode::Apalis => {
            let queue = ApalisQueue::connect(database_url, &cfg.queue).await?;
            queue.migrate().await?;
            if cfg.enabled {
                let consumer = Arc::new(ApalisConsumer::new(queue.clone(), executor.clone()));
                tasks.push(consumer.spawn());
            }
            Arc::new(ApalisRunSink::new(queue))
        }
    };

    let producer: Option<Arc<dyn TriggerSource>> = match cfg.trigger {
        TriggerMode::Cron => Some(Arc::new(CronTriggerSource::new(
            executor.store().clone(),
            sink.clone(),
        ))),
        TriggerMode::Manual => None,
    };

    if cfg.enabled
        && let Some(source) = &producer
    {
        tasks.push(Worker::new(source.clone(), leader_lock, cfg).spawn());
    }

    Ok(JobRuntimeHandle {
        runtime: Arc::new(JobRuntime::new(sink, producer)),
        tasks,
    })
}

/// leader 选主 + 触发侧生命周期。用 `watch` 信号优雅退位。
pub struct Worker {
    source: Arc<dyn TriggerSource>,
    lock: Arc<dyn DistributedLock>,
    token: String,
    ttl_secs: u64,
    retry_secs: u64,
}

impl Worker {
    pub fn new(
        source: Arc<dyn TriggerSource>,
        lock: Arc<dyn DistributedLock>,
        cfg: &JobRuntimeConfig,
    ) -> Self {
        Self {
            source,
            lock,
            token: format!("{}:{}", cfg.instance_id, uuid::Uuid::new_v4()),
            ttl_secs: cfg.leader_ttl_secs,
            retry_secs: cfg.follower_retry_secs,
        }
    }

    /// 后台启动，返回句柄。进程存活期间不退位。
    pub fn spawn(self) -> tokio::task::JoinHandle<()> {
        let (tx, rx) = tokio::sync::watch::channel(false);
        tokio::spawn(async move {
            let _keep_tx_alive = tx;
            self.run(rx).await;
        })
    }

    /// 启动并监听优雅退出信号。
    pub async fn run(self, mut shutdown: tokio::sync::watch::Receiver<bool>) {
        tracing::info!(instance = %self.token, "定时任务触发运行时启动");
        let renew_every = Duration::from_secs((self.ttl_secs / 3).max(1));
        let retry_every = Duration::from_secs(self.retry_secs);

        while !*shutdown.borrow() {
            match self
                .lock
                .try_lock(LEADER_KEY, &self.token, self.ttl_secs)
                .await
            {
                Ok(true) => {
                    tracing::info!(instance = %self.token, "已选为 leader，开始产出触发");
                    if let Err(e) = self.source.start().await {
                        tracing::error!(error = %e, "启动触发侧失败");
                    }
                    // 持有租约并续期，直到续租失败或收到退出信号。
                    loop {
                        tokio::select! {
                            _ = tokio::time::sleep(renew_every) => {
                                match self.lock.renew(LEADER_KEY, &self.token, self.ttl_secs).await {
                                    Ok(true) => {
                                        // 周期对账：把别的实例上刚改过的任务定义同步进来。
                                        if let Err(e) = self.source.sync().await {
                                            tracing::warn!(error = %e, "任务定义对账失败");
                                        }
                                    }
                                    Ok(false) => {
                                        tracing::warn!("leader 租约丢失，退位");
                                        break;
                                    }
                                    Err(e) => tracing::warn!(error = %e, "续租失败，重试"),
                                }
                            }
                            _ = shutdown.changed() => break,
                        }
                    }
                    let _ = self.source.stop().await;
                    let _ = self.lock.unlock(LEADER_KEY, &self.token).await;
                }
                Ok(false) => {
                    tracing::debug!(instance = %self.token, "非 leader，等待重试");
                    tokio::select! {
                        _ = tokio::time::sleep(retry_every) => {}
                        _ = shutdown.changed() => {}
                    }
                }
                Err(e) => {
                    tracing::warn!(error = %e, "选主失败，稍后重试");
                    tokio::select! {
                        _ = tokio::time::sleep(retry_every) => {}
                        _ = shutdown.changed() => {}
                    }
                }
            }
        }
        tracing::info!(instance = %self.token, "定时任务触发运行时退出");
    }
}
