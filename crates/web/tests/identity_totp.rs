//! 身份绑定 / TOTP / 登录日志 / 审计中间件集成测试（真实 PostgreSQL + 内存会话）。

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
const ADMIN_PASSWORD: &str = "Cicada@2026";

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

async fn setup(schema: &str) -> (AppState, String, Arc<InMemoryAuditSink>) {
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

    let audit = Arc::new(InMemoryAuditSink::new());
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
        audit: audit.clone(),
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
    (state, token, audit)
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
    serde_json::from_slice(&body).unwrap()
}

#[tokio::test]
#[ignore = "需要真实 PostgreSQL"]
async fn identity_totp_and_login_logs() {
    let (state, token, audit) = setup("web_identity_test").await;
    let app = router(state);

    // ---- 登录成功 → 登录日志 success ----
    let login = app
        .clone()
        .oneshot(req(
            "POST",
            "/auth/login",
            "",
            Some(serde_json::json!({ "username": "admin", "password": ADMIN_PASSWORD })),
        ))
        .await
        .unwrap();
    assert_eq!(login.status(), StatusCode::OK);
    // ---- 登录失败（错误密码）→ 登录日志 fail，且不泄露账号是否存在 ----
    let bad = app
        .clone()
        .oneshot(req(
            "POST",
            "/auth/login",
            "",
            Some(serde_json::json!({ "username": "admin", "password": "wrong" })),
        ))
        .await
        .unwrap();
    assert_eq!(bad.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let logins = audit.logins();
    assert_eq!(logins.len(), 2);
    assert_eq!(logins[0].status, 0);
    assert_eq!(logins[1].status, 1);
    assert_eq!(logins[0].user_name, "admin");

    // ---- 初始身份：Username admin 为主身份 ----
    let list = app
        .clone()
        .oneshot(req("GET", "/auth/identities", &token, None))
        .await
        .unwrap();
    assert_eq!(list.status(), StatusCode::OK);
    let items = json_body(list).await;
    assert_eq!(items.as_array().unwrap().len(), 1);
    assert_eq!(items[0]["identity_type"], "Username");
    assert_eq!(items[0]["is_primary"], true);

    // ---- 绑定邮箱（验证码正确）→ 201，成为 Email 主身份 ----
    let bind = app
        .clone()
        .oneshot(req(
            "POST",
            "/auth/identities",
            &token,
            Some(serde_json::json!({
                "identity_type": "Email", "identifier": "Admin@Example.com", "code": "123456"
            })),
        ))
        .await
        .unwrap();
    let bind_status = bind.status();
    let bound = json_body(bind).await;
    assert_eq!(
        bind_status,
        StatusCode::CREATED,
        "email bind body = {bound}"
    );
    assert_eq!(bound["identifier"], "admin@example.com"); // 规范化小写
    assert_eq!(bound["is_primary"], true);

    // ---- 重复绑定 → 409（业务码目录 `CICADA_AUTH_1008`：身份已属于其他账号）----
    let dup = app
        .clone()
        .oneshot(req(
            "POST",
            "/auth/identities",
            &token,
            Some(serde_json::json!({
                "identity_type": "Email", "identifier": "admin@example.com", "code": "123456"
            })),
        ))
        .await
        .unwrap();
    assert_eq!(dup.status(), StatusCode::CONFLICT);

    // ---- 缺验证码 → 422 ----
    let no_code = app
        .clone()
        .oneshot(req(
            "POST",
            "/auth/identities",
            &token,
            Some(serde_json::json!({ "identity_type": "Phone", "identifier": "13800000000" })),
        ))
        .await
        .unwrap();
    assert_eq!(no_code.status(), StatusCode::UNPROCESSABLE_ENTITY);

    // ---- 手机号登录：绑定后可用手机 + 账号密码登录 ----
    let bind_phone = app
        .clone()
        .oneshot(req(
            "POST",
            "/auth/identities",
            &token,
            Some(serde_json::json!({
                "identity_type": "Phone", "identifier": "138 0000 0000", "code": "123456"
            })),
        ))
        .await
        .unwrap();
    assert_eq!(bind_phone.status(), StatusCode::CREATED);
    let phone_login = app
        .clone()
        .oneshot(req(
            "POST",
            "/auth/login",
            "",
            Some(serde_json::json!({ "username": "138-0000-0000", "password": ADMIN_PASSWORD })),
        ))
        .await
        .unwrap();
    assert_eq!(phone_login.status(), StatusCode::OK);

    // ---- 解绑：邮箱解绑后 204；解绑到只剩一个身份时 409 ----
    let email_id = bound["identity_id"].as_i64().unwrap();
    let unbind = app
        .clone()
        .oneshot(req(
            "DELETE",
            &format!("/auth/identities/{email_id}"),
            &token,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(unbind.status(), StatusCode::NO_CONTENT);

    // ---- TOTP：setup → enable → 登录需要动态码 ----
    let setup_resp = app
        .clone()
        .oneshot(req("POST", "/auth/totp/setup", &token, None))
        .await
        .unwrap();
    assert_eq!(setup_resp.status(), StatusCode::OK);
    let setup_json = json_body(setup_resp).await;
    let secret = setup_json["secret"].as_str().unwrap().to_string();
    assert!(
        setup_json["otpauth_url"]
            .as_str()
            .unwrap()
            .starts_with("otpauth://")
    );

    let status = app
        .clone()
        .oneshot(req("GET", "/auth/totp", &token, None))
        .await
        .unwrap();
    assert_eq!(json_body(status).await["enabled"], false);

    let code = cicada_auth::totp::current_code(&secret).unwrap();
    let enable = app
        .clone()
        .oneshot(req(
            "POST",
            "/auth/totp/enable",
            &token,
            Some(serde_json::json!({ "code": code })),
        ))
        .await
        .unwrap();
    assert_eq!(enable.status(), StatusCode::NO_CONTENT);

    // 缺 TOTP → 401；带 TOTP → 200。
    let no_totp = app
        .clone()
        .oneshot(req(
            "POST",
            "/auth/login",
            "",
            Some(serde_json::json!({ "username": "admin", "password": ADMIN_PASSWORD })),
        ))
        .await
        .unwrap();
    assert_eq!(no_totp.status(), StatusCode::UNAUTHORIZED);

    let code = cicada_auth::totp::current_code(&secret).unwrap();
    let with_totp = app
        .clone()
        .oneshot(req(
            "POST",
            "/auth/login",
            "",
            Some(serde_json::json!({ "username": "admin", "password": ADMIN_PASSWORD, "totp": code })),
        ))
        .await
        .unwrap();
    assert_eq!(with_totp.status(), StatusCode::OK);

    // ---- 关闭 TOTP → 204 ----
    let code = cicada_auth::totp::current_code(&secret).unwrap();
    let disable = app
        .clone()
        .oneshot(req(
            "POST",
            "/auth/totp/disable",
            &token,
            Some(serde_json::json!({ "code": code })),
        ))
        .await
        .unwrap();
    assert_eq!(disable.status(), StatusCode::NO_CONTENT);

    // ---- 审计中间件记录了身份与 TOTP 操作 ----
    let titles: Vec<String> = audit.opers().into_iter().map(|o| o.title).collect();
    assert!(
        titles.contains(&"auth:identity:bind".to_string()),
        "{titles:?}"
    );
    assert!(
        titles.contains(&"auth:identity:unbind".to_string()),
        "{titles:?}"
    );
    assert!(
        titles.contains(&"auth:totp:enable".to_string()),
        "{titles:?}"
    );
    assert!(
        titles.contains(&"auth:totp:disable".to_string()),
        "{titles:?}"
    );
}
