//! 定时任务端口():**触发**与**执行**是两个正交的轴。
//!
//! - [`TriggerSource`]:谁决定「现在该跑任务 X」——cron tick / 人工 / 外部事件。
//! - [`RunSink`]:这一跑去哪——进程内直调,还是投进队列由 worker 消费。
//!
//! 二者不是对等的替代品:调度器(Cron)解决"什么时候跑",任务队列/Apalis 解决
//! "由谁跑、跑失败怎么办"。共享内核是 [`JobRegistry`]（调用目标白名单）+
//! `cicada-worker::JobExecutor`(并发策略 / 超时 / 重试 / 执行日志)。
//!
//! 调用目标是注册表 key,不是字符串反射:字符串只能命中 [`JobRegistry`] 里显式注册的
//! handler,找不到即失败,从根上排除参考实现定时任务反序列化 RCE。

use std::collections::HashMap;
use std::future::Future;
use std::sync::Arc;

use async_trait::async_trait;

use cicada_kernel::{JobId, TenantId, TraceId, TriggerKind};

#[derive(Debug, thiserror::Error)]
pub enum JobError {
    #[error("未知的任务目标:{0}")]
    UnknownTarget(String),
    #[error("任务定义非法:{0}")]
    Invalid(String),
    #[error("任务执行失败:{0}")]
    Execution(String),
    #[error("分布式锁操作失败:{0}")]
    Lock(String),
    #[error("调度/存储基础设施失败:{0}")]
    Infra(String),
}

/// 交给 handler 的只读上下文。系统上下文:无登录用户,只有租户与链路标识。
#[derive(Clone, Debug)]
pub struct JobContext {
    pub job_id: JobId,
    pub tenant_id: TenantId,
    pub job_name: String,
    pub job_group: String,
    pub invoke_target: String,
    pub trigger_kind: TriggerKind,
    pub trace_id: TraceId,
}

/// 一个可被调度的业务处理器。
#[async_trait]
pub trait JobHandler: Send + Sync {
    /// 返回 `Ok(Some(summary))` 记成功摘要,`Ok(None)` 无摘要,`Err` 记失败。
    async fn run(&self, ctx: &JobContext) -> Result<Option<String>, JobError>;
}

/// 闭包 handler:[`JobRegistry::register_fn`] 的底层载体。
pub struct FnHandler<F>(F);

#[async_trait]
impl<F, Fut> JobHandler for FnHandler<F>
where
    F: Fn(JobContext) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<Option<String>, JobError>> + Send,
{
    async fn run(&self, ctx: &JobContext) -> Result<Option<String>, JobError> {
        (self.0)(ctx.clone()).await
    }
}

/// 显式的 handler 注册表:key → handler。组装根一次性登记,运行期只读。
#[derive(Default, Clone)]
pub struct JobRegistry {
    handlers: HashMap<&'static str, Arc<dyn JobHandler>>,
}

impl JobRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// 登记一个 handler。同键重复登记后写覆盖(组装根负责唯一性)。
    pub fn register(&mut self, key: &'static str, handler: impl JobHandler + 'static) {
        self.handlers.insert(key, Arc::new(handler));
    }

    /// 登记一个闭包 handler。
    pub fn register_fn<F, Fut>(&mut self, key: &'static str, f: F)
    where
        F: Fn(JobContext) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<Option<String>, JobError>> + Send,
    {
        self.register(key, FnHandler(f));
    }

    pub fn get(&self, key: &str) -> Option<Arc<dyn JobHandler>> {
        self.handlers.get(key).cloned()
    }

    pub fn contains(&self, key: &str) -> bool {
        self.handlers.contains_key(key)
    }

    /// 已注册的全部 key(启动期对账 `sys_job.invoke_target` 用)。
    pub fn keys(&self) -> Vec<&'static str> {
        let mut out: Vec<&'static str> = self.handlers.keys().copied().collect();
        out.sort_unstable();
        out
    }

    pub fn len(&self) -> usize {
        self.handlers.len()
    }

    pub fn is_empty(&self) -> bool {
        self.handlers.is_empty()
    }
}

/// 一次触发的结果。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TriggerOutcome {
    /// 已执行(同步执行侧)。`success=false` 表示 handler 失败但已按重试策略跑完。
    Executed {
        success: bool,
        duration_ms: i64,
        message: Option<String>,
    },
    /// 已投递到队列(异步执行侧,如 apalis),由 worker 消费。
    Enqueued,
    /// 因并发策略被跳过。
    Skipped { reason: SkipReason },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SkipReason {
    /// 上一次执行尚未结束(禁止并发)。
    AlreadyRunning,
}

/// 触发侧:产出一系列「该跑任务 X」的决定。**集群下只有 leader 启动它。**
#[async_trait]
pub trait TriggerSource: Send + Sync {
    /// 启动产出(幂等)。
    async fn start(&self) -> Result<(), JobError>;
    /// 停止产出(幂等)。
    async fn stop(&self) -> Result<(), JobError>;
    /// 定义变更后对账(新增 / 删除 / cron 变更);未启动时为空操作。
    async fn sync(&self) -> Result<(), JobError>;
}

/// 执行侧:把一次触发落到某个执行位置。**每个实例都可以启用**(队列模式下即多消费者)。
#[async_trait]
pub trait RunSink: Send + Sync {
    /// 提交一次运行。同步执行侧直接执行并返回 [`TriggerOutcome::Executed`];
    /// 队列执行侧只入队并返回 [`TriggerOutcome::Enqueued`]。
    async fn submit(
        &self,
        tenant: TenantId,
        job_id: JobId,
        kind: TriggerKind,
    ) -> Result<TriggerOutcome, JobError>;
}

/// 未装配时的占位触发源:什么都不做。
#[derive(Default, Clone)]
pub struct NoopTriggerSource;

#[async_trait]
impl TriggerSource for NoopTriggerSource {
    async fn start(&self) -> Result<(), JobError> {
        Ok(())
    }
    async fn stop(&self) -> Result<(), JobError> {
        Ok(())
    }
    async fn sync(&self) -> Result<(), JobError> {
        Ok(())
    }
}

/// 未装配时的占位执行侧:明确失败,避免"静默不执行"。
#[derive(Default, Clone)]
pub struct NoopRunSink;

#[async_trait]
impl RunSink for NoopRunSink {
    async fn submit(
        &self,
        _tenant: TenantId,
        _job_id: JobId,
        _kind: TriggerKind,
    ) -> Result<TriggerOutcome, JobError> {
        Err(JobError::Infra("未装配执行侧(RunSink)".into()))
    }
}

/// 触发 × 执行的组合,是 HTTP 面与组装根共享的入口。
///
/// `producer` 为 `None` 表示只接受人工/外部触发(`jobs.trigger = manual`)。
pub struct JobRuntime {
    sink: Arc<dyn RunSink>,
    producer: Option<Arc<dyn TriggerSource>>,
}

impl JobRuntime {
    pub fn new(sink: Arc<dyn RunSink>, producer: Option<Arc<dyn TriggerSource>>) -> Self {
        Self { sink, producer }
    }

    /// 人工 / 外部触发入口(`PUT /monitor/jobs/{id}/run`)。
    pub async fn submit(
        &self,
        tenant: TenantId,
        job_id: JobId,
        kind: TriggerKind,
    ) -> Result<TriggerOutcome, JobError> {
        self.sink.submit(tenant, job_id, kind).await
    }

    /// 定义变更后让触发侧对账(非 leader 上是空操作)。
    pub async fn sync(&self) -> Result<(), JobError> {
        match &self.producer {
            Some(p) => p.sync().await,
            None => Ok(()),
        }
    }

    pub fn producer(&self) -> Option<Arc<dyn TriggerSource>> {
        self.producer.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn registry_finds_only_registered_keys() {
        let mut reg = JobRegistry::new();
        reg.register_fn("demo:ok", |_ctx| async move { Ok(Some("done".into())) });
        assert!(reg.contains("demo:ok"));
        assert!(!reg.contains("demo:missing"));
        assert_eq!(reg.keys(), vec!["demo:ok"]);

        let handler = reg.get("demo:ok").unwrap();
        let ctx = JobContext {
            job_id: JobId::new(1),
            tenant_id: TenantId::new(1),
            job_name: "demo".into(),
            job_group: "DEFAULT".into(),
            invoke_target: "demo:ok".into(),
            trigger_kind: TriggerKind::Manual,
            trace_id: TraceId::new("t-1"),
        };
        assert_eq!(handler.run(&ctx).await.unwrap(), Some("done".into()));
        assert!(reg.get("demo:missing").is_none());
    }

    #[tokio::test]
    async fn noop_sink_fails_loud() {
        let sink = NoopRunSink;
        let out = sink
            .submit(TenantId::new(1), JobId::new(1), TriggerKind::Manual)
            .await;
        assert!(out.is_err());
    }

    #[tokio::test]
    async fn runtime_without_producer_syncs_as_noop() {
        let runtime = JobRuntime::new(Arc::new(NoopRunSink), None);
        assert!(runtime.sync().await.is_ok());
        assert!(runtime.producer().is_none());
    }
}
