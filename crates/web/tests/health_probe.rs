//! `/readyz` 的失败形状（统一响应 + 「就绪，查 PG 与 Redis」+ 9002/9003）。
//!
//! 依赖故障在集成测试里最容易造出来的是「会话存储不可用」：包一层 `SessionStore`，
//! 只让 `ping` 失败，其余照常工作。PG 故障分支由 `health.rs` 的单测覆盖（纯映射函数）。

use std::sync::Arc;

use async_trait::async_trait;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use sea_orm::{ConnectOptions, ConnectionTrait, Database, DatabaseConnection};
use sea_orm_migration::MigratorTraitSelf;
use tower::ServiceExt;

use cicada_audit::InMemoryAuditSink;
use cicada_auth::{AuthError, InMemorySessionStore, Session, SessionStore, TokenService};
use cicada_infra::Db;
use cicada_infra::config::{Config, DbConfig, Secret};
use cicada_rbac::{AuthzCache, InMemoryAuthzStore};
use cicada_system::LocalDeptDirectory;
use cicada_web::router;
use cicada_web::state::AppState;

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

/// 会话存储可用、但探活失败的替身（等价于 Redis 掉了而缓存里还有数据）。
struct PingFailsStore(InMemorySessionStore);

#[async_trait]
impl SessionStore for PingFailsStore {
    async fn create(&self, jti: &str, s: &Session, ttl: u64) -> Result<(), AuthError> {
        self.0.create(jti, s, ttl).await
    }
    async fn get(&self, jti: &str) -> Result<Option<Session>, AuthError> {
        self.0.get(jti).await
    }
    async fn touch(&self, jti: &str, ttl: u64) -> Result<(), AuthError> {
        self.0.touch(jti, ttl).await
    }
    async fn revoke(&self, jti: &str) -> Result<(), AuthError> {
        self.0.revoke(jti).await
    }
    async fn revoke_user(&self, user_id: i64) -> Result<(), AuthError> {
        self.0.revoke_user(user_id).await
    }
    async fn list_user(&self, user_id: i64) -> Result<Vec<(String, Session)>, AuthError> {
        self.0.list_user(user_id).await
    }
    async fn list_all(&self) -> Result<Vec<(String, Session)>, AuthError> {
        self.0.list_all().await
    }
    async fn ping(&self) -> Result<(), AuthError> {
        Err(AuthError::Session("probe: redis down".to_string()))
    }
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
    let db = connect(Some(schema)).await;
    cicada_migrations::Migrator.up(&db, None).await.unwrap();

    let db = Db::connect(&DbConfig {
        url: Secret::new(url()),
        max_connections: 3,
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
        tokens: TokenService::new(JWT_SECRET, 900),
        sessions: Arc::new(PingFailsStore(InMemorySessionStore::new())),
        authz: Arc::new(AuthzCache::new(Arc::new(InMemoryAuthzStore::new()), 60)),
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
        cache_inspector: Arc::new(cicada_ports::InMemoryCacheInspector::new()),
    }
}

#[tokio::test]
#[ignore = "需要真实 PostgreSQL"]
async fn readyz_failure_is_problem_json_with_a_biz_code() {
    let app = router(setup("web_readyz_probe").await);
    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/readyz")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    let ctype = resp
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    assert!(
        ctype.contains("application/problem+json"),
        "失败响应必须是 problem+json，实际 {ctype}"
    );

    let body = axum::body::to_bytes(resp.into_body(), 64 * 1024)
        .await
        .unwrap();
    let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(value["code"].as_str(), Some("CICADA_INTERNAL_9003"));
    assert_eq!(value["status"].as_i64(), Some(503));
    assert_eq!(value["title"].as_str(), Some("缓存不可用"));
    assert!(
        value["detail"]
            .as_str()
            .unwrap_or_default()
            .contains("Redis"),
        "要说清是哪个依赖：{value}"
    );
}
