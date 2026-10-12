//! 任务运行时集成测试（真实 PostgreSQL）：并发策略 / 超时重试 / 集群选主 / 队列后端。

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicUsize, Ordering};
use std::time::Duration;

use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectOptions, ConnectionTrait, Database, DatabaseConnection,
    EntityTrait, QueryFilter, Set,
};
use sea_orm_migration::MigratorTraitSelf;

use cicada_infra::Db;
use cicada_infra::config::{DbConfig, Secret};
use cicada_jobs::{JobActiveModel, JobDefinition, JobLogModel, JobStore};
use cicada_kernel::{JobId, TenantId, TriggerKind};
use cicada_ports::{
    DistributedLock, InMemoryDistributedLock, JobContext, JobError, JobRegistry, RunSink,
    TriggerOutcome, TriggerSource,
};
use cicada_worker::runtime::LEADER_KEY;
use cicada_worker::{
    ApalisConsumer, ApalisQueue, ApalisRunSink, CronTriggerSource, ExecutionMode, InlineRunSink,
    JobExecutor, JobRuntimeConfig, TriggerMode, Worker, build_job_runtime,
};

fn url() -> String {
    std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://cicada:2025%40hanmole@localhost:5432/cicada".to_string())
}

/// 每个测试用自己的队列命名空间，共享同一张 `apalis.jobs` 表也互不干扰。
fn namespace(schema: &str) -> String {
    format!("cicada::test::{schema}")
}

async fn queue_for(schema: &str) -> ApalisQueue {
    let queue = ApalisQueue::connect(&url(), &namespace(schema))
        .await
        .unwrap();
    queue.migrate().await.unwrap();
    queue
}

/// 轮询等待执行日志出现，最多等 `secs` 秒。
async fn wait_logs(db: &Db, job_id: JobId, secs: u64) -> Vec<JobLogModel> {
    let deadline = std::time::Instant::now() + Duration::from_secs(secs);
    loop {
        let rows = log_rows(db, job_id).await;
        if !rows.is_empty() || std::time::Instant::now() > deadline {
            return rows;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

async fn connect(schema: Option<&str>) -> DatabaseConnection {
    let mut opt = ConnectOptions::new(url());
    if let Some(s) = schema {
        opt.set_schema_search_path(s.to_string());
    }
    Database::connect(opt).await.unwrap()
}

async fn setup(schema: &str) -> Db {
    let admin = connect(None).await;
    admin
        .execute_unprepared(&format!("DROP SCHEMA IF EXISTS {schema} CASCADE"))
        .await
        .unwrap();
    admin
        .execute_unprepared(&format!("CREATE SCHEMA {schema}"))
        .await
        .unwrap();
    // `apalis.jobs` 是全测试台共享的一张表（`namespace()` 只按 job_type 分道）。
    // 新建 schema 的自增 id 会从 1 重新开始，于是上一轮遗留、这一轮才被取走的队列行
    // 会写出「同一个 job_id 两条日志」——把断言变成竞态。开跑前先把本命名空间的遗留清掉。
    let _ = admin
        .execute_unprepared(&format!(
            "DELETE FROM apalis.jobs WHERE job_type = '{}'",
            namespace(schema)
        ))
        .await;

    let conn = connect(Some(schema)).await;
    cicada_migrations::Migrator.up(&conn, None).await.unwrap();

    Db::connect(&DbConfig {
        url: Secret::new(url()),
        max_connections: 5,
        min_connections: 0,
        acquire_timeout_secs: 5,
        slow_query_ms: 200,
        schema: schema.to_string(),
    })
    .await
    .unwrap()
}

/// 插入一条任务定义，返回任务 id。
async fn insert_job(
    db: &Db,
    name: &str,
    invoke_target: &str,
    concurrency: i16,
    timeout_secs: i64,
    max_retry: i32,
) -> JobId {
    insert_job_cron(
        db,
        name,
        invoke_target,
        concurrency,
        timeout_secs,
        max_retry,
        "0/10 * * * * *",
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn insert_job_cron(
    db: &Db,
    name: &str,
    invoke_target: &str,
    concurrency: i16,
    timeout_secs: i64,
    max_retry: i32,
    cron: &str,
) -> JobId {
    let row = JobActiveModel {
        tenant_id: Set(1),
        job_name: Set(name.to_string()),
        job_group: Set("DEFAULT".to_string()),
        invoke_target: Set(invoke_target.to_string()),
        cron_expression: Set(cron.to_string()),
        concurrency: Set(concurrency),
        timeout_secs: Set(timeout_secs),
        max_retry: Set(max_retry),
        status: Set(0),
        create_time: Set(chrono::Utc::now()),
        ..Default::default()
    }
    .insert(db.conn())
    .await
    .unwrap();
    JobId::new(row.job_id)
}

async fn load_def(db: &Db, job_id: JobId) -> JobDefinition {
    JobStore::new(db.clone())
        .get(TenantId::new(1), job_id)
        .await
        .unwrap()
        .unwrap()
}

async fn log_rows(db: &Db, job_id: JobId) -> Vec<JobLogModel> {
    use cicada_jobs::{JobLogColumn, JobLogEntity};
    JobLogEntity::find()
        .filter(JobLogColumn::JobId.eq(job_id.get()))
        .all(db.conn())
        .await
        .unwrap()
}

fn executor_with(db: &Db, registry: JobRegistry, instance: &str) -> Arc<JobExecutor> {
    Arc::new(JobExecutor::new(
        JobStore::new(db.clone()),
        Arc::new(registry),
        Arc::new(InMemoryDistributedLock::new()),
        instance,
        0,
    ))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn forbid_policy_skips_overlapping_run() {
    let db = setup("t_job_forbid").await;
    let mut registry = JobRegistry::new();
    let runs = Arc::new(AtomicUsize::new(0));
    let r = runs.clone();
    registry.register_fn("slow", move |_ctx: JobContext| {
        let r = r.clone();
        async move {
            r.fetch_add(1, Ordering::SeqCst);
            tokio::time::sleep(Duration::from_millis(300)).await;
            Ok(Some("done".to_string()))
        }
    });
    let ex = executor_with(&db, registry, "inst-forbid");
    // concurrency = 1（禁止并发）。
    let job = insert_job(&db, "串行任务", "slow", 1, 5, 0).await;
    let d = load_def(&db, job).await;

    let second = {
        let ex = ex.clone();
        let d = d.clone();
        async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            ex.execute(&d, TriggerKind::Manual).await
        }
    };
    let (a, b) = tokio::join!(ex.execute(&d, TriggerKind::Manual), second);

    let executed = matches!(a, TriggerOutcome::Executed { success: true, .. })
        || matches!(b, TriggerOutcome::Executed { success: true, .. });
    let skipped =
        matches!(a, TriggerOutcome::Skipped { .. }) || matches!(b, TriggerOutcome::Skipped { .. });
    assert!(executed, "应有一次执行");
    assert!(skipped, "第二次触发应被并发策略跳过");
    assert_eq!(runs.load(Ordering::SeqCst), 1, "handler 只应被调用一次");
    assert_eq!(log_rows(&db, job).await.len(), 1, "跳过不写日志");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn queue_policy_serializes_executions() {
    let db = setup("t_job_queue").await;
    let mut registry = JobRegistry::new();
    let in_flight = Arc::new(AtomicI64::new(0));
    let max_in_flight = Arc::new(AtomicI64::new(0));
    let (i, m) = (in_flight.clone(), max_in_flight.clone());
    registry.register_fn("serial", move |_ctx: JobContext| {
        let (i, m) = (i.clone(), m.clone());
        async move {
            let cur = i.fetch_add(1, Ordering::SeqCst) + 1;
            m.fetch_max(cur, Ordering::SeqCst);
            tokio::time::sleep(Duration::from_millis(150)).await;
            i.fetch_sub(1, Ordering::SeqCst);
            Ok(None)
        }
    });
    let ex = executor_with(&db, registry, "inst-queue");
    // concurrency = 2（队列）。
    let job = insert_job(&db, "队列任务", "serial", 2, 5, 0).await;
    let d = load_def(&db, job).await;

    let (a, b) = tokio::join!(
        ex.execute(&d, TriggerKind::Scheduled),
        ex.execute(&d, TriggerKind::Scheduled)
    );
    assert!(matches!(a, TriggerOutcome::Executed { success: true, .. }));
    assert!(matches!(b, TriggerOutcome::Executed { success: true, .. }));
    assert_eq!(max_in_flight.load(Ordering::SeqCst), 1, "队列策略应串行");
    assert_eq!(log_rows(&db, job).await.len(), 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn timeout_marks_failure() {
    let db = setup("t_job_timeout").await;
    let mut registry = JobRegistry::new();
    registry.register_fn("hang", |_ctx: JobContext| async move {
        tokio::time::sleep(Duration::from_secs(3)).await;
        Ok(None)
    });
    let ex = executor_with(&db, registry, "inst-timeout");
    // concurrency = 0（允许），timeout = 1s。
    let job = insert_job(&db, "超时任务", "hang", 0, 1, 0).await;
    let d = load_def(&db, job).await;

    let outcome = ex.execute(&d, TriggerKind::Scheduled).await;
    match outcome {
        TriggerOutcome::Executed {
            success: false,
            message,
            duration_ms,
        } => {
            assert!(message.unwrap().contains("超时"), "异常摘要应含超时");
            assert!(
                (900..2500).contains(&duration_ms),
                "应在 ~1s 超时：{duration_ms}"
            );
        }
        other => panic!("应超时失败：{other:?}"),
    }
    let rows = log_rows(&db, job).await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].status, 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn retry_until_success_and_log_once() {
    let db = setup("t_job_retry").await;
    let mut registry = JobRegistry::new();
    let calls = Arc::new(AtomicUsize::new(0));
    let c = calls.clone();
    registry.register_fn("flaky", move |_ctx: JobContext| {
        let c = c.clone();
        async move {
            let n = c.fetch_add(1, Ordering::SeqCst) + 1;
            if n < 3 {
                Err(JobError::Execution(format!("第 {n} 次失败")))
            } else {
                Ok(Some("第 3 次成功".to_string()))
            }
        }
    });
    let ex = executor_with(&db, registry, "inst-retry");
    let job = insert_job(&db, "重试任务", "flaky", 0, 5, 3).await;
    let d = load_def(&db, job).await;

    let outcome = ex.execute(&d, TriggerKind::Manual).await;
    assert!(matches!(
        outcome,
        TriggerOutcome::Executed { success: true, .. }
    ));
    assert_eq!(calls.load(Ordering::SeqCst), 3);
    let rows = log_rows(&db, job).await;
    assert_eq!(rows.len(), 1, "重试只落一条最终日志");
    assert_eq!(rows[0].status, 0);
    assert_eq!(rows[0].result.as_deref(), Some("第 3 次成功"));
}

/// apalis 队列后端（真实 PostgreSQL）：入队 → 消费者执行 → 落执行日志。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn apalis_queue_enqueues_and_consumes() {
    let db = setup("t_job_apalis").await;
    let mut registry = JobRegistry::new();
    registry.register_fn("queued", |_ctx: JobContext| async move {
        Ok(Some("apalis-done".to_string()))
    });
    let ex = executor_with(&db, registry, "inst-apalis");
    let job = insert_job(&db, "队列后端任务", "queued", 0, 5, 0).await;

    let queue = queue_for("t_job_apalis").await;
    let consumer = Arc::new(ApalisConsumer::new(queue.clone(), ex));
    let handle = consumer.spawn();

    let outcome = ApalisRunSink::new(queue)
        .submit(TenantId::new(1), job, TriggerKind::Manual)
        .await
        .unwrap();
    assert_eq!(outcome, TriggerOutcome::Enqueued);

    let rows = wait_logs(&db, job, 5).await;
    assert_eq!(rows.len(), 1, "apalis 队列应落一次执行日志");
    assert_eq!(rows[0].status, 0);
    assert_eq!(rows[0].result.as_deref(), Some("apalis-done"));
    assert_eq!(rows[0].trigger_kind, 1, "人工入队应记录为人工触发(1)");
    consumer.stop().await;
    handle.abort();
}

/// 生产默认实现不是进程内队列：消费者没运行时的入队必须留在库里，重启后仍被执行。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn apalis_queue_survives_consumer_restart() {
    let db = setup("t_job_apalis_restart").await;
    let mut registry = JobRegistry::new();
    registry.register_fn("durable", |_ctx: JobContext| async move {
        Ok(Some("durable-done".to_string()))
    });
    let ex = executor_with(&db, registry, "inst-apalis-restart");
    let job = insert_job(&db, "持久化任务", "durable", 0, 5, 0).await;

    let queue = queue_for("t_job_apalis_restart").await;
    // 没有消费者时入队：只应落库，不应执行。
    ApalisRunSink::new(queue.clone())
        .submit(TenantId::new(1), job, TriggerKind::Scheduled)
        .await
        .unwrap();
    assert!(log_rows(&db, job).await.is_empty(), "无消费者时不应执行");

    // 「重启」后再起消费者：仍能取到这条任务。
    let consumer = Arc::new(ApalisConsumer::new(queue, ex));
    let handle = consumer.spawn();
    let rows = wait_logs(&db, job, 10).await;
    consumer.stop().await;
    handle.abort();

    assert_eq!(rows.len(), 1, "队列任务应在消费者启动后被执行");
    assert_eq!(rows[0].trigger_kind, 0, "调度入队应记录为调度触发(0)");
}

/// 回归：cron 生产 + 队列消费必须同时在线，定时任务才会「到点 → 入队 → 执行」。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cron_producer_with_queue_consumer_fires_on_schedule() {
    let db = setup("t_job_apalis_sched").await;
    let mut registry = JobRegistry::new();
    registry.register_fn("tick", |_ctx: JobContext| async move {
        Ok(Some("tick".to_string()))
    });
    let ex = executor_with(&db, registry, "inst-apalis-sched");
    // 每秒触发一次。
    let job = insert_job_cron(&db, "每秒任务", "tick", 0, 5, 0, "*/1 * * * * *").await;

    let queue = queue_for("t_job_apalis_sched").await;
    let consumer = Arc::new(ApalisConsumer::new(queue.clone(), ex.clone()));
    let handle = consumer.spawn();

    let sink: Arc<dyn RunSink> = Arc::new(ApalisRunSink::new(queue));
    let producer = CronTriggerSource::new(ex.store().clone(), sink);
    producer.start().await.unwrap();

    let rows = wait_logs(&db, job, 8).await;
    producer.stop().await.unwrap();
    consumer.stop().await;
    handle.abort();

    assert!(!rows.is_empty(), "cron 生产 + 队列消费应能到点自动触发");
    assert_eq!(rows[0].trigger_kind, 0, "应由调度触发(0),而非人工(1)");
    assert_eq!(rows[0].result.as_deref(), Some("tick"));
}

/// 组装矩阵：`execution = apalis` + `trigger = manual` → 无 cron 生产者，但消费者常驻。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn build_runtime_apalis_manual_runs_via_queue() {
    let db = setup("t_job_build_apalis").await;
    let mut registry = JobRegistry::new();
    registry.register_fn("via-queue", |_ctx: JobContext| async move {
        Ok(Some("via-queue".to_string()))
    });
    let ex = executor_with(&db, registry, "inst-build");
    let job = insert_job(&db, "组装任务", "via-queue", 0, 5, 0).await;

    let cfg = JobRuntimeConfig {
        enabled: true,
        trigger: TriggerMode::Manual,
        execution: ExecutionMode::Apalis,
        queue: namespace("t_job_build_apalis"),
        instance_id: "inst-build".to_string(),
        leader_ttl_secs: 30,
        follower_retry_secs: 1,
        default_timeout_secs: 0,
    };
    let leader_lock: Arc<dyn DistributedLock> = Arc::new(InMemoryDistributedLock::new());
    let handle = build_job_runtime(&cfg, ex, leader_lock, &url())
        .await
        .unwrap();
    assert!(
        handle.runtime.producer().is_none(),
        "manual 不应有 cron 生产者"
    );

    let outcome = handle
        .runtime
        .submit(TenantId::new(1), job, TriggerKind::Manual)
        .await
        .unwrap();
    assert_eq!(outcome, TriggerOutcome::Enqueued);

    let rows = wait_logs(&db, job, 10).await;
    assert_eq!(rows.len(), 1, "消费者应执行入队任务");
    for task in handle.tasks {
        task.abort();
    }
}

/// 记录 start 次数的桩触发源，用于验证选主只让一个 worker 起调度。
#[derive(Default)]
struct RecordingRunner {
    starts: AtomicUsize,
    started: AtomicBool,
}

impl RecordingRunner {
    fn starts(&self) -> usize {
        self.starts.load(Ordering::SeqCst)
    }
}

#[async_trait::async_trait]
impl TriggerSource for RecordingRunner {
    async fn start(&self) -> Result<(), JobError> {
        self.starts.fetch_add(1, Ordering::SeqCst);
        self.started.store(true, Ordering::SeqCst);
        Ok(())
    }
    async fn stop(&self) -> Result<(), JobError> {
        self.started.store(false, Ordering::SeqCst);
        Ok(())
    }
    async fn sync(&self) -> Result<(), JobError> {
        Ok(())
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn leader_election_starts_only_one_scheduler() {
    let lock: Arc<dyn DistributedLock> = Arc::new(InMemoryDistributedLock::new());
    let a = Arc::new(RecordingRunner::default());
    let b = Arc::new(RecordingRunner::default());
    let cfg = JobRuntimeConfig {
        enabled: true,
        trigger: TriggerMode::Cron,
        execution: ExecutionMode::Inline,
        queue: namespace("t_job_leader"),
        instance_id: "cluster".to_string(),
        leader_ttl_secs: 30,
        follower_retry_secs: 1,
        default_timeout_secs: 0,
    };
    let a_handle = Worker::new(a.clone(), lock.clone(), &cfg).spawn();
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert_eq!(a.starts(), 1, "第一个 worker 应取得 leader 并启动调度");

    let _b_handle = Worker::new(b.clone(), lock.clone(), &cfg).spawn();
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert_eq!(b.starts(), 0, "第二个 worker 不应启动调度（选主排斥）");
    assert!(!lock.try_lock(LEADER_KEY, "probe", 30).await.unwrap());
    a_handle.abort();
}

/// 触发源可以真正构造并启动一次（用内存锁 + 空任务表）。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cron_trigger_source_start_stop_is_idempotent() {
    let db = setup("t_job_cron_runner").await;
    let ex = executor_with(&db, JobRegistry::new(), "inst-cron");
    let sink: Arc<dyn RunSink> = Arc::new(InlineRunSink::new(ex.clone()));
    let source = CronTriggerSource::new(ex.store().clone(), sink);
    source.start().await.unwrap();
    source.start().await.unwrap();
    source.stop().await.unwrap();
    source.stop().await.unwrap();
}
