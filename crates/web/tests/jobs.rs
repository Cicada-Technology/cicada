//! 定时任务模块集成测试（真实 PostgreSQL）：CRUD / 状态切换 / 人工触发 / 执行日志 / 导出。

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use sea_orm::{ConnectOptions, ConnectionTrait, Database, DatabaseConnection};
use sea_orm_migration::MigratorTraitSelf;
use tower::ServiceExt;

use cicada_audit::InMemoryAuditSink;
use cicada_auth::{InMemorySessionStore, Session, SessionStore, TokenService, hash_refresh};
use cicada_infra::Db;
use cicada_infra::config::{Config, DbConfig, Secret};
use cicada_jobs::{JobStore, validate_cron};
use cicada_ports::{InMemoryDistributedLock, JobContext, JobRegistry, JobRuntime, RunSink};
use cicada_rbac::{AuthzCache, InMemoryAuthzStore};
use cicada_system::LocalDeptDirectory;
use cicada_web::state::AppState;
use cicada_worker::{CronTriggerSource, InlineRunSink, JobExecutor};

const JWT_SECRET: &str = "test-secret";

fn url() -> String {
    std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://cicada:2025%40hanmole@localhost:5432/cicada".to_string())
}

async fn connect(schema: Option<&str>) -> DatabaseConnection {
    let mut opt = ConnectOptions::new(url());
    if let Some(s) = schema {
        opt.set_schema_search_path(s.to_string());
    }
    Database::connect(opt).await.unwrap()
}

async fn setup(schema: &str) -> (AppState, String) {
    let admin = connect(None).await;
    admin
        .execute_unprepared(&format!("DROP SCHEMA IF EXISTS {schema} CASCADE"))
        .await
        .unwrap();
    admin
        .execute_unprepared(&format!("CREATE SCHEMA {schema}"))
        .await
        .unwrap();

    let conn = connect(Some(schema)).await;
    cicada_migrations::Migrator.up(&conn, None).await.unwrap();
    cicada_migrations::seed::seed(&conn).await.unwrap();
    cicada_migrations::init_admin::init_admin(&conn)
        .await
        .unwrap();

    let db = Db::connect(&DbConfig {
        url: Secret::new(url()),
        max_connections: 5,
        min_connections: 0,
        acquire_timeout_secs: 5,
        slow_query_ms: 200,
        schema: schema.to_string(),
    })
    .await
    .unwrap();

    let tokens = TokenService::new(JWT_SECRET, 900);
    let token = tokens.sign_access(1, 1, "sess-admin", "admin").unwrap();
    let sessions = Arc::new(InMemorySessionStore::new());
    sessions
        .create(
            "sess-admin",
            &Session {
                user_id: 1,
                tenant_id: 1,
                username: "admin".to_string(),
                refresh_hash: hash_refresh("unused"),
                ip: "127.0.0.1".to_string(),
                user_agent: "test".to_string(),
                login_time: 0,
                last_seen: 0,
            },
            600,
        )
        .await
        .unwrap();

    let mut registry = JobRegistry::new();
    registry.register_fn("demo:ok", |_ctx: JobContext| async move {
        Ok(Some("done".to_string()))
    });
    let executor = Arc::new(JobExecutor::new(
        JobStore::new(db.clone()),
        Arc::new(registry),
        Arc::new(InMemoryDistributedLock::new()),
        "test-inst",
        0,
    ));
    // 人工触发走进程内执行侧：HTTP 面只认 `JobRuntime`，与生产用同一套组装方式。
    let sink: Arc<dyn RunSink> = Arc::new(InlineRunSink::new(executor));
    let producer: Arc<dyn cicada_ports::TriggerSource> = Arc::new(CronTriggerSource::new(
        JobStore::new(db.clone()),
        sink.clone(),
    ));
    let job_runtime = Arc::new(JobRuntime::new(sink, Some(producer)));

    let state = AppState {
        dept_directory: Arc::new(LocalDeptDirectory::new(db.clone())),
        config: Config::default(),
        db: db.clone(),
        tokens,
        sessions,
        lockout: Arc::new(cicada_auth::InMemoryLockoutStore::new()),
        captcha: Arc::new(cicada_auth::CaptchaService::new(
            Arc::new(cicada_auth::InMemoryCaptchaStore::new()),
            cicada_auth::CaptchaParams {
                required_after: u32::MAX,
                ..Default::default()
            },
        )),
        idempotency: Arc::new(cicada_web::idempotency::IdempotencyService::new(Arc::new(
            cicada_web::idempotency::InMemoryIdempotencyStore::new(),
        ))),
        storage: Arc::new(cicada_storage::MemoryStorage::new()),
        grants: Arc::new(cicada_storage::InMemoryGrantStore::new()),
        sys_cache: cicada_ports::KvCache::memory(),
        job_runtime,
        notify: Arc::new(cicada_notify::NotifyService::new(
            db.clone(),
            cicada_notify::shared(cicada_notify::RecordingSender::new()),
        )),
        api_keys: Arc::new(cicada_system::ApiKeyService::new(db.clone())),
        quota: Arc::new(cicada_system::TenantQuota::new(db.clone())),
        rate_limit: Arc::new(cicada_web::ratelimit::RateLimit::disabled()),
        oauth: Arc::new(cicada_auth::OAuthService::unconfigured()),
        cache_inspector: Arc::new(cicada_ports::InMemoryCacheInspector::new()),
        authz: Arc::new(AuthzCache::new(Arc::new(InMemoryAuthzStore::new()), 60)),
        audit: Arc::new(InMemoryAuditSink::new()),
        identity_verifier: Arc::new(cicada_auth::StaticCodeVerifier::new("123456")),
    };
    (state, token)
}

fn bearer(method: &str, uri: &str, token: &str) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap()
}

fn json_req(method: &str, uri: &str, token: &str, body: serde_json::Value) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header("authorization", format!("Bearer {token}"))
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

fn sample_job(name: &str, cron: &str) -> serde_json::Value {
    serde_json::json!({
        "job_name": name,
        "job_group": "DEFAULT",
        "invoke_target": "demo:ok",
        "cron_expression": cron,
        "concurrency": 1,
        "timeout_secs": 5,
        "max_retry": 0,
        "status": 1,
        "remark": "test"
    })
}

async fn json_body(resp: axum::response::Response) -> serde_json::Value {
    let body = axum::body::to_bytes(resp.into_body(), 64 * 1024 * 1024)
        .await
        .unwrap();
    serde_json::from_slice(&body).unwrap()
}

#[tokio::test]
async fn job_crud_status_run_logs_export() {
    let (state, token) = setup("t_jobs_crud").await;
    let app = cicada_web::router(state);

    // 创建。
    let resp = app
        .clone()
        .oneshot(json_req(
            "POST",
            "/api/v1/monitor/jobs",
            &token,
            sample_job("清理日志", "0/10 * * * * *"),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let created = json_body(resp).await;
    let job_id = created["job_id"].as_i64().unwrap();
    assert_eq!(created["concurrency"].as_i64().unwrap(), 1);

    // 列表。
    let resp = app
        .clone()
        .oneshot(bearer("GET", "/api/v1/monitor/jobs", &token))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let page = json_body(resp).await;
    // 2 = 本用例建的那条 + seed 落的框架维护任务。
    assert_eq!(page["total"].as_u64().unwrap(), 2);

    // 详情。
    let resp = app
        .clone()
        .oneshot(bearer(
            "GET",
            &format!("/api/v1/monitor/jobs/{job_id}"),
            &token,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(json_body(resp).await["job_name"], "清理日志");

    // 状态切换为正常。
    let resp = app
        .clone()
        .oneshot(json_req(
            "PUT",
            &format!("/api/v1/monitor/jobs/{job_id}/status"),
            &token,
            serde_json::json!({ "status": 0 }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(json_body(resp).await["status"].as_i64().unwrap(), 0);

    // 人工触发：同步后端应立即执行成功。
    let resp = app
        .clone()
        .oneshot(bearer(
            "PUT",
            &format!("/api/v1/monitor/jobs/{job_id}/run"),
            &token,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let run = json_body(resp).await;
    assert_eq!(run["outcome"], "executed");
    assert_eq!(run["success"], true);
    assert_eq!(run["message"], "done");

    // 执行日志：一条成功记录，带 instance_id / trace_id。
    let resp = app
        .clone()
        .oneshot(bearer(
            "GET",
            &format!("/api/v1/monitor/jobs/{job_id}/logs"),
            &token,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let logs = json_body(resp).await;
    assert_eq!(logs["total"].as_u64().unwrap(), 1);
    let entry = &logs["items"][0];
    assert_eq!(entry["status"].as_i64().unwrap(), 0);
    assert_eq!(entry["trigger_kind"].as_i64().unwrap(), 1);
    assert_eq!(entry["instance_id"], "test-inst");
    assert!(entry["trace_id"].as_str().unwrap().len() == 36);

    // 导出。
    let resp = app
        .clone()
        .oneshot(bearer("GET", "/api/v1/monitor/jobs/export", &token))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(
        resp.headers()
            .get(header::CONTENT_TYPE)
            .unwrap()
            .to_str()
            .unwrap()
            .contains("spreadsheet")
    );
    let bytes = axum::body::to_bytes(resp.into_body(), 16 * 1024 * 1024)
        .await
        .unwrap();
    assert_eq!(&bytes[..2], b"PK", "xlsx 应以 zip magic 开头");

    // 删除（软删）。
    let resp = app
        .clone()
        .oneshot(bearer(
            "DELETE",
            &format!("/api/v1/monitor/jobs/{job_id}"),
            &token,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    let resp = app
        .clone()
        .oneshot(bearer(
            "GET",
            &format!("/api/v1/monitor/jobs/{job_id}"),
            &token,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn invalid_cron_and_duplicate_are_rejected() {
    let (state, token) = setup("t_jobs_validate").await;
    let app = cicada_web::router(state);

    let resp = app
        .clone()
        .oneshot(json_req(
            "POST",
            "/api/v1/monitor/jobs",
            &token,
            sample_job("坏表达式", "not a cron"),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);

    let resp = app
        .clone()
        .oneshot(json_req(
            "POST",
            "/api/v1/monitor/jobs",
            &token,
            sample_job("重复任务", "0/10 * * * * *"),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    let resp = app
        .clone()
        .oneshot(json_req(
            "POST",
            "/api/v1/monitor/jobs",
            &token,
            sample_job("重复任务", "0/20 * * * * *"),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CONFLICT);
}

/// 框架自带的维护任务必须**真的可被调度**：`register_fn` 只挂 handler，跑不跑看 `sys_job` 的行。
/// 这里从调度器的装载入口（`list_schedulable`）反查 seed 落的那一行，并让它过同一套 cron 校验。
#[tokio::test]
async fn seeded_framework_job_is_schedulable() {
    let (state, _token) = setup("t_job_seed").await;
    let store = JobStore::new(state.db.clone());

    let schedulable = store.list_schedulable().await.unwrap();
    let seeded = schedulable
        .iter()
        .find(|d| d.invoke_target == cicada_migrations::seed::JOB_TARGET_LOGS_RETENTION)
        .unwrap_or_else(|| {
            panic!(
                "seed 的框架任务没进可调度集合（status 或 cron 有问题？）：{:?}",
                schedulable
                    .iter()
                    .map(|d| (&d.job_name, &d.invoke_target, &d.cron_expression))
                    .collect::<Vec<_>>()
            )
        });

    assert_eq!(seeded.job_group, "SYSTEM", "框架任务要落在 SYSTEM 组");
    assert_eq!(
        seeded.concurrency,
        cicada_kernel::ConcurrencyPolicy::Forbid,
        "归档跑两遍会互相踩"
    );
    validate_cron(&seeded.cron_expression)
        .unwrap_or_else(|e| panic!("seed 的 cron 过不了校验：{e}"));
}
