//! M4 交付物集成测试（真实 PostgreSQL）：通知中心、开放接口凭证、租户套餐限额。
//!
//! 默认 ignore（`-- --ignored` 运行）。

use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use sea_orm::{ConnectOptions, ConnectionTrait, Database, DatabaseConnection};
use sea_orm_migration::MigratorTraitSelf;
use tower::ServiceExt;

use cicada_audit::InMemoryAuditSink;
use cicada_auth::{
    InMemorySessionStore, Session, SessionStore, StaticCodeVerifier, TokenService, hash_refresh,
};
use cicada_infra::Db;
use cicada_infra::config::{Config, DbConfig, RateLimitConfig, Secret};
use cicada_notify::{NotifyService, RecordingSender};
use cicada_rbac::{AuthzCache, InMemoryAuthzStore};
use cicada_system::{ApiKeyService, LocalDeptDirectory, TenantQuota};
use cicada_web::ratelimit::RateLimit;
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

async fn setup(schema: &str) -> (AppState, String, Arc<RecordingSender>) {
    setup_with_rate_limit(schema, RateLimit::disabled()).await
}

/// 与 `setup` 相同，但可注入自定义限流配置（限流测试需要打开限流）。
async fn setup_with_rate_limit(
    schema: &str,
    rate_limit: RateLimit,
) -> (AppState, String, Arc<RecordingSender>) {
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

    let sender = Arc::new(RecordingSender::new());
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
        notify: Arc::new(NotifyService::new(db.clone(), sender.clone())),
        api_keys: Arc::new(ApiKeyService::new(db.clone())),
        quota: Arc::new(TenantQuota::new(db.clone())),
        rate_limit: Arc::new(rate_limit),
        oauth: Arc::new(cicada_auth::OAuthService::unconfigured()),
        job_runtime: Arc::new(cicada_ports::JobRuntime::new(
            Arc::new(cicada_ports::NoopRunSink),
            None,
        )),
        cache_inspector: Arc::new(cicada_ports::InMemoryCacheInspector::new()),
    };
    (state, token, sender)
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

fn req_api_key(
    method: &str,
    uri: &str,
    key: &str,
    body: Option<serde_json::Value>,
) -> Request<Body> {
    let b = Request::builder()
        .method(method)
        .uri(uri)
        .header("x-api-key", key);
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

/// 建好「webhook 渠道 + 模板」，返回 (app, 渠道/模板 id)。
async fn prepare_notify(app: &Router, token: &str) -> serde_json::Value {
    let channel = app
        .clone()
        .oneshot(req(
            "POST",
            "/api/v1/notify/channels",
            token,
            Some(serde_json::json!({
                "code": "ops-hook",
                "name": "运维 Webhook",
                "kind": "webhook",
                "config": { "endpoint": "https://hooks.example.invalid/ops" }
            })),
        ))
        .await
        .unwrap();
    let status = channel.status();
    let channel = json_body(channel).await;
    assert_eq!(status, StatusCode::CREATED, "{channel}");

    let template = app
        .clone()
        .oneshot(req(
            "POST",
            "/api/v1/notify/templates",
            token,
            Some(serde_json::json!({
                "code": "alert",
                "name": "告警",
                "channel_kind": "webhook",
                "title": "{{ level }} 告警",
                "body": "服务 {{ service }} 在 {{ time }} 异常"
            })),
        ))
        .await
        .unwrap();
    let status = template.status();
    let template = json_body(template).await;
    assert_eq!(status, StatusCode::CREATED, "{template}");

    serde_json::json!({ "channel": channel, "template": template })
}

#[tokio::test]
#[ignore = "需要真实 PostgreSQL"]
async fn notify_center_renders_delivers_and_records() {
    let (state, token, sender) = setup("web_m4_notify").await;
    let app = router(state);
    let ids = prepare_notify(&app, &token).await;
    assert!(ids["channel"]["config"]["endpoint"].is_string());

    // 语法错误的模板应当被拒（保存期就暴露，而不是发送时）。
    let bad = app
        .clone()
        .oneshot(req(
            "POST",
            "/api/v1/notify/templates",
            &token,
            Some(serde_json::json!({
                "code": "broken", "name": "坏模板", "channel_kind": "webhook",
                "title": "x", "body": "{% if %}"
            })),
        ))
        .await
        .unwrap();
    assert_eq!(bad.status(), StatusCode::UNPROCESSABLE_ENTITY);

    // 发送：模板渲染 + 渠道投递 + 记录留痕。
    let send = app
        .clone()
        .oneshot(req(
            "POST",
            "/api/v1/notify/send",
            &token,
            Some(serde_json::json!({
                "channel_code": "ops-hook",
                "template_code": "alert",
                "recipient": "ops@example.com",
                "payload": { "level": "P1", "service": "payment", "time": "10:00" }
            })),
        ))
        .await
        .unwrap();
    let status = send.status();
    let sent = json_body(send).await;
    assert_eq!(status, StatusCode::OK, "{sent}");
    assert_eq!(sent["status"], 1);
    assert_eq!(sent["rendered_body"], "服务 payment 在 10:00 异常");

    // 投递内容确实按渠道约定编码了（webhook 带上原始 payload）。
    let deliveries = sender.deliveries();
    assert_eq!(deliveries.len(), 1);
    assert_eq!(deliveries[0].recipient, "ops@example.com");
    assert_eq!(deliveries[0].title, "P1 告警");

    // 记录可查，状态为「已发」。
    let records = app
        .clone()
        .oneshot(req("GET", "/api/v1/notify/records", &token, None))
        .await
        .unwrap();
    assert_eq!(records.status(), StatusCode::OK);
    let records = json_body(records).await;
    assert_eq!(records["total"], 1);
    assert_eq!(records["items"][0]["status"], 1);
    assert_eq!(records["items"][0]["recipient"], "ops@example.com");

    // 停用渠道后再发 → 409（渠道停用）。
    let channel_id = ids["channel"]["id"].as_i64().unwrap();
    let disable = app
        .clone()
        .oneshot(req(
            "PUT",
            &format!("/api/v1/notify/channels/{channel_id}"),
            &token,
            Some(serde_json::json!({
                "code": "ops-hook", "name": "运维 Webhook", "kind": "webhook",
                "config": { "endpoint": "https://hooks.example.invalid/ops" },
                "enabled": false
            })),
        ))
        .await
        .unwrap();
    assert_eq!(disable.status(), StatusCode::OK);

    let send_again = app
        .clone()
        .oneshot(req(
            "POST",
            "/api/v1/notify/send",
            &token,
            Some(serde_json::json!({
                "channel_code": "ops-hook",
                "template_code": "alert",
                "recipient": "ops@example.com",
                "payload": { "level": "P2", "service": "cart", "time": "11:00" }
            })),
        ))
        .await
        .unwrap();
    assert_eq!(send_again.status(), StatusCode::CONFLICT);
}

#[tokio::test]
#[ignore = "需要真实 PostgreSQL"]
async fn api_key_scopes_gate_access() {
    let (state, token, _) = setup("web_m4_apikey").await;
    let app = router(state);

    // 签发：只给「用户列表」权限。
    let issued = app
        .clone()
        .oneshot(req(
            "POST",
            "/api/v1/system/api-keys",
            &token,
            Some(serde_json::json!({
                "name": "报表只读",
                "scopes": ["system:user:list"]
            })),
        ))
        .await
        .unwrap();
    let status = issued.status();
    let issued = json_body(issued).await;
    assert_eq!(status, StatusCode::CREATED, "{issued}");
    let secret = issued["secret"].as_str().unwrap().to_string();
    let id = issued["id"].as_i64().unwrap();
    assert!(secret.starts_with("ck_"), "{secret}");
    // 响应里只有前缀，没有哈希。
    assert_eq!(issued["key_prefix"].as_str().unwrap(), &secret[..16]);
    assert!(issued.get("secret_hash").is_none());

    // 有权限的接口放行。
    let list = app
        .clone()
        .oneshot(req_api_key("GET", "/api/v1/system/users", &secret, None))
        .await
        .unwrap();
    assert_eq!(list.status(), StatusCode::OK);

    // 没在 scopes 里的接口 403。
    let create = app
        .clone()
        .oneshot(req_api_key(
            "POST",
            "/api/v1/system/users",
            &secret,
            Some(serde_json::json!({
                "user_name": "k1", "nick_name": "K", "password": "K@123456"
            })),
        ))
        .await
        .unwrap();
    assert_eq!(create.status(), StatusCode::FORBIDDEN);

    // 伪造前缀无效。
    let bogus = app
        .clone()
        .oneshot(req_api_key(
            "GET",
            "/api/v1/system/users",
            "ck_00000000000000",
            None,
        ))
        .await
        .unwrap();
    assert_eq!(bogus.status(), StatusCode::UNAUTHORIZED);

    // 停用后立即失效（无状态鉴权的代价是每次都要查库，这里正是要验证这点）。
    let disable = app
        .clone()
        .oneshot(req(
            "PUT",
            &format!("/api/v1/system/api-keys/{id}/status"),
            &token,
            Some(serde_json::json!({ "status": 1 })),
        ))
        .await
        .unwrap();
    assert_eq!(disable.status(), StatusCode::NO_CONTENT);
    let after = app
        .clone()
        .oneshot(req_api_key("GET", "/api/v1/system/users", &secret, None))
        .await
        .unwrap();
    assert_eq!(after.status(), StatusCode::UNAUTHORIZED);

    // 列表不泄露明文。
    let list = app
        .clone()
        .oneshot(req("GET", "/api/v1/system/api-keys", &token, None))
        .await
        .unwrap();
    let list = json_body(list).await;
    let raw = list.to_string();
    assert!(!raw.contains(&secret));
}

#[tokio::test]
#[ignore = "需要真实 PostgreSQL"]
async fn tenant_package_quota_blocks_user_creation() {
    let (state, token, _) = setup("web_m4_quota").await;
    let app = router(state);

    // 套餐：只允许 1 个用户（默认租户已有 admin，正好占满）。
    let package = app
        .clone()
        .oneshot(req(
            "POST",
            "/api/v1/system/tenant-packages",
            &token,
            Some(serde_json::json!({
                "code": "trial",
                "name": "试用版",
                "user_limit": 1,
                "storage_limit_mb": 10,
                "api_qps_limit": 5
            })),
        ))
        .await
        .unwrap();
    let status = package.status();
    let package = json_body(package).await;
    assert_eq!(status, StatusCode::CREATED, "{package}");
    let package_id = package["id"].as_i64().unwrap();

    // 绑定到默认租户。
    let bind = app
        .clone()
        .oneshot(req(
            "PUT",
            "/api/v1/system/tenants/1",
            &token,
            Some(serde_json::json!({
                "tenant_name": "默认租户", "status": 0, "package_id": package_id
            })),
        ))
        .await
        .unwrap();
    assert_eq!(bind.status(), StatusCode::OK);

    // 用量快照：1 个用户已占用。
    let quota = app
        .clone()
        .oneshot(req("GET", "/api/v1/system/tenants/1/quota", &token, None))
        .await
        .unwrap();
    let quota = json_body(quota).await;
    assert_eq!(quota["users"], 1);
    assert_eq!(quota["user_limit"], 1);
    assert_eq!(quota["package_code"], "trial");

    // 再建号 → 409（限额，而不是 403 权限）。
    let blocked = app
        .clone()
        .oneshot(req(
            "POST",
            "/api/v1/system/users",
            &token,
            Some(serde_json::json!({
                "user_name": "bob", "nick_name": "Bob", "password": "Bob@123456"
            })),
        ))
        .await
        .unwrap();
    assert_eq!(blocked.status(), StatusCode::CONFLICT);
    let msg = json_body(blocked).await;
    assert!(msg["detail"].as_str().unwrap().contains("用户数已达上限"));

    // 放宽限额后即可建号。
    let widen = app
        .clone()
        .oneshot(req(
            "PUT",
            &format!("/api/v1/system/tenant-packages/{package_id}"),
            &token,
            Some(serde_json::json!({
                "code": "trial", "name": "试用版", "user_limit": 5,
                "storage_limit_mb": 10, "api_qps_limit": 5
            })),
        ))
        .await
        .unwrap();
    assert_eq!(widen.status(), StatusCode::OK);

    let ok = app
        .clone()
        .oneshot(req(
            "POST",
            "/api/v1/system/users",
            &token,
            Some(serde_json::json!({
                "user_name": "bob", "nick_name": "Bob", "password": "Bob@123456"
            })),
        ))
        .await
        .unwrap();
    assert_eq!(ok.status(), StatusCode::CREATED, "{}", json_body(ok).await);

    // 仍被租户引用的套餐不可删（409）。
    let del = app
        .clone()
        .oneshot(req(
            "DELETE",
            &format!("/api/v1/system/tenant-packages/{package_id}"),
            &token,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(del.status(), StatusCode::CONFLICT);
}

#[tokio::test]
#[ignore = "需要真实 PostgreSQL"]
async fn rate_limit_ip_dimension_returns_429() {
    // 单 IP 每秒 1 次、无突发：第二个非豁免请求必被挡。
    let limit = RateLimit::new(&RateLimitConfig {
        enabled: true,
        per_ip_qps: 1,
        per_ip_burst: 1,
        tenant_cache_ttl_secs: 60,
    });
    let (state, token, _) = setup_with_rate_limit("web_m4_ratelimit_ip", limit).await;
    let app = router(state);

    let first = app
        .clone()
        .oneshot(req("GET", "/api/v1/system/users", &token, None))
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::OK);

    let second = app
        .clone()
        .oneshot(req("GET", "/api/v1/system/users", &token, None))
        .await
        .unwrap();
    assert_eq!(second.status(), StatusCode::TOO_MANY_REQUESTS);
    // 429 必须带 Retry-After，客户端才能退避重试。
    let retry_after = second
        .headers()
        .get("retry-after")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok())
        .expect("429 应带 Retry-After");
    assert!(retry_after >= 1);

    // 探针端点豁免：桶已空也放行，避免编排探针抖动导致重启。
    let health = app
        .clone()
        .oneshot(req("GET", "/healthz", "", None))
        .await
        .unwrap();
    assert_eq!(health.status(), StatusCode::OK);
}

#[tokio::test]
#[ignore = "需要真实 PostgreSQL"]
async fn tenant_package_qps_limit_returns_429() {
    // IP 维度关闭（0 = 不限），只验证租户套餐的 QPS 上限。
    let limit = RateLimit::new(&RateLimitConfig {
        enabled: true,
        per_ip_qps: 0,
        per_ip_burst: 0,
        tenant_cache_ttl_secs: 60,
    });
    let (state, token, _) = setup_with_rate_limit("web_m4_ratelimit_tenant", limit).await;

    // 默认租户挂「QPS = 1」的套餐。
    state
        .db
        .conn()
        .execute_unprepared(
            "INSERT INTO sys_tenant_package (code, name, api_qps_limit) VALUES ('starter', '入门版', 1)",
        )
        .await
        .unwrap();
    state
        .db
        .conn()
        .execute_unprepared(
            "UPDATE sys_tenant SET package_id = (SELECT id FROM sys_tenant_package WHERE code = 'starter') WHERE tenant_id = 1",
        )
        .await
        .unwrap();

    let app = router(state);

    let first = app
        .clone()
        .oneshot(req("GET", "/api/v1/system/users", &token, None))
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::OK);

    let second = app
        .clone()
        .oneshot(req("GET", "/api/v1/system/users", &token, None))
        .await
        .unwrap();
    assert_eq!(second.status(), StatusCode::TOO_MANY_REQUESTS);

    // 探针端点同样豁免租户维度。
    let health = app
        .clone()
        .oneshot(req("GET", "/readyz", "", None))
        .await
        .unwrap();
    assert_eq!(health.status(), StatusCode::OK);
}

/// 邮件渠道的 SMTP 形状（技术选型的 lettre 选型）与凭据不回显。
#[tokio::test]
#[ignore = "需要真实 PostgreSQL"]
async fn smtp_email_channel_is_accepted_and_its_secret_is_masked() {
    let (state, token, _sender) = setup("web_m4_smtp_channel").await;
    let app = router(state);

    // 少填 password：建渠道时就 422，而不是等第一次投递。
    let half = app
        .clone()
        .oneshot(req(
            "POST",
            "/api/v1/notify/channels",
            &token,
            Some(serde_json::json!({
                "code": "mail-half",
                "name": "缺口令的 SMTP 渠道",
                "kind": "email",
                "config": {
                    "host": "smtp.example.invalid",
                    "from": "noreply@example.invalid",
                    "username": "notify@example.invalid"
                }
            })),
        ))
        .await
        .unwrap();
    let status = half.status();
    let body = json_body(half).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    let detail = serde_json::to_string(&body).unwrap();
    assert!(detail.contains("password"), "报错要点名缺的字段：{detail}");

    // 完整 SMTP 形状：201，且响应里的 password 被掩掉。
    let created = app
        .clone()
        .oneshot(req(
            "POST",
            "/api/v1/notify/channels",
            &token,
            Some(serde_json::json!({
                "code": "mail-smtp",
                "name": "SMTP 邮件渠道",
                "kind": "email",
                "config": {
                    "host": "smtp.example.invalid",
                    "port": 465,
                    "tls": "ssl",
                    "from": "NoReply <noreply@example.invalid>",
                    "username": "notify@example.invalid",
                    "password": "hunter2"
                }
            })),
        ))
        .await
        .unwrap();
    let status = created.status();
    let vo = json_body(created).await;
    assert_eq!(status, StatusCode::CREATED, "{vo}");
    assert_eq!(vo["config"]["host"], "smtp.example.invalid");
    assert_eq!(vo["config"]["tls"], "ssl");
    assert_eq!(vo["config"]["password"], "***", "凭据不许回显：{vo}");
    assert_eq!(vo["config"]["port"], 465);
    // 远端 + 明文（tls=none）也要在建渠道时拦住。
    let plaintext = app
        .clone()
        .oneshot(req(
            "POST",
            "/api/v1/notify/channels",
            &token,
            Some(serde_json::json!({
                "code": "mail-plain",
                "name": "明文 SMTP",
                "kind": "email",
                "config": {
                    "host": "smtp.example.invalid",
                    "tls": "none",
                    "from": "noreply@example.invalid"
                }
            })),
        ))
        .await
        .unwrap();
    let status = plaintext.status();
    let body = json_body(plaintext).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert!(
        serde_json::to_string(&body).unwrap().contains("回环"),
        "应当说明明文只允许本地 relay：{body}"
    );

    // 列表接口同样掩码。
    let list = app
        .clone()
        .oneshot(req("GET", "/api/v1/notify/channels", &token, None))
        .await
        .unwrap();
    let page = json_body(list).await;
    let raw = serde_json::to_string(&page).unwrap();
    assert!(!raw.contains("hunter2"), "列表里漏出了口令：{raw}");
    assert!(raw.contains("mail-smtp"), "渠道没进列表：{raw}");
}

/// 越权防护归属校验：写接口提交的角色 / 部门引用必须落在调用者自己的租户里。
///
/// 这是「单库单 schema + `tenant_id` 列」（ADR 0004）下的真实通道：授权装载确实按租户
/// 过滤，所以挂上别的租户的角色不会**立刻**提权，但 `sys_user_role` / `sys_role_dept`
/// 里会留下一条跨租户的引用——那行以后被回收、被改归属，权限就凭空出现。拦住写入口，
/// 而不是指望读侧永远记得过滤。
#[tokio::test]
#[ignore = "需要真实 PostgreSQL"]
async fn cross_tenant_references_are_rejected() {
    use cicada_system::{DeptActiveModel, RoleActiveModel};
    use sea_orm::{ActiveModelTrait, Set};

    let (state, token, _) = setup("web_m4_ownership").await;
    let app = router(state.clone());
    let db = state.db.conn();

    let foreign_role = RoleActiveModel {
        tenant_id: Set(2),
        role_name: Set("外租户角色".to_string()),
        role_key: Set("foreign_role".to_string()),
        role_sort: Set(9),
        data_scope: Set(1),
        status: Set(0),
        create_time: Set(chrono::Utc::now()),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();
    let foreign_dept = DeptActiveModel {
        tenant_id: Set(2),
        parent_id: Set(0),
        ancestors: Set("0".to_string()),
        dept_name: Set("外租户部门".to_string()),
        order_num: Set(1),
        status: Set(0),
        create_time: Set(chrono::Utc::now()),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();

    // 1) 建用户时挂别的租户的角色。
    let resp = app
        .clone()
        .oneshot(req(
            "POST",
            "/api/v1/system/users",
            &token,
            Some(serde_json::json!({
                "user_name": "borrowed_roles", "nick_name": "借来的角色",
                "password": "Cicada@2026", "role_ids": [foreign_role.role_id]
            })),
        ))
        .await
        .unwrap();
    assert_2002(resp, "跨租户角色").await;

    // 2) 建用户时放进别的租户的部门。
    let resp = app
        .clone()
        .oneshot(req(
            "POST",
            "/api/v1/system/users",
            &token,
            Some(serde_json::json!({
                "user_name": "borrowed_dept", "nick_name": "借来的部门",
                "password": "Cicada@2026", "dept_id": foreign_dept.dept_id
            })),
        ))
        .await
        .unwrap();
    assert_2002(resp, "跨租户部门").await;

    // 3) 角色的自定义数据范围（data_scope=2）指向别的租户的部门。
    let local = app
        .clone()
        .oneshot(req(
            "POST",
            "/api/v1/system/roles",
            &token,
            Some(serde_json::json!({
                "role_name": "本租户范围角色", "role_key": "local_scope_role"
            })),
        ))
        .await
        .unwrap();
    assert_eq!(local.status(), StatusCode::CREATED);
    let local_role = json_body(local).await["role_id"].as_i64().unwrap();

    let resp = app
        .clone()
        .oneshot(req(
            "PUT",
            &format!("/api/v1/system/roles/{local_role}"),
            &token,
            Some(serde_json::json!({
                "role_name": "本租户范围角色", "role_key": "local_scope_role",
                "data_scope": 2, "dept_ids": [foreign_dept.dept_id], "version": 0
            })),
        ))
        .await
        .unwrap();
    assert_2002(resp, "跨租户数据范围").await;

    // 4) 正例：本租户的角色与部门照常能挂——拦的是跨租户，不是所有引用。
    let dept = app
        .clone()
        .oneshot(req(
            "POST",
            "/api/v1/system/depts",
            &token,
            Some(serde_json::json!({ "parent_id": 0, "dept_name": "归属校验组" })),
        ))
        .await
        .unwrap();
    assert_eq!(dept.status(), StatusCode::CREATED);
    let local_dept = json_body(dept).await["dept_id"].as_i64().unwrap();

    let ok = app
        .clone()
        .oneshot(req(
            "POST",
            "/api/v1/system/users",
            &token,
            Some(serde_json::json!({
                "user_name": "local_refs", "nick_name": "本租户引用",
                "password": "Cicada@2026", "dept_id": local_dept, "role_ids": [local_role]
            })),
        ))
        .await
        .unwrap();
    assert_eq!(ok.status(), StatusCode::CREATED, "本租户的引用不该被拦");

    // 5) 悬空的菜单引用同样当场拒绝（`sys_menu` 是全局表，这里退化成存在性校验）。
    let resp = app
        .clone()
        .oneshot(req(
            "PUT",
            &format!("/api/v1/system/roles/{local_role}"),
            &token,
            Some(serde_json::json!({
                "role_name": "本租户范围角色", "role_key": "local_scope_role",
                "menu_ids": [9_999_999_i64], "version": 0
            })),
        ))
        .await
        .unwrap();
    assert_2002(resp, "不存在的菜单").await;
}

/// 归属校验的正确形状：403 + 业务码目录的 `CICADA_RBAC_2002`。
/// 用 fn 而不是闭包，是因为断言体要 `await`，闭包会把 `label` 的寿命绑进返回类型里。
async fn assert_2002(resp: axum::response::Response, label: &str) {
    let status = resp.status();
    let body = json_body(resp).await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "{label} 应被归属校验拦住：{body}"
    );
    assert_eq!(
        body["code"].as_str(),
        Some("CICADA_RBAC_2002"),
        "{label} 要回业务码目录的租户不匹配码：{body}"
    );
}
