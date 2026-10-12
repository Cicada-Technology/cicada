//! 认证/授权接口测试（真实 PostgreSQL + `tower::ServiceExt::oneshot`）。
//!
//! 覆盖：登录 → 刷新（轮换）→ 旧刷新令牌重放 → 登出 → 令牌失效；
//! 以及有效令牌 `/auth/profile`、`/auth/routers`，无令牌 401。

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
    let token = tokens.sign_access(1, 1, "session-1", "admin").unwrap();

    let sessions = Arc::new(InMemorySessionStore::new());
    sessions
        .create(
            "session-1",
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

fn bearer(uri: &str, token: &str) -> Request<Body> {
    Request::builder()
        .uri(uri)
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap()
}

async fn json_body(resp: axum::response::Response) -> serde_json::Value {
    let body = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
        .await
        .unwrap();
    serde_json::from_slice(&body).unwrap()
}

#[tokio::test]
#[ignore = "需要真实 PostgreSQL"]
async fn profile_and_routers_with_valid_token() {
    let (state, token) = setup("web_auth_test_a").await;
    let app = router(state);

    let profile = app
        .clone()
        .oneshot(bearer("/auth/profile", &token))
        .await
        .unwrap();
    assert_eq!(profile.status(), StatusCode::OK);
    let json = json_body(profile).await;
    assert_eq!(json["user_name"], "admin");
    assert!(
        json["roles"]
            .as_array()
            .unwrap()
            .contains(&serde_json::json!("admin"))
    );
    assert!(!json["permissions"].as_array().unwrap().is_empty());

    let routers = app
        .clone()
        .oneshot(bearer("/auth/routers", &token))
        .await
        .unwrap();
    assert_eq!(routers.status(), StatusCode::OK);
    let tree = json_body(routers).await;
    assert!(
        tree.as_array().unwrap().len() >= 3,
        "菜单树应含系统管理/监控/工具三个根目录"
    );

    // 无令牌 → 401（profile 要求认证）。
    let unauth = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/auth/profile")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(unauth.status(), StatusCode::UNAUTHORIZED);

    // 会话不存在（模拟被强退）→ 401。
    let (state2, token2) = setup("web_auth_test_b").await;
    let _ = state2.sessions.revoke("session-1").await;
    let app2 = router(state2);
    let revoked = app2
        .oneshot(bearer("/auth/profile", &token2))
        .await
        .unwrap();
    assert_eq!(revoked.status(), StatusCode::UNAUTHORIZED);
}

async fn post_json(
    app: axum::Router,
    uri: &str,
    body: serde_json::Value,
) -> axum::response::Response {
    app.oneshot(
        Request::builder()
            .method("POST")
            .uri(uri)
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap(),
    )
    .await
    .unwrap()
}

#[tokio::test]
#[ignore = "需要真实 PostgreSQL"]
async fn login_refresh_replay_and_logout() {
    let (state, _) = setup("web_auth_test_c").await;
    let app = router(state);

    // 登录
    let login = post_json(
        app.clone(),
        "/auth/login",
        serde_json::json!({ "username": "admin", "password": "Cicada@2026" }),
    )
    .await;
    assert_eq!(login.status(), StatusCode::OK);
    let tokens = json_body(login).await;
    let _access = tokens["access_token"].as_str().unwrap().to_string();
    let refresh = tokens["refresh_token"].as_str().unwrap().to_string();

    // 刷新：拿到新令牌
    let refreshed = post_json(
        app.clone(),
        "/auth/refresh",
        serde_json::json!({ "refresh_token": refresh }),
    )
    .await;
    assert_eq!(refreshed.status(), StatusCode::OK);
    let tokens2 = json_body(refreshed).await;
    let access2 = tokens2["access_token"].as_str().unwrap().to_string();
    assert_ne!(tokens2["refresh_token"].as_str().unwrap(), refresh);

    // 旧刷新令牌重放 → 401，且整条会话链被撤销
    let replay = post_json(
        app.clone(),
        "/auth/refresh",
        serde_json::json!({ "refresh_token": refresh }),
    )
    .await;
    assert_eq!(replay.status(), StatusCode::UNAUTHORIZED);

    // 新 access 也随之失效
    let after_replay = app
        .clone()
        .oneshot(bearer("/auth/profile", &access2))
        .await
        .unwrap();
    assert_eq!(after_replay.status(), StatusCode::UNAUTHORIZED);

    // 重新登录 → 登出 → 令牌失效
    let login2 = post_json(
        app.clone(),
        "/auth/login",
        serde_json::json!({ "username": "admin", "password": "Cicada@2026" }),
    )
    .await;
    let t = json_body(login2).await;
    let access3 = t["access_token"].as_str().unwrap().to_string();

    let logout = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/auth/logout")
                .header("authorization", format!("Bearer {access3}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(logout.status(), StatusCode::NO_CONTENT);

    let after_logout = app
        .oneshot(bearer("/auth/profile", &access3))
        .await
        .unwrap();
    assert_eq!(after_logout.status(), StatusCode::UNAUTHORIZED);
}

fn with_ua(uri: &str, token: &str, ua: &str, forwarded_for: Option<&str>) -> Request<Body> {
    let mut b = Request::builder()
        .uri(uri)
        .header("authorization", format!("Bearer {token}"))
        .header("user-agent", ua);
    if let Some(ip) = forwarded_for {
        b = b.header("x-forwarded-for", ip);
    }
    b.body(Body::empty()).unwrap()
}

/// 会话指纹：换浏览器要重新登录，换 IP 不算——移动网络下 IP 一直在变。
#[tokio::test]
#[ignore = "需要真实 PostgreSQL"]
async fn session_fingerprint_revokes_on_user_agent_change_only() {
    let (state, token) = setup("web_auth_fingerprint").await;
    let app = router(state);

    // 同一个 UA、不同出口 IP：放行（会话里记的是登录时的 IP）。
    let ok = app
        .clone()
        .oneshot(with_ua(
            "/auth/profile",
            &token,
            "test",
            Some("203.0.113.7"),
        ))
        .await
        .unwrap();
    assert_eq!(ok.status(), StatusCode::OK, "IP 变化不该硬绑定");

    // UA 变了：401 + CICADA_AUTH_1011，语义是「去重新登录」而不是「无权限」。
    let resp = app
        .clone()
        .oneshot(with_ua("/auth/profile", &token, "OtherBrowser/9.9", None))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    let body = json_body(resp).await;
    assert_eq!(body["code"].as_str(), Some("CICADA_AUTH_1011"), "{body}");

    // 撤销是真的发生：拿回原来的 UA 也进不去了（会话已从权威源删掉）。
    let resp = app
        .oneshot(with_ua("/auth/profile", &token, "test", None))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    let body = json_body(resp).await;
    assert_eq!(
        body["code"].as_str(),
        Some("CICADA_AUTH_1001"),
        "会话被撤销后按未认证处理：{body}"
    );
}
