//! Idempotency-Key 防重复提交测试：同键重放首个结果、
//! 执行中 409、非法键与非写方法不受影响。

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use sea_orm::{ConnectOptions, ConnectionTrait, Database, DatabaseConnection};
use sea_orm_migration::MigratorTraitSelf;
use tower::ServiceExt;

use cicada_audit::InMemoryAuditSink;
use cicada_auth::{InMemorySessionStore, Session, SessionStore, TokenService, hash_refresh};
use cicada_infra::Db;
use cicada_infra::config::{Config, DbConfig, Secret};
use cicada_ports::InMemoryCacheInspector;
use cicada_rbac::{AuthzCache, InMemoryAuthzStore};
use cicada_system::LocalDeptDirectory;
use cicada_web::idempotency::{IdempotencyService, IdempotencyStore, InMemoryIdempotencyStore};
use cicada_web::state::AppState;

const ADMIN_PW_KEY: &str = "idem.test.key1";

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

async fn setup(schema: &str) -> (AppState, Arc<InMemoryIdempotencyStore>) {
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

    let tokens = TokenService::new("test-secret", 900);
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

    let idem_store = Arc::new(InMemoryIdempotencyStore::new());
    let state = AppState {
        dept_directory: Arc::new(LocalDeptDirectory::new(db.clone())),
        config: Config::default(),
        db: db.clone(),
        tokens,
        sessions,
        authz: Arc::new(AuthzCache::new(Arc::new(InMemoryAuthzStore::new()), 60)),
        lockout: Arc::new(cicada_auth::InMemoryLockoutStore::new()),
        captcha: Arc::new(cicada_auth::CaptchaService::new(
            Arc::new(cicada_auth::InMemoryCaptchaStore::new()),
            cicada_auth::CaptchaParams {
                required_after: u32::MAX,
                ..Default::default()
            },
        )),
        idempotency: Arc::new(IdempotencyService::new(idem_store.clone())),
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
    (state, idem_store)
}

async fn post_config(
    state: &AppState,
    token: &str,
    config_key: &str,
    idem_key: Option<&str>,
) -> (StatusCode, serde_json::Value) {
    let mut builder = Request::builder()
        .method("POST")
        .uri("/api/v1/system/configs")
        .header("content-type", "application/json")
        .header("authorization", format!("Bearer {token}"));
    if let Some(k) = idem_key {
        builder = builder.header("idempotency-key", k);
    }
    let resp = cicada_web::router(state.clone())
        .oneshot(
            builder
                .body(Body::from(
                    serde_json::json!({
                        "config_name": "幂等测试", "config_key": config_key, "config_value": "1"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
        .await
        .unwrap();
    let json = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
    (status, json)
}

#[tokio::test]
async fn same_key_replays_first_result() {
    let (state, _store) = setup("web_idem_replay").await;
    let token = state
        .tokens
        .sign_access(1, 1, "sess-admin", "admin")
        .unwrap();

    let (s1, b1) = post_config(&state, &token, ADMIN_PW_KEY, Some("idem-key-0001")).await;
    assert_eq!(s1, StatusCode::CREATED, "body = {b1}");
    let id1 = b1["config_id"].as_i64().unwrap();

    // 同键重放：状态与体完全一致，不再落库。
    let (s2, b2) = post_config(&state, &token, ADMIN_PW_KEY, Some("idem-key-0001")).await;
    assert_eq!(s2, StatusCode::CREATED);
    assert_eq!(b2, b1, "重放应返回首个请求的结果");
    assert_eq!(b2["config_id"].as_i64().unwrap(), id1);

    // 不同键不受影响：另一把键照常执行（新的 config_key 才不撞唯一约束）。
    let (s3, b3) = post_config(&state, &token, "idem.test.key2", Some("idem-key-0002")).await;
    assert_eq!(s3, StatusCode::CREATED, "body = {b3}");
    assert_ne!(b3["config_id"].as_i64().unwrap(), id1);
}

#[tokio::test]
async fn in_flight_same_key_conflicts() {
    let (state, store) = setup("web_idem_inflight").await;
    let token = state
        .tokens
        .sign_access(1, 1, "sess-admin", "admin")
        .unwrap();

    // 预占执行位（模拟同键请求正在处理；键作用域为超管用户 u1）。
    assert!(
        store
            .claim("cicada:idem:u1:idem-key-busy:proc", 60)
            .await
            .unwrap()
    );
    let (status, body) = post_config(&state, &token, ADMIN_PW_KEY, Some("idem-key-busy")).await;
    assert_eq!(status, StatusCode::CONFLICT, "body = {body}");
    assert!(body["detail"].as_str().unwrap().contains("正在处理中"));
}

#[tokio::test]
async fn missing_or_invalid_key_is_passthrough() {
    let (state, _store) = setup("web_idem_passthrough").await;
    let token = state
        .tokens
        .sign_access(1, 1, "sess-admin", "admin")
        .unwrap();

    // 无头：两次请求各自执行（不同 config_key 避开唯一约束）。
    let (s1, b1) = post_config(&state, &token, "idem.test.p1", None).await;
    let (s2, b2) = post_config(&state, &token, "idem.test.p2", None).await;
    assert_eq!((s1, s2), (StatusCode::CREATED, StatusCode::CREATED));
    assert_ne!(b1["config_id"], b2["config_id"]);

    // 非法键（含空格/太短）按未带头处理。
    let (s3, _) = post_config(&state, &token, "idem.test.p3", Some("bad key")).await;
    assert_eq!(s3, StatusCode::CREATED);
}
