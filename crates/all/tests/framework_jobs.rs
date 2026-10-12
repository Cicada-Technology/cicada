//! 框架自带任务的「注册」与「调度」两端不得漂移（保留期 / 自动建分区任务）。
//!
//! `register_fn` 只挂 handler，真正让它跑起来的是 `sys_job` 里的那一行；两边各写一份
//! 字符串字面量时，改一边就静默失效——任务列表里看着正常，日志分区窗口却再也不动。

#[cfg(feature = "migrations")]
#[test]
fn seeded_framework_jobs_are_registered_by_the_assembly_root() {
    assert_eq!(
        cicada_migrations::seed::FRAMEWORK_JOB_TARGETS,
        [cicada::bootstrap::JOB_LOGS_RETENTION],
        "seed 里的调度行与组装根注册的 handler 对不上"
    );
}
