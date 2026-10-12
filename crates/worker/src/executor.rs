//! 任务执行器：并发策略 / 超时 / 重试 / 执行日志的唯一实现。
//!
//! 触发来源（cron 调度、apalis 队列、人工 `/run`）只是入口不同，执行语义一致。

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::Utc;
use tokio::sync::{Mutex, OwnedMutexGuard};

use cicada_infra::metrics::JOB_RUNS;
use cicada_jobs::{JobDefinition, JobStore, NewJobLog};
use cicada_kernel::{ConcurrencyPolicy, TraceId, TriggerKind};
use cicada_ports::{
    DistributedLock, JobContext, JobError, JobHandler, JobRegistry, SkipReason, TriggerOutcome,
};

/// 单条日志里结果/异常摘要的最大字符数。
const MAX_RESULT_CHARS: usize = 2000;

/// 共享执行器。廉价克隆（内部全是 `Arc`）。
#[derive(Clone)]
pub struct JobExecutor {
    store: JobStore,
    registry: Arc<JobRegistry>,
    lock: Arc<dyn DistributedLock>,
    instance_id: String,
    default_timeout_secs: u64,
    /// 队列策略的实例内串行闸门，按任务 id 懒建。
    queue_gates: Arc<Mutex<HashMap<i64, Arc<Mutex<()>>>>>,
}

impl JobExecutor {
    pub fn new(
        store: JobStore,
        registry: Arc<JobRegistry>,
        lock: Arc<dyn DistributedLock>,
        instance_id: impl Into<String>,
        default_timeout_secs: u64,
    ) -> Self {
        Self {
            store,
            registry,
            lock,
            instance_id: instance_id.into(),
            default_timeout_secs,
            queue_gates: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn instance_id(&self) -> &str {
        &self.instance_id
    }

    pub fn store(&self) -> &JobStore {
        &self.store
    }

    pub fn registry(&self) -> &JobRegistry {
        &self.registry
    }

    /// 执行一次任务，落 `sys_job_log`，返回结果。并发策略在入口处短路。
    pub async fn execute(&self, def: &JobDefinition, trigger: TriggerKind) -> TriggerOutcome {
        let run_token = format!("{}-{}", self.instance_id, uuid::Uuid::new_v4());
        let lock_key = format!("cicada:job:run:{}", def.job_id.get());
        let ttl = self.lock_ttl(def);

        let mut held_lock = false;
        let mut _queue_guard: Option<OwnedMutexGuard<()>> = None;

        match def.concurrency {
            ConcurrencyPolicy::Forbid => match self.lock.try_lock(&lock_key, &run_token, ttl).await
            {
                Ok(true) => held_lock = true,
                Ok(false) => {
                    count_run("skipped");
                    return TriggerOutcome::Skipped {
                        reason: SkipReason::AlreadyRunning,
                    };
                }
                Err(e) => {
                    // 选不到锁时不冒险并发执行：失败关闭为「跳过」。
                    tracing::warn!(job = %def.job_name, error = %e, "获取任务运行锁失败，跳过本次触发");
                    count_run("skipped");
                    return TriggerOutcome::Skipped {
                        reason: SkipReason::AlreadyRunning,
                    };
                }
            },
            ConcurrencyPolicy::Queue => {
                let gate = {
                    let mut gates = self.queue_gates.lock().await;
                    gates
                        .entry(def.job_id.get())
                        .or_insert_with(|| Arc::new(Mutex::new(())))
                        .clone()
                };
                _queue_guard = Some(gate.lock_owned().await);
            }
            ConcurrencyPolicy::Allow => {}
        }

        let started = Instant::now();
        let start_time = Utc::now();
        let trace_id = TraceId::new(uuid::Uuid::new_v4().to_string());
        let ctx = JobContext {
            job_id: def.job_id,
            tenant_id: def.tenant_id,
            job_name: def.job_name.clone(),
            job_group: def.job_group.clone(),
            invoke_target: def.invoke_target.clone(),
            trigger_kind: trigger,
            trace_id: trace_id.clone(),
        };

        let (success, message) = self.run_with_retry(def, &ctx).await;
        count_run(if success { "success" } else { "failed" });

        let end_time = Utc::now();
        let duration_ms = started.elapsed().as_millis() as i64;

        let log = NewJobLog {
            tenant_id: def.tenant_id,
            job_id: def.job_id,
            job_name: def.job_name.clone(),
            job_group: def.job_group.clone(),
            invoke_target: def.invoke_target.clone(),
            instance_id: self.instance_id.clone(),
            trigger_kind: trigger.code(),
            start_time,
            end_time,
            duration_ms,
            status: if success { 0 } else { 1 },
            result: message.clone().map(|m| truncate(&m, MAX_RESULT_CHARS)),
            trace_id: Some(trace_id.as_str().to_string()),
        };
        if let Err(e) = self.store.insert_log(log).await {
            tracing::error!(job = %def.job_name, error = %e, "写任务执行日志失败");
        }

        if held_lock && let Err(e) = self.lock.unlock(&lock_key, &run_token).await {
            tracing::warn!(job = %def.job_name, error = %e, "释放任务运行锁失败");
        }

        TriggerOutcome::Executed {
            success,
            duration_ms,
            message,
        }
    }

    /// 直接执行一个已解析定义（不含并发控制），供测试与内部编排使用。
    pub async fn run_handler(
        &self,
        def: &JobDefinition,
        trigger: TriggerKind,
    ) -> Result<Option<String>, JobError> {
        let ctx = JobContext {
            job_id: def.job_id,
            tenant_id: def.tenant_id,
            job_name: def.job_name.clone(),
            job_group: def.job_group.clone(),
            invoke_target: def.invoke_target.clone(),
            trigger_kind: trigger,
            trace_id: TraceId::new(uuid::Uuid::new_v4().to_string()),
        };
        let handler = self
            .registry
            .get(&def.invoke_target)
            .ok_or_else(|| JobError::UnknownTarget(def.invoke_target.clone()))?;
        self.call_with_timeout(&handler, &ctx, def.timeout_secs)
            .await
    }

    async fn run_with_retry(
        &self,
        def: &JobDefinition,
        ctx: &JobContext,
    ) -> (bool, Option<String>) {
        let Some(handler) = self.registry.get(&def.invoke_target) else {
            return (
                false,
                Some(format!("未知的任务目标：{}", def.invoke_target)),
            );
        };
        let attempts = def.max_retry as usize + 1;
        let mut last: Option<String> = None;
        for attempt in 0..attempts {
            match self
                .call_with_timeout(&handler, ctx, def.timeout_secs)
                .await
            {
                Ok(msg) => return (true, msg),
                Err(e) => {
                    last = Some(e.to_string());
                    if attempt + 1 < attempts {
                        // 线性退避：200ms * (attempt + 1)，上限 5s。
                        let backoff = Duration::from_millis(200 * (attempt as u64 + 1))
                            .min(Duration::from_secs(5));
                        tokio::time::sleep(backoff).await;
                    }
                }
            }
        }
        (false, last)
    }

    async fn call_with_timeout(
        &self,
        handler: &Arc<dyn JobHandler>,
        ctx: &JobContext,
        timeout_secs: u64,
    ) -> Result<Option<String>, JobError> {
        let effective = if timeout_secs > 0 {
            timeout_secs
        } else {
            self.default_timeout_secs
        };
        if effective == 0 {
            handler.run(ctx).await
        } else {
            match tokio::time::timeout(Duration::from_secs(effective), handler.run(ctx)).await {
                Ok(res) => res,
                Err(_) => Err(JobError::Execution(format!("执行超时（{effective}s）"))),
            }
        }
    }

    /// 「禁止并发」锁的 TTL：留出超时 + 重试的余量，避免锁提前过期导致重叠执行。
    fn lock_ttl(&self, def: &JobDefinition) -> u64 {
        lock_ttl_secs(def.timeout_secs, def.max_retry, self.default_timeout_secs)
    }
}

/// 「禁止并发」锁的 TTL 计算：留出超时 + 重试的余量。
fn lock_ttl_secs(timeout_secs: u64, max_retry: u32, default_timeout_secs: u64) -> u64 {
    let timeout = if timeout_secs > 0 {
        timeout_secs
    } else {
        default_timeout_secs
    };
    if timeout == 0 {
        return 3600;
    }
    let attempts = max_retry as u64 + 1;
    (timeout * attempts).max(30) + 30
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max).collect();
    out.push('…');
    out
}

/// 任务执行结果计数：`job_runs_total{status}`，status 取
/// `success` / `failed` / `skipped`。
fn count_run(status: &'static str) {
    metrics::counter!(JOB_RUNS, "status" => status).increment(1);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_keeps_short_and_clips_long() {
        assert_eq!(truncate("abc", 10), "abc");
        let long = "x".repeat(50);
        let clipped = truncate(&long, 10);
        assert_eq!(clipped.chars().count(), 11); // 10 + 省略号
        assert!(clipped.ends_with('…'));
    }

    #[test]
    fn lock_ttl_has_floor_and_grows_with_attempts() {
        assert_eq!(lock_ttl_secs(10, 2, 0), 10 * 3 + 30);
        // 未声明超时时用兜底值。
        assert_eq!(lock_ttl_secs(0, 0, 60), 60 + 30);
        // 完全不限时用 1 小时锁。
        assert_eq!(lock_ttl_secs(0, 0, 0), 3600);
    }
}
