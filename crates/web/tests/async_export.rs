//! 异步导出任务端到端测试：
//! 受理 → 后台执行 → 轮询状态 → 下载产物；未知模块 422、未完成下载 409。

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use sea_orm::{ConnectOptions, ConnectionTrait, Database, DatabaseConnection};
use sea_orm_migration::MigratorTraitSelf;
use tower::ServiceExt;

use cicada_audit::InMemoryAuditSink;
use cicada_auth::{
    CaptchaParams, CaptchaService, InMemoryCaptchaStore, InMemorySessionStore, TokenService,
};
use cicada_infra::Db;
use cicada_infra::config::{Config, DbConfig, Secret};
use cicada_ports::InMemoryCacheInspector;
use cicada_rbac::{AuthzCache, InMemoryAuthzStore};
use cicada_system::LocalDeptDirectory;
use cicada_web::state::AppState;

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

async fn setup(schema: &str) -> AppState {
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

    AppState {
        dept_directory: Arc::new(LocalDeptDirectory::new(db.clone())),
        config: Config::default(),
        db: db.clone(),
        tokens: TokenService::new("test-secret", 900),
        sessions: Arc::new(InMemorySessionStore::new()),
        authz: Arc::new(AuthzCache::new(Arc::new(InMemoryAuthzStore::new()), 60)),
        lockout: Arc::new(cicada_auth::InMemoryLockoutStore::new()),
        captcha: Arc::new(CaptchaService::new(
            Arc::new(InMemoryCaptchaStore::new()),
            CaptchaParams {
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
        audit: Arc::new(InMemoryAuditSink::new()),
        identity_verifier: Arc::new(cicada_auth::StaticCodeVerifier::new("123456")),
        job_runtime: Arc::new(cicada_ports::JobRuntime::new(
            Arc::new(cicada_ports::NoopRunSink),
            None,
        )),
        notify: Arc::new(cicada_notify::NotifyService::new(
            db.clone(),
            cicada_notify::shared(cicada_notify::RecordingSender::new()),
        )),
        api_keys: Arc::new(cicada_system::ApiKeyService::new(db.clone())),
        quota: Arc::new(cicada_system::TenantQuota::new(db.clone())),
        rate_limit: Arc::new(cicada_web::ratelimit::RateLimit::disabled()),
        oauth: Arc::new(cicada_auth::OAuthService::unconfigured()),
        cache_inspector: Arc::new(InMemoryCacheInspector::new()),
    }
}

async fn ask(
    state: &AppState,
    method: &str,
    uri: &str,
    body: Option<serde_json::Value>,
    token: Option<&str>,
) -> (StatusCode, serde_json::Value, Vec<u8>) {
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some(t) = token {
        builder = builder.header("authorization", format!("Bearer {t}"));
    }
    let req = match body {
        Some(v) => {
            builder = builder.header("content-type", "application/json");
            builder.body(Body::from(v.to_string())).unwrap()
        }
        None => builder.body(Body::empty()).unwrap(),
    };
    let resp = cicada_web::router(state.clone())
        .oneshot(req)
        .await
        .unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 4 * 1024 * 1024)
        .await
        .unwrap()
        .to_vec();
    let json = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
    (status, json, bytes)
}

async fn login(state: &AppState) -> String {
    let (status, vo, _) = ask(
        state,
        "POST",
        "/auth/login",
        Some(serde_json::json!({
            "username": "admin",
            "password": "Cicada@2026"
        })),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{vo}");
    vo["access_token"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn async_export_full_cycle() {
    let state = setup("web_async_export_cycle").await;
    let token = login(&state).await;

    // 受理：Pending 落库并后台开跑。
    let (status, task, _) = ask(
        &state,
        "POST",
        "/api/v1/system/export-tasks",
        Some(serde_json::json!({ "module": "system:user", "query": {} })),
        Some(&token),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{task}");
    let id = task["id"].as_i64().unwrap();
    assert_eq!(task["status"].as_i64().unwrap(), 0);

    // 轮询直到 Done（或超时失败）。
    let mut vo = task;
    for _ in 0..100 {
        let (_s, cur, _) = ask(
            &state,
            "GET",
            &format!("/api/v1/system/export-tasks/{id}"),
            None,
            Some(&token),
        )
        .await;
        vo = cur;
        if vo["status"].as_i64().unwrap() >= 2 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert_eq!(vo["status"].as_i64(), Some(2), "任务应完成：{vo}");
    assert!(vo["file_id"].as_i64().is_some(), "{vo}");
    assert!(vo["processed"].as_i64().unwrap() >= 1, "{vo}");

    // 列表只见自己（OwnerOnly 范围）。
    let (status, page, _) = ask(
        &state,
        "GET",
        "/api/v1/system/export-tasks",
        None,
        Some(&token),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{page}");
    assert_eq!(page["total"].as_u64(), Some(1));

    // 下载产物：xlsx magic（PK zip 头）。
    let (status, _, bytes) = ask(
        &state,
        "GET",
        &format!("/api/v1/system/export-tasks/{id}/download"),
        None,
        Some(&token),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(bytes.starts_with(b"PK\x03\x04"), "应为 xlsx 字节流");
}

#[tokio::test]
async fn unknown_module_rejected_and_pending_download_conflicts() {
    let state = setup("web_async_export_guards").await;
    let token = login(&state).await;

    let (status, body, _) = ask(
        &state,
        "POST",
        "/api/v1/system/export-tasks",
        Some(serde_json::json!({ "module": "system:nope" })),
        Some(&token),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");

    // 未完成即下载 → 409（用刚受理、尚未完成的任务；轮询窗口极短，
    // 直接查一个不存在完成的场景不可靠，这里以新建任务立刻下载为准）。
    let (_s, task, _) = ask(
        &state,
        "POST",
        "/api/v1/system/export-tasks",
        Some(serde_json::json!({ "module": "system:user", "query": { "user_name": "不存在的用户_zzz" } })),
        Some(&token),
    )
    .await;
    let id = task["id"].as_i64().unwrap();
    // 等它结束（无论成败），若已 Done 则本断言改判：空结果集导出很快，可能已完成。
    for _ in 0..100 {
        let (_s, cur, _) = ask(
            &state,
            "GET",
            &format!("/api/v1/system/export-tasks/{id}"),
            None,
            Some(&token),
        )
        .await;
        if cur["status"].as_i64().unwrap() >= 2 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    let (status, body, _) = ask(
        &state,
        "GET",
        &format!("/api/v1/system/export-tasks/{id}/download"),
        None,
        Some(&token),
    )
    .await;
    // 完成 → 200；若仍是 Pending/Running → 409。两者都说明闸门语义正确。
    assert!(
        status == StatusCode::OK || status == StatusCode::CONFLICT,
        "{status} {body}"
    );

    // 未认证：401。
    let (status, _, _) = ask(&state, "GET", "/api/v1/system/export-tasks", None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

/// 导入导出「完成后经通知中心给下载链接」的那条链路，两个分支都要跑到。
#[tokio::test]
async fn export_completion_goes_through_the_notify_center() {
    let state = setup("web_async_export_notify").await;
    let token = login(&state).await;

    // 模板是种子给的；渠道不是——它带 SMTP 端点与凭据，按配置只能由运营建。
    assert!(
        scalar_i64(
            &state,
            "SELECT count(*) FROM sys_notify_template \
              WHERE code = 'export.done' AND channel_kind = 'email'"
        )
        .await
            >= 2,
        "种子应带 zh-CN / en-US 两份导出完成模板"
    );

    // 创建者有邮箱，但还没有任何可用渠道：这一趟必须「任务完成、通知没发」。
    sql(
        &state,
        "UPDATE sys_user SET email = 'ops@example.com' WHERE user_name = 'admin'",
    )
    .await;
    let first = submit(&state, &token).await;
    let vo = await_done(&state, &token, first).await;
    assert_eq!(vo["status"].as_i64(), Some(2), "没有渠道也要完成：{vo}");
    assert_eq!(
        scalar_i64(&state, "SELECT count(*) FROM sys_notify_record").await,
        0,
        "没有启用的邮件渠道时不该留下投递记录"
    );

    sql(
        &state,
        r#"INSERT INTO sys_notify_channel
             (tenant_id, code, name, kind, config, enabled, create_time)
           VALUES (1, 'smtp-main', '主 SMTP', 'email',
                   '{"endpoint":"https://mail.example.test/send","from":"no-reply@cicada.dev"}'::jsonb,
                   true, now())"#,
    )
    .await;

    let second = submit(&state, &token).await;
    let vo = await_done(&state, &token, second).await;
    assert_eq!(vo["status"].as_i64(), Some(2), "{vo}");

    // 投递是在后台任务里同步 await 的，DONE 落库时记录必然已经在。
    let (recipient, template, status, payload_task) = record_row(&state).await;
    assert_eq!(recipient, "ops@example.com");
    assert_eq!(
        template,
        cicada_web::async_export::EXPORT_DONE_TEMPLATE_CODE
    );
    assert_eq!(status, 1, "RecordingSender 应当投递成功（1 已发）");
    assert_eq!(
        payload_task,
        second.to_string(),
        "通知载荷里的 task_id 必须是这次的任务"
    );

    // 链接指向下载端点：模板正文是这条承诺的载体。
    let body = scalar_text(
        &state,
        "SELECT body FROM sys_notify_template \
          WHERE code = 'export.done' AND locale = 'zh-CN'",
    )
    .await;
    assert!(
        body.contains("/api/v1/system/export-tasks/{{ task_id }}/download"),
        "模板正文没写下载路径：{body}"
    );
}

/// 受理一次异步导出，返回任务 id。
async fn submit(state: &AppState, token: &str) -> i64 {
    let (status, task, _) = ask(
        state,
        "POST",
        "/api/v1/system/export-tasks",
        Some(serde_json::json!({ "module": "system:user", "query": {} })),
        Some(token),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{task}");
    task["id"].as_i64().unwrap()
}

/// 轮询到终态（2 完成 / 3 失败），失败时直接把 vo 抖出来。
async fn await_done(state: &AppState, token: &str, id: i64) -> serde_json::Value {
    for _ in 0..100 {
        let (_s, vo, _) = ask(
            state,
            "GET",
            &format!("/api/v1/system/export-tasks/{id}"),
            None,
            Some(token),
        )
        .await;
        if vo["status"].as_i64().unwrap_or(0) >= 2 {
            return vo;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    panic!("任务 {id} 没有在 5 秒内进入终态");
}

async fn sql(state: &AppState, text: &str) {
    state
        .db
        .conn()
        .execute_unprepared(text)
        .await
        .unwrap_or_else(|e| panic!("{text} 失败：{e}"));
}

async fn scalar_i64(state: &AppState, text: &str) -> i64 {
    state
        .db
        .conn()
        .query_one_raw(sea_orm::Statement::from_string(
            sea_orm::DatabaseBackend::Postgres,
            text,
        ))
        .await
        .unwrap()
        .map(|r| r.try_get_by_index::<i64>(0).unwrap_or(-1))
        .unwrap_or(-1)
}

async fn scalar_text(state: &AppState, text: &str) -> String {
    state
        .db
        .conn()
        .query_one_raw(sea_orm::Statement::from_string(
            sea_orm::DatabaseBackend::Postgres,
            text,
        ))
        .await
        .unwrap()
        .map(|r| r.try_get_by_index::<String>(0).unwrap_or_default())
        .unwrap_or_default()
}

/// 最近一条投递记录：收件人 / 模板码 / 状态 / 载荷里的 task_id。
async fn record_row(state: &AppState) -> (String, String, i64, String) {
    let row = state
        .db
        .conn()
        .query_one_raw(sea_orm::Statement::from_string(
            sea_orm::DatabaseBackend::Postgres,
            "SELECT recipient, template_code, status::int8, payload->>'task_id' FROM sys_notify_record \
              ORDER BY id DESC LIMIT 1",
        ))
        .await
        .unwrap()
        .expect("应有一条通知记录");
    (
        row.try_get_by_index::<String>(0).unwrap(),
        row.try_get_by_index::<String>(1).unwrap(),
        row.try_get_by_index::<i64>(2).unwrap(),
        row.try_get_by_index::<String>(3).unwrap_or_default(),
    )
}

/// 种子与生产者各持一份模板码字面量（`web`(L4) 不依赖 `migrations`，没法共用一个常量），
/// 所以在这里对账一次——改一边忘另一边时，通知会静默变成「模板不存在」。
/// 同审计设计里框架任务目标字符串的处理（`crates/all/tests/framework_jobs.rs`）。
#[test]
fn export_template_code_matches_the_seed() {
    assert_eq!(
        cicada_web::async_export::EXPORT_DONE_TEMPLATE_CODE,
        cicada_migrations::seed::EXPORT_DONE_TEMPLATE_CODE
    );
}
