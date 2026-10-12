//! 运行时权限守卫测试：`#[require_perm]` 声明的路由在缺少权限时返回 403，
//! 未认证返回 401，具备权限（或超级管理员）放行。

use std::net::{IpAddr, Ipv4Addr};
use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::middleware::{self, Next};
use axum::response::Response;
use axum::routing::post;
use tower::ServiceExt;

use cicada_audit::InMemoryAuditSink;
use cicada_auth::{InMemorySessionStore, TokenService};
use cicada_infra::Db;
use cicada_infra::config::{Config, DbConfig, Secret};
use cicada_kernel::{
    DataScope, Permission, Principal, RequestContext, RequestId, SessionId, TenantId, TraceId,
    UserId,
};
use cicada_rbac::{AuthzCache, InMemoryAuthzStore};
use cicada_system::LocalDeptDirectory;
use cicada_web::Routes;
use cicada_web::state::AppState;

#[cicada_macros::require_perm("system:user:add")]
async fn guarded() -> &'static str {
    "ok"
}

async fn state() -> AppState {
    let url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://cicada:2025%40hanmole@localhost:5432/cicada".to_string());
    let db = Db::connect(&DbConfig {
        url: Secret::new(url),
        max_connections: 2,
        min_connections: 0,
        acquire_timeout_secs: 5,
        slow_query_ms: 200,
        schema: "public".to_string(),
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

fn ctx(perms: &[&str], super_admin: bool) -> RequestContext {
    let mut ctx = ctx_anon();
    ctx.principal = Some(Principal {
        user_id: UserId::new(1),
        tenant_id: TenantId::new(1),
        session_id: SessionId("s".to_string()),
        user_name: "admin".to_string(),
    });
    ctx.permissions = perms.iter().map(|p| Permission::new(*p)).collect();
    ctx.is_super_admin = super_admin;
    ctx
}

fn ctx_anon() -> RequestContext {
    RequestContext {
        request_id: RequestId::new("req"),
        principal: None,
        tenant_id: TenantId::new(1),
        data_scope: DataScope::none(),
        permissions: Vec::new(),
        roles: Vec::new(),
        is_super_admin: false,
        ip: IpAddr::V4(Ipv4Addr::LOCALHOST),
        user_agent: None,
        trace_id: TraceId::new("trace"),
        locale: "zh-CN".to_string(),
    }
}

async fn inject(State(ctx): State<RequestContext>, mut req: Request, next: Next) -> Response {
    req.extensions_mut().insert(ctx);
    next.run(req).await
}

fn guarded_router() -> Router<AppState> {
    let (router, _) = Routes::new()
        .route("/guarded", post(guarded), guarded__perms())
        .into_parts();
    router
}

async fn call(app: Router) -> StatusCode {
    app.oneshot(
        Request::builder()
            .method("POST")
            .uri("/guarded")
            .body(Body::empty())
            .unwrap(),
    )
    .await
    .unwrap()
    .status()
}

#[tokio::test]
#[ignore = "需要真实 PostgreSQL"]
async fn denies_without_permission() {
    let app = guarded_router()
        .layer(middleware::from_fn_with_state(ctx(&[], false), inject))
        .with_state(state().await);
    assert_eq!(call(app).await, StatusCode::FORBIDDEN);
}

#[tokio::test]
#[ignore = "需要真实 PostgreSQL"]
async fn allows_with_permission() {
    let app = guarded_router()
        .layer(middleware::from_fn_with_state(
            ctx(&["system:user:add"], false),
            inject,
        ))
        .with_state(state().await);
    assert_eq!(call(app).await, StatusCode::OK);
}

#[tokio::test]
#[ignore = "需要真实 PostgreSQL"]
async fn allows_super_admin() {
    let app = guarded_router()
        .layer(middleware::from_fn_with_state(ctx(&[], true), inject))
        .with_state(state().await);
    assert_eq!(call(app).await, StatusCode::OK);
}

#[tokio::test]
#[ignore = "需要真实 PostgreSQL"]
async fn unauthenticated_is_401() {
    let app = guarded_router().with_state(state().await);
    assert_eq!(call(app).await, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
#[ignore = "需要真实 PostgreSQL"]
async fn anonymous_is_401() {
    let app = guarded_router()
        .layer(middleware::from_fn_with_state(ctx_anon(), inject))
        .with_state(state().await);
    assert_eq!(call(app).await, StatusCode::UNAUTHORIZED);
}
