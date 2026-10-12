//! 定时任务域实体。
//!
//! 命名约定与 `cicada-system` 一致：每个子模块的 `Entity` / `Model` / `Column` /
//! `ActiveModel` 在此统一重命名为 `JobXxx` / `JobLogXxx`。

pub mod job;
pub mod job_log;

pub use job::{
    ActiveModel as JobActiveModel, Column as JobColumn, Entity as JobEntity, Model as JobModel,
    Relation as JobRelation,
};
pub use job_log::{
    ActiveModel as JobLogActiveModel, Column as JobLogColumn, Entity as JobLogEntity,
    Model as JobLogModel, Relation as JobLogRelation,
};
