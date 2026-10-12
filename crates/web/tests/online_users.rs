//! 在线用户管理接口测试（真实 PostgreSQL + 内存会话存储）。
//!
//! 覆盖：列表（Redis 为权威源）、单条强退、批量强退、未认证 401。

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use sea_orm::{ConnectOptions, ConnectionTrait, Database, DatabaseConnection};
use sea_orm_migration::MigratorTraitSelf;
use tower::ServiceExt;

use cicada_audit::InMemoryAuditSink;
use cicada_auth::{
    InMemorySessionStore, Session, SessionStore, SnapshottedSessionStore, TokenService,
    hash_refresh,
};
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

async fn session(store: &dyn SessionStore, jti: &str, user_id: i64, last_seen: i64) {
    store
        .create(
            jti,
            &Session {
                user_id,
                tenant_id: 1,
                username: if user_id == 1 { "admin" } else { "alice" }.to_string(),
                refresh_hash: hash_refresh("r"),
                ip: "10.0.0.1".to_string(),
                user_agent: "test".to_string(),
                login_time: 1,
                last_seen,
            },
            600,
        )
        .await
        .unwrap();
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

    // 与生产组装根同一条路径：权威源（这里用内存实现代替 Redis）+ `sys_user_online`
    // write-behind 快照。测试因此也能验到历史接口。
    let sessions: Arc<dyn SessionStore> = Arc::new(SnapshottedSessionStore::new(
        Arc::new(InMemorySessionStore::new()),
        db.clone(),
    ));
    session(sessions.as_ref(), "sess-admin", 1, 100).await;
    session(sessions.as_ref(), "sess-alice", 2, 200).await;

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

async fn json_body(resp: axum::response::Response) -> serde_json::Value {
    let body = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
        .await
        .unwrap();
    serde_json::from_slice(&body).unwrap()
}

fn del(uri: &str, token: &str) -> Request<Body> {
    Request::builder()
        .method("DELETE")
        .uri(uri)
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap()
}

#[tokio::test]
#[ignore = "需要真实 PostgreSQL"]
async fn list_force_and_batch_force_logout() {
    let (state, token) = setup("web_online_test").await;
    let app = router(state);

    // 列表：应含两个会话，按 last_seen 倒序（alice 在前）
    let list = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/monitor/online-users")
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(list.status(), StatusCode::OK);
    let page = json_body(list).await;
    assert_eq!(page["total"], 2);
    assert_eq!(page["items"][0]["session_id"], "sess-alice");

    // 按用户名过滤
    let filtered = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/monitor/online-users?user_name=alice")
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let filtered = json_body(filtered).await;
    assert_eq!(filtered["total"], 1);

    // 单条强退
    let force = app
        .clone()
        .oneshot(del("/api/v1/monitor/online-users/sess-alice", &token))
        .await
        .unwrap();
    assert_eq!(force.status(), StatusCode::NO_CONTENT);

    let after = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/monitor/online-users")
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let after = json_body(after).await;
    assert_eq!(after["total"], 1);
    assert_eq!(after["items"][0]["session_id"], "sess-admin");

    // 批量强退（含不存在的 id，幂等）
    let batch = app
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri("/api/v1/monitor/online-users")
                .header("authorization", format!("Bearer {token}"))
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({ "session_ids": ["sess-admin", "nope"] }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(batch.status(), StatusCode::NO_CONTENT);

    // 未认证 → 401
    let unauth = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/monitor/online-users")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(unauth.status(), StatusCode::UNAUTHORIZED);
}

/// 轮询后台写入的结果：快照写是 fire-and-forget，断言不能靠 `sleep` 猜时间。
async fn wait_count(state: &AppState, sql: &str, want: i64) -> i64 {
    for _ in 0..100 {
        let rows = state
            .db
            .conn()
            .query_all_raw(sea_orm::Statement::from_string(
                sea_orm::DatabaseBackend::Postgres,
                sql.to_string(),
            ))
            .await
            .unwrap();
        let got = rows
            .first()
            .and_then(|r| r.try_get_by_index::<i64>(0).ok())
            .unwrap_or(0);
        if got == want {
            return got;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    panic!("等待 {sql} = {want} 超时");
}

fn get(uri: &str, token: &str) -> Request<Body> {
    Request::builder()
        .uri(uri)
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap()
}

/// 会话历史（在线用户管理页读 Redis，历史审计读快照表）。
///
/// 快照是异步写的：登出/强退只把行标成「已退出」，不删——历史的价值就在这些行里。
#[tokio::test]
#[ignore = "需要真实 PostgreSQL"]
async fn session_history_is_written_behind_and_reconcilable() {
    let (state, token) = setup("web_online_history_test").await;
    let app = router(state.clone());

    // 两个会话经 write-behind 落进快照表。
    wait_count(&state, "SELECT count(*) FROM sys_user_online", 2).await;

    let resp = app
        .clone()
        .oneshot(get("/api/v1/monitor/online-sessions", &token))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let page = json_body(resp).await;
    assert_eq!(page["total"].as_u64().unwrap(), 2);
    assert!(
        page["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|i| i["end_time"].is_null() && i["status"].as_i64() == Some(0)),
        "刚登录的两条都应是在线：{page}"
    );

    // 大小写不敏感（分页与查询要的是 ILIKE，不是 `contains` 的 LIKE）：搜索框输入 `ADMIN`
    // 应能命中用户名 `admin`。
    let resp = app
        .clone()
        .oneshot(get(
            "/api/v1/monitor/online-sessions?user_name=ADMIN",
            &token,
        ))
        .await
        .unwrap();
    assert_eq!(
        json_body(resp).await["total"].as_u64().unwrap(),
        1,
        "模糊检索不该区分大小写"
    );

    // 用户输入的 `%` 是字面量而不是通配符：`?user_name=%` 不该匹配到任何会话。
    let resp = app
        .clone()
        .oneshot(get("/api/v1/monitor/online-sessions?user_name=%25", &token))
        .await
        .unwrap();
    assert_eq!(
        json_body(resp).await["total"].as_u64().unwrap(),
        0,
        "通配符必须被转义，否则一个 % 就把全表捞出来"
    );

    // 强退一条：Redis 少一条，快照那行改成已退出而不是消失。
    let resp = app
        .clone()
        .oneshot(del("/api/v1/monitor/online-users/sess-alice", &token))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    wait_count(
        &state,
        "SELECT count(*) FROM sys_user_online WHERE end_time IS NOT NULL",
        1,
    )
    .await;

    let resp = app
        .clone()
        .oneshot(get("/api/v1/monitor/online-sessions?status=1", &token))
        .await
        .unwrap();
    let page = json_body(resp).await;
    assert_eq!(page["total"].as_u64().unwrap(), 1);
    assert_eq!(
        page["items"][0]["session_id"].as_str().unwrap(),
        "sess-alice"
    );
    assert_eq!(page["items"][0]["user_name"].as_str().unwrap(), "alice");

    // 按用户名过滤 + 翻页形状（统一响应集合响应）。
    let resp = app
        .clone()
        .oneshot(get("/api/v1/monitor/online-sessions?user_name=adm", &token))
        .await
        .unwrap();
    let page = json_body(resp).await;
    assert_eq!(page["total"].as_u64().unwrap(), 1);
    assert_eq!(
        page["items"][0]["session_id"].as_str().unwrap(),
        "sess-admin"
    );
    for key in ["items", "total", "page", "page_size"] {
        assert!(page.get(key).is_some(), "缺字段 {key}：{page}");
    }

    // 对账：库里挂着「在线」但权威源已经没有的行，收成已退出（进程被杀 / TTL 过期）。
    state
        .db
        .conn()
        .execute_unprepared(
            "INSERT INTO sys_user_online (session_id, tenant_id, user_id, user_name, ip, \
             user_agent, login_time, last_access_time, status) \
             VALUES ('ghost', 1, 3, 'bob', '10.0.0.3', 'test', now(), now(), 0)",
        )
        .await
        .unwrap();
    let snapshot = cicada_auth::OnlineSnapshot::new(state.db.clone());
    let report = snapshot.reconcile_with(&*state.sessions).await.unwrap();
    assert_eq!(report.closed, 1, "只有 ghost 该行被收口：{report:?}");
    // alice 已被强退，权威源只剩 admin 一个会话，所以刷回 1 条。
    assert_eq!(
        report.refreshed, 1,
        "Redis 里剩几个会话就刷回几条：{report:?}"
    );
    assert_eq!(
        snapshot
            .open_session_ids()
            .await
            .unwrap()
            .into_iter()
            .collect::<Vec<_>>()
            .join(","),
        "sess-admin",
        "ghost 已退出，alice 被强退，只剩 admin 在线"
    );
}
