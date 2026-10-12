//! cicada-worker（L4 接入层，可选独立进程）：后台任务 / 定时任务运行时。
//!
//! 触发与执行分成两条正交的轴，各有一个端口（见 `cicada-ports::job`）：
//! - 触发侧 [`runtime::TriggerMode`]：[`cron::CronTriggerSource`]（进程内 cron）
//!   或 `manual`（只接受人工 / 外部触发）。
//! - 执行侧 [`runtime::ExecutionMode`]：[`inline::InlineRunSink`]（触发即执行，默认）
//!   或 [`apalis::ApalisRunSink`]（投递到 PostgreSQL 队列，由 [`apalis::ApalisConsumer`] 消费）。
//!
//! 两侧共享 [`executor::JobExecutor`]：并发策略、超时、失败重试、执行日志只有一份实现。
//! 集群下只有触发侧需要选主（[`runtime::Worker`]），执行侧每个实例都启用。

pub mod apalis;
pub mod cron;
pub mod executor;
pub mod inline;
pub mod lock;
pub mod runtime;
pub mod schedule;

pub use apalis::{ApalisConsumer, ApalisQueue, ApalisRunSink, JobEnvelope};
pub use cron::CronTriggerSource;
pub use executor::JobExecutor;
pub use inline::InlineRunSink;
pub use lock::RedisDistributedLock;
pub use runtime::{
    ExecutionMode, JobRuntimeConfig, JobRuntimeHandle, TriggerMode, Worker, build_job_runtime,
};
