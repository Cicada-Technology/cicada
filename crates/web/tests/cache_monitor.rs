//! 缓存监控集成测试：概览 / 分页扫描 / 键元信息 /
//! 按域清理 / 参数缓存刷新 / 审计落库。
//!
//! 用 `InMemoryCacheInspector` 作观测后端，验证的是**HTTP 面的白名单与权限边界**，
//! 不依赖真实 Redis；真实 Redis adapter 的白名单在同名单元测试里覆盖。

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
use cicada_ports::{CacheInspector, InMemoryCacheInspector};
use cicada_rbac::{AuthzCache, InMemoryAuthzStore};
use cicada_system::LocalDeptDirectory;
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

/// 建库 schema + 迁移 + 种子，返回 (状态, 令牌, 可断言的审计 sink, 可断言的缓存后端)。
async fn setup(
    schema: &str,
) -> (
    AppState,
    String,
    Arc<InMemoryAuditSink>,
    Arc<InMemoryCacheInspector>,
) {
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

    let audit = Arc::new(InMemoryAuditSink::new());
    let cache = Arc::new(InMemoryCacheInspector::new());
    // 权限集合缓存 2 条 + 会话 1 条 + 锁定 1 条 + 参数缓存 2 条 + 白名单外的键 1 条。
    cache.set("cicada:authz:1:1:v1", "{}", Some(60));
    cache.set("cicada:authz:1:2:v1", "{}", Some(60));
    cache.set("session:abc", "{}", Some(60));
    cache.set("pwd_lock:admin", "1", Some(60));
    cache.set(
        "cicada:config:1:sys.user.initPassword:v1",
        "123456",
        Some(60),
    );
    cache.set("cicada:configver:1", "2", Some(60));
    cache.set("some:other:app:key", "kept", None);

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
        cache_inspector: cache.clone(),
    };
    (state, token, audit, cache)
}

fn get(uri: &str, token: &str) -> Request<Body> {
    Request::builder()
        .method("GET")
        .uri(uri)
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap()
}

fn method_req(method: &str, uri: &str, token: &str) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap()
}

async fn json_body(resp: axum::response::Response) -> serde_json::Value {
    let body = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
        .await
        .unwrap();
    serde_json::from_slice(&body).unwrap_or(serde_json::Value::Null)
}

#[tokio::test]
async fn cache_observes_within_whitelist_only() {
    let (state, token, _audit, _cache) = setup("web_cache_monitor").await;
    let app = cicada_web::router(state);

    // 概览：库里 7 个键，但只有白名单内的域被统计。
    let resp = app
        .clone()
        .oneshot(get("/api/v1/monitor/cache/overview", &token))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = json_body(resp).await;
    assert_eq!(body["info"]["db_keys"].as_u64().unwrap(), 7);
    assert_eq!(body["info"]["hit_rate"].as_f64().unwrap(), 0.0);
    let domains = body["domains"].as_array().unwrap();
    let authz = domains.iter().find(|d| d["name"] == "authz").unwrap();
    assert_eq!(authz["keys"].as_u64().unwrap(), 2);
    let config = domains.iter().find(|d| d["name"] == "config").unwrap();
    assert_eq!(config["keys"].as_u64().unwrap(), 2, "config 域含版本号键");
    let session = domains.iter().find(|d| d["name"] == "session").unwrap();
    assert!(session["sensitive"].as_bool().unwrap(), "会话域应标记敏感");

    // 域列表：与白名单一致。
    let resp = app
        .clone()
        .oneshot(get("/api/v1/monitor/cache/domains", &token))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        json_body(resp).await["items"].as_array().unwrap().len(),
        cicada_monitor::cache::CACHE_DOMAINS.len()
    );

    // 域内键分页：只列该域的键。
    let resp = app
        .clone()
        .oneshot(get("/api/v1/monitor/cache/keys?domain=authz", &token))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let page = json_body(resp).await;
    assert_eq!(page["keys"].as_array().unwrap().len(), 2);
    assert_eq!(page["cursor"].as_u64().unwrap(), 0);
    assert!(!page["has_more"].as_bool().unwrap());

    // 单键元信息：域内可查。
    let resp = app
        .clone()
        .oneshot(get(
            "/api/v1/monitor/cache/key?key=pwd_lock%3Aadmin",
            &token,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let meta = json_body(resp).await;
    assert_eq!(meta["domain"], "lockout");
    assert_eq!(meta["kind"], "string");
    assert!(meta["ttl_secs"].as_i64().unwrap() > 0);

    // 白名单外：域名与键名都必须被拒（422），绝不静默降级。
    for uri in [
        "/api/v1/monitor/cache/keys?domain=nope",
        "/api/v1/monitor/cache/key?key=some%3Aother%3Aapp%3Akey",
    ] {
        let resp = app.clone().oneshot(get(uri, &token)).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY, "{uri}");
    }
}

#[tokio::test]
async fn cache_clears_by_domain_and_audits() {
    let (state, token, audit, cache) = setup("web_cache_clear").await;
    let app = cicada_web::router(state);

    // 清单个键：必须落在指定域内。
    let resp = app
        .clone()
        .oneshot(method_req(
            "DELETE",
            "/api/v1/monitor/cache/keys?domain=lockout&key=pwd_lock%3Aadmin",
            &token,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(json_body(resp).await["removed"].as_u64().unwrap(), 1);
    assert!(cache.get("pwd_lock:admin").is_none());

    // 跨域键：拒绝。
    let resp = app
        .clone()
        .oneshot(method_req(
            "DELETE",
            "/api/v1/monitor/cache/keys?domain=lockout&key=session%3Aabc",
            &token,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert!(cache.get("session:abc").is_some(), "拒绝时不得删键");

    // 清整域。
    let resp = app
        .clone()
        .oneshot(method_req(
            "DELETE",
            "/api/v1/monitor/cache/keys?domain=authz",
            &token,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(json_body(resp).await["removed"].as_u64().unwrap(), 2);
    assert_eq!(
        cache.dbsize().await.unwrap(),
        4,
        "只清掉 lockout:admin 与 authz 域"
    );

    // 破坏性操作必须留痕（`#[audit]` 中间件）。
    let logged = audit.opers();
    let clear = logged
        .iter()
        .filter(|l| l.oper_url == "/api/v1/monitor/cache/keys")
        .collect::<Vec<_>>();
    // 三次调用都落审计：两次成功 + 一次被拒（跨域键），拒绝同样要留痕。
    assert_eq!(clear.len(), 3);
    assert_eq!(clear.iter().filter(|l| l.status == 0).count(), 2);
    assert_eq!(clear.iter().filter(|l| l.status == 1).count(), 1);
    assert_eq!(clear[0].operator.as_deref(), Some("admin"));
}

#[tokio::test]
async fn refresh_config_cache_flushes_the_config_domain() {
    let (state, token, _audit, cache) = setup("web_cache_refresh").await;
    let app = cicada_web::router(state);

    let resp = app
        .clone()
        .oneshot(method_req(
            "POST",
            "/api/v1/system/configs/refresh-cache",
            &token,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(json_body(resp).await["removed"].as_u64().unwrap(), 2);

    assert!(
        cache
            .get("cicada:config:1:sys.user.initPassword:v1")
            .is_none()
    );
    assert!(cache.get("cicada:configver:1").is_none());
    assert!(cache.get("cicada:authz:1:1:v1").is_some(), "不得误伤其他域");
}
