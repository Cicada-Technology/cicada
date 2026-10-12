//! cicada-jobs（L2 领域）
//!
//! 定时任务定义与执行日志：`sys_job` / `sys_job_log` 的唯一属主。
//! 调度运行时（`cicada-worker`）只经 [`store::JobStore`] 读写这里的数据。
//!
//! 调用目标是**注册表 key**，不是字符串反射：Rust 没有反射，字符串只能映射到
//! `cicada-ports::JobRegistry` 里显式注册的 handler，从根上排除参考实现的
//! 定时任务反序列化 RCE。

pub mod cron;
pub mod entity;
pub mod store;

pub use cron::validate_cron;
pub use entity::{
    JobActiveModel, JobColumn, JobEntity, JobLogActiveModel, JobLogColumn, JobLogEntity,
    JobLogModel, JobModel,
};
pub use store::{JobDefinition, JobStatus, JobStore, JobStoreError, NewJobLog};
