//! 第三方登录集成测试（真实 PostgreSQL）：授权地址 → 回调 → 未绑定发票据 → 票据绑定 →
//! 再回调直接发令牌。用 `RecordingOAuthClient` 顶替真实 IdP，因此不需要外部服务。
//!
//! 默认 ignore（`-- --ignored` 运行）。

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use sea_orm::{ConnectOptions, ConnectionTrait, Database, DatabaseConnection};
use sea_orm_migration::MigratorTraitSelf;
use tower::ServiceExt;

use cicada_audit::InMemoryAuditSink;
use cicada_auth::{
    ExternalIdentity, HttpOAuthClient, IdentityType, InMemoryOAuthStateStore, InMemorySessionStore,
    OAuthClient, OAuthService, OAuthStateStore, RecordingOAuthClient, Session, SessionStore,
    StaticCodeVerifier, TokenService, hash_refresh,
};
use cicada_infra::Db;
use cicada_infra::config::{Config, DbConfig, OAuthProviderConfig, Secret};
use cicada_rbac::{AuthzCache, InMemoryAuthzStore};
use cicada_system::{ApiKeyService, LocalDeptDirectory, TenantQuota};
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

fn provider() -> OAuthProviderConfig {
    OAuthProviderConfig {
        code: "keycloak".to_string(),
        kind: "oidc".to_string(),
        client_id: "cicada".to_string(),
        client_secret: Secret::new("secret".to_string()),
        authorize_url: "https://idp.example/authorize".to_string(),
        token_url: "https://idp.example/token".to_string(),
        userinfo_url: "https://idp.example/userinfo".to_string(),
        redirect_uri: "https://api.example/auth/login/keycloak/callback".to_string(),
        scopes: Vec::new(),
        identity_type: None,
    }
}

struct Fixture {
    state: AppState,
    token: String,
    client: Arc<RecordingOAuthClient>,
}

async fn setup(schema: &str) -> Fixture {
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

    let client = Arc::new(RecordingOAuthClient::new(vec![(
        "code-alice".to_string(),
        ExternalIdentity {
            identity_type: IdentityType::Oidc,
            identifier: "keycloak:alice".to_string(),
            provider: "keycloak".to_string(),
            nickname: Some("Alice".to_string()),
            avatar: None,
            email: Some("alice@example.com".to_string()),
        },
    )]));
    let states: Arc<dyn OAuthStateStore> = Arc::new(InMemoryOAuthStateStore::new());
    let oauth = Arc::new(OAuthService::new(
        client.clone(),
        states,
        vec![provider()],
        300,
        600,
    ));

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
        identity_verifier: Arc::new(StaticCodeVerifier::new("123456")),
        oauth,
        notify: Arc::new(cicada_notify::NotifyService::new(
            db.clone(),
            cicada_notify::shared(cicada_notify::RecordingSender::new()),
        )),
        api_keys: Arc::new(ApiKeyService::new(db.clone())),
        quota: Arc::new(TenantQuota::new(db.clone())),
        rate_limit: Arc::new(cicada_web::ratelimit::RateLimit::disabled()),
        job_runtime: Arc::new(cicada_ports::JobRuntime::new(
            Arc::new(cicada_ports::NoopRunSink),
            None,
        )),
        cache_inspector: Arc::new(cicada_ports::InMemoryCacheInspector::new()),
    };
    Fixture {
        state,
        token,
        client,
    }
}

fn req(method: &str, uri: &str, token: &str, body: Option<serde_json::Value>) -> Request<Body> {
    let mut b = Request::builder().method(method).uri(uri);
    if !token.is_empty() {
        b = b.header("authorization", format!("Bearer {token}"));
    }
    match body {
        Some(v) => b
            .header("content-type", "application/json")
            .body(Body::from(v.to_string()))
            .unwrap(),
        None => b.body(Body::empty()).unwrap(),
    }
}

async fn json_body(resp: axum::response::Response) -> serde_json::Value {
    let body = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
        .await
        .unwrap();
    if body.is_empty() {
        return serde_json::Value::Null;
    }
    serde_json::from_slice(&body).unwrap()
}

/// 取一次授权地址与 state。
async fn authorize(app: &axum::Router) -> (String, String) {
    let resp = app
        .clone()
        .oneshot(req("GET", "/auth/login/keycloak/authorize-url", "", None))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = json_body(resp).await;
    (
        body["authorize_url"].as_str().unwrap().to_string(),
        body["state"].as_str().unwrap().to_string(),
    )
}

#[tokio::test]
#[ignore = "需要真实 PostgreSQL"]
async fn oauth_unbound_then_bind_then_login() {
    let Fixture {
        state,
        token,
        client,
    } = setup("web_oauth_flow").await;
    let app = router(state);

    // 未配置的提供方 → 404（默认部署形态）。
    let unknown = app
        .clone()
        .oneshot(req("GET", "/auth/login/wechat/authorize-url", "", None))
        .await
        .unwrap();
    assert_eq!(unknown.status(), StatusCode::NOT_FOUND);

    // 1) 授权地址：带 state，且指向配置的授权端点。
    let (authorize_url, state) = authorize(&app).await;
    assert!(authorize_url.starts_with("https://idp.example/authorize?"));
    assert!(authorize_url.contains(&state));

    // 2) state 不对 → 401，且不消耗掉正确的 state。
    let bad = app
        .clone()
        .oneshot(req(
            "POST",
            "/auth/login/keycloak/callback",
            "",
            Some(serde_json::json!({ "code": "code-alice", "state": "forged" })),
        ))
        .await
        .unwrap();
    assert_eq!(bad.status(), StatusCode::UNAUTHORIZED);

    // 3) 正确回调：外部身份尚未绑定 → 只发绑定票据。
    let callback = app
        .clone()
        .oneshot(req(
            "POST",
            "/auth/login/keycloak/callback",
            "",
            Some(serde_json::json!({ "code": "code-alice", "state": state })),
        ))
        .await
        .unwrap();
    let status = callback.status();
    let body = json_body(callback).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["status"], "unbound");
    assert_eq!(body["external"]["nickname"], "Alice");
    assert_eq!(body["external"]["identifier"], "keycloak:alice");
    assert!(body["login"].is_null());
    let ticket = body["binding_ticket"].as_str().unwrap().to_string();

    // 同一个 state 不能重放（单次消费）。
    let replay = app
        .clone()
        .oneshot(req(
            "POST",
            "/auth/login/keycloak/callback",
            "",
            Some(serde_json::json!({ "code": "code-alice", "state": state })),
        ))
        .await
        .unwrap();
    assert_eq!(replay.status(), StatusCode::UNAUTHORIZED);

    // 4) 登录用户拿票据绑定到自己的账号。
    let bind = app
        .clone()
        .oneshot(req(
            "POST",
            "/auth/identities/oauth",
            &token,
            Some(serde_json::json!({ "binding_ticket": ticket })),
        ))
        .await
        .unwrap();
    let status = bind.status();
    let bound = json_body(bind).await;
    assert_eq!(status, StatusCode::CREATED, "{bound}");
    assert_eq!(bound["identity_type"], "Oidc");
    assert_eq!(bound["provider"], "keycloak");
    assert_eq!(bound["identifier"], "keycloak:alice");

    // 绑定后出现在身份列表里。
    let identities = app
        .clone()
        .oneshot(req("GET", "/auth/identities", &token, None))
        .await
        .unwrap();
    let list = json_body(identities).await;
    assert!(
        list.as_array()
            .unwrap()
            .iter()
            .any(|i| i["identifier"] == "keycloak:alice"),
        "{list}"
    );

    // 5) 再走一次：这次直接发令牌；令牌可用于受保护接口。
    let (_, state) = authorize(&app).await;
    let callback = app
        .clone()
        .oneshot(req(
            "POST",
            "/auth/login/keycloak/callback",
            "",
            Some(serde_json::json!({ "code": "code-alice", "state": state })),
        ))
        .await
        .unwrap();
    let status = callback.status();
    let body = json_body(callback).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["status"], "bound");
    assert_eq!(body["login"]["user_id"], 1);
    let access = body["login"]["access_token"].as_str().unwrap().to_string();

    let profile = app
        .clone()
        .oneshot(req("GET", "/auth/profile", &access, None))
        .await
        .unwrap();
    assert_eq!(profile.status(), StatusCode::OK);
    assert_eq!(client.calls().len(), 2);

    // 6) 再回调仍是「已绑定」：不会重复发票据（票据只在未绑定时出现）。
    //    重复绑定因此不可达；真正的兜底是 `uk_sys_user_identity` 唯一索引（并发竞态时返回 422）。
    let (_, state) = authorize(&app).await;
    let callback = app
        .clone()
        .oneshot(req(
            "POST",
            "/auth/login/keycloak/callback",
            "",
            Some(serde_json::json!({ "code": "code-alice", "state": state })),
        ))
        .await
        .unwrap();
    let body = json_body(callback).await;
    assert_eq!(body["status"], "bound", "{body}");
    assert!(body["binding_ticket"].is_null());
}

#[tokio::test]
#[ignore = "需要真实 PostgreSQL"]
async fn oauth_ticket_is_bound_to_the_signing_tenant() {
    let Fixture { state, .. } = setup("web_oauth_tenant").await;
    let app = router(state);

    // 票据用「另一个租户」签发 → 绑定必须被拒（即使票据本身有效）。
    let other_tokens = TokenService::new(JWT_SECRET, 900);
    let foreign = other_tokens
        .sign_binding_ticket(9, "Oidc", "keycloak", "keycloak:bob", None, 600)
        .unwrap();
    let admin = TokenService::new(JWT_SECRET, 900)
        .sign_access(1, 1, "sess-admin", "admin")
        .unwrap();
    let resp = app
        .clone()
        .oneshot(req(
            "POST",
            "/auth/identities/oauth",
            &admin,
            Some(serde_json::json!({ "binding_ticket": foreign })),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    // 伪造签名的票据同样被拒。
    let forged = app
        .clone()
        .oneshot(req(
            "POST",
            "/auth/identities/oauth",
            &admin,
            Some(serde_json::json!({ "binding_ticket": "not-a-jwt" })),
        ))
        .await
        .unwrap();
    assert_eq!(forged.status(), StatusCode::UNAUTHORIZED);
}

#[test]
fn http_client_is_constructible() {
    // 保证生产 adapter 能被组合根构造（不需要真实 IdP）。
    let _ = HttpOAuthClient::new();
    fn _assert_object_safe(_: Arc<dyn OAuthClient>) {}
}
