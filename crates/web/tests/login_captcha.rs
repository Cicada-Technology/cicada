//! 登录图形验证码测试：失败达到阈值后强制验证码，
//! 验证码一次性消费，`GET /auth/captcha` 公开可取。

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use sea_orm::{ConnectOptions, ConnectionTrait, Database, DatabaseConnection};
use sea_orm_migration::MigratorTraitSelf;
use tower::ServiceExt;

use cicada_audit::InMemoryAuditSink;
use cicada_auth::{
    CaptchaParams, CaptchaService, CaptchaStore, InMemoryCaptchaStore, InMemorySessionStore,
    TokenService,
};
use cicada_infra::Db;
use cicada_infra::config::{Config, DbConfig, Secret};
use cicada_ports::InMemoryCacheInspector;
use cicada_rbac::{AuthzCache, InMemoryAuthzStore};
use cicada_system::LocalDeptDirectory;
use cicada_web::state::AppState;

const ADMIN_PW: &str = "Cicada@2026";

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

async fn setup(schema: &str, required_after: u32) -> (AppState, Arc<InMemoryCaptchaStore>) {
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

    let captcha_store = Arc::new(InMemoryCaptchaStore::new());
    let state = AppState {
        dept_directory: Arc::new(LocalDeptDirectory::new(db.clone())),
        config: Config::default(),
        db: db.clone(),
        tokens: TokenService::new("test-secret", 900),
        sessions: Arc::new(InMemorySessionStore::new()),
        authz: Arc::new(AuthzCache::new(Arc::new(InMemoryAuthzStore::new()), 60)),
        lockout: Arc::new(cicada_auth::InMemoryLockoutStore::new()),
        captcha: Arc::new(CaptchaService::new(
            captcha_store.clone(),
            CaptchaParams {
                required_after,
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
    };
    (state, captcha_store)
}

async fn post(state: &AppState, path: &str, body: serde_json::Value) -> (StatusCode, String) {
    let resp = cicada_web::router(state.clone())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(path)
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 64 * 1024)
        .await
        .unwrap();
    (status, String::from_utf8_lossy(&bytes).to_string())
}

#[tokio::test]
async fn captcha_endpoint_is_public() {
    let (state, _store) = setup("web_captcha_public", 0).await;
    let resp = cicada_web::router(state)
        .oneshot(Request::get("/auth/captcha").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
        .await
        .unwrap();
    let vo: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(vo["enabled"], serde_json::json!(true));
    assert!(!vo["captcha_id"].as_str().unwrap().is_empty());
    assert!(!vo["img"].as_str().unwrap().is_empty());
}

#[tokio::test]
async fn login_requires_captcha_when_always_on() {
    let (state, _store) = setup("web_captcha_required", 0).await;
    // 必带验证码：不带 → 422 验证码错误（不进入凭据校验）。
    let (status, body) = post(
        &state,
        "/auth/login",
        serde_json::json!({ "username": "admin", "password": ADMIN_PW }),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(body.contains("图形验证码"), "body = {body}");
}

#[tokio::test]
async fn captcha_answer_is_one_shot_and_unblocks_login() {
    let (state, store) = setup("web_captcha_oneshot", 0).await;
    store.issue("cap-fixed", "ABCD", 120).await.unwrap();

    // 大小写不敏感通过，但密码错误 → 走到凭据校验（422 账号或密码错误）。
    let (status, body) = post(
        &state,
        "/auth/login",
        serde_json::json!({
            "username": "admin", "password": "wrong-pass",
            "captcha_id": "cap-fixed", "captcha_code": "abcd",
        }),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(!body.contains("图形验证码"), "验证码应已通过: {body}");

    // 同一个 id 已被消费：再提交回到「图形验证码错误或已过期」。
    let (status, body) = post(
        &state,
        "/auth/login",
        serde_json::json!({
            "username": "admin", "password": ADMIN_PW,
            "captcha_id": "cap-fixed", "captcha_code": "abcd",
        }),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(body.contains("图形验证码"), "body = {body}");

    // 重新发放正确验证码 + 正确密码 → 200。
    store.issue("cap-2", "wxyz", 120).await.unwrap();
    let (status, body) = post(
        &state,
        "/auth/login",
        serde_json::json!({
            "username": "admin", "password": ADMIN_PW,
            "captcha_id": "cap-2", "captcha_code": "WXYZ",
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body = {body}");
    let vo: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert!(!vo["access_token"].as_str().unwrap().is_empty());
}

#[tokio::test]
async fn captcha_enforced_after_consecutive_failures() {
    // required_after = 1：第一次失败后强制。
    let (state, store) = setup("web_captcha_threshold", 1).await;

    // 第 1 次：不要求验证码，错误密码正常报凭据错误并计数。
    let (status, body) = post(
        &state,
        "/auth/login",
        serde_json::json!({ "username": "admin", "password": "bad" }),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(!body.contains("图形验证码"), "body = {body}");

    // 第 2 次：失败计数达到阈值，不带验证码被拦。
    let (status, body) = post(
        &state,
        "/auth/login",
        serde_json::json!({ "username": "admin", "password": ADMIN_PW }),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(body.contains("图形验证码"), "body = {body}");

    // 带正确验证码 → 放行。
    store.issue("cap-3", "9k2m", 120).await.unwrap();
    let (status, body) = post(
        &state,
        "/auth/login",
        serde_json::json!({
            "username": "admin", "password": ADMIN_PW,
            "captcha_id": "cap-3", "captcha_code": "9K2M",
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body = {body}");
}
