//! 业务模块接入（`router_with`， 规则 8）。
//!
//! 使用者工程（`cicada new` 产出）与框架自己的 `apps/server` 只差一个模块列表：
//! 拿同一个 `AppState`，`router_with` 把业务模块并入内置模块一起装配、一起对账。
//! 本文件用一个最小业务模块（一张表、一个权限点、一条路由）验证这条缝：
//!
//! - 业务路由可达（带凭证 200 / 无凭证 401）；
//! - 业务权限点参与装配期双向对账（声明了却没人用 → 启动失败）。

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
use cicada_rbac::{AuthzCache, InMemoryAuthzStore};
use cicada_system::LocalDeptDirectory;
use cicada_web::state::AppState;
use cicada_web::{Manifest, Module, Permission, Routes, TableName, router_with};

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

    let db = connect(Some(schema)).await;
    cicada_migrations::Migrator.up(&db, None).await.unwrap();
    cicada_migrations::seed::seed(&db).await.unwrap();
    cicada_migrations::init_admin::init_admin(&db)
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
    };
    (state, token)
}

/// 最小业务模块：一张表、一个权限点、一条路由。
mod ping {
    use super::*;
    use axum::routing::get;

    #[cicada_macros::require_perm("business:ping:list")]
    async fn list() -> &'static str {
        "pong"
    }

    fn routes(_state: AppState) -> Routes {
        Routes::new().route("/api/v1/business/ping", get(list), list__perms())
    }

    pub struct PingModule;

    impl Module for PingModule {
        fn manifest(self: Box<Self>) -> Manifest {
            Manifest::new("business:ping")
                .routes(routes)
                .owns([TableName("biz_ping")])
                .perms([Permission::new("business:ping:list")])
        }
    }
}

/// 声明了权限点却没有路由使用：装配期必须失败（`OrphanPermission`）。
mod orphan {
    use super::*;

    pub struct OrphanModule;

    impl Module for OrphanModule {
        fn manifest(self: Box<Self>) -> Manifest {
            Manifest::new("business:orphan").perms([Permission::new("business:orphan:list")])
        }
    }
}

async fn get(app: axum::Router, uri: &str, token: &str) -> StatusCode {
    let mut b = Request::builder().method("GET").uri(uri);
    if !token.is_empty() {
        b = b.header("authorization", format!("Bearer {token}"));
    }
    app.oneshot(b.body(Body::empty()).unwrap())
        .await
        .unwrap()
        .status()
}

#[tokio::test]
#[ignore = "需要真实 PostgreSQL"]
async fn business_module_is_mounted_and_guarded() {
    let (state, token) = setup("web_business_module").await;
    let app = router_with(state.clone(), vec![Box::new(ping::PingModule)]);

    // 无凭证：认证中间件先挡住。
    assert_eq!(
        get(app.clone(), "/api/v1/business/ping", "").await,
        StatusCode::UNAUTHORIZED
    );
    // 有凭证：业务路由可达（admin 是超级管理员，权限点对账已在启动期通过）。
    assert_eq!(
        get(app.clone(), "/api/v1/business/ping", &token).await,
        StatusCode::OK
    );
    // 内置路由不受影响。
    assert_eq!(get(app, "/healthz", "").await, StatusCode::OK);
}

#[tokio::test]
#[ignore = "需要真实 PostgreSQL"]
async fn orphan_permission_refuses_to_start() {
    let (state, _token) = setup("web_business_orphan").await;
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        router_with(state, vec![Box::new(orphan::OrphanModule)])
    }));
    assert!(result.is_err(), "声明了权限点却没有路由使用，装配必须失败");
}
