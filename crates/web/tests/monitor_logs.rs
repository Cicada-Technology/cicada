//! 操作日志 / 登录日志接口集成测试（真实 PostgreSQL）。
//!
//! 覆盖  的完整落库路径：审计 sink 只写 `outbox_event` → relay 投递 →
//! `system` 的处理器写 `sys_oper_log` / `sys_logininfor`，再经
//! `monitor:operlog:*` / `monitor:logininfor:*` 读回、详情、导出（xlsx）、删除与清空，
//! 以及权限守卫（无权限 403）。

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use sea_orm::{ConnectOptions, ConnectionTrait, Database, DatabaseConnection};
use sea_orm_migration::MigratorTraitSelf;
use tower::ServiceExt;

use cicada_audit::{Outbox, OutboxAuditSink};
use cicada_auth::{InMemorySessionStore, Session, SessionStore, TokenService, hash_refresh};
use cicada_infra::Db;
use cicada_infra::config::{Config, DbConfig, Secret};
use cicada_ports::EventRouter;
use cicada_rbac::{AuthzCache, InMemoryAuthzStore};
use cicada_system::{LocalDeptDirectory, SysLogStore};
use cicada_web::router;
use cicada_web::state::AppState;

const JWT_SECRET: &str = "test-secret";

/// 测试里的 relay：真实进程由 `bootstrap` 起后台任务循环调用同一组动作。
struct Relay {
    outbox: Outbox,
    events: Arc<EventRouter>,
}

impl Relay {
    async fn drain(&self) -> cicada_audit::DrainReport {
        self.outbox.drain_once(&self.events, 200, 5).await.unwrap()
    }
}

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

async fn setup(schema: &str) -> (AppState, String, Relay) {
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

    // 发件箱（生产者）+ relay 的路由（消费者是 system 的落库处理器）。
    let outbox = Outbox::new(db.clone());
    let events = Arc::new(EventRouter::new());
    events.register(Arc::new(SysLogStore::new(db.clone())));

    let state = AppState {
        dept_directory: Arc::new(LocalDeptDirectory::new(db.clone())),
        config: Config::default(),
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
        audit: Arc::new(OutboxAuditSink::from_outbox(outbox.clone())),
        db: db.clone(),
        tokens,
        sessions,
        authz: Arc::new(AuthzCache::new(Arc::new(InMemoryAuthzStore::new()), 60)),
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
    (state, token, Relay { outbox, events })
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
async fn operation_and_login_logs_are_queryable() {
    let (state, admin_token, relay) = setup("web_monitor_logs_test").await;
    let app = router(state);

    // 触发一次被审计的写操作 → 落 sys_oper_log。
    let created = app
        .clone()
        .oneshot(req(
            "POST",
            "/api/v1/system/roles",
            &admin_token,
            Some(serde_json::json!({ "role_name": "审计样例", "role_key": "audit_sample" })),
        ))
        .await
        .unwrap();
    assert_eq!(created.status(), StatusCode::CREATED);

    // 审计设计：审计此刻只在发件箱里，`sys_oper_log` 还没有被 `audit` 越过属主写过。
    assert!(
        relay.outbox.pending_count().await.unwrap() >= 1,
        "写操作后发件箱应有待发事件"
    );
    let before = app
        .clone()
        .oneshot(req(
            "GET",
            "/api/v1/monitor/oper-logs?title=system:role:add",
            &admin_token,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(json_body(before).await["total"].as_u64().unwrap(), 0);

    // relay 投递后事件才落到属主表。
    let report = relay.drain().await;
    assert!(report.delivered >= 1, "{report:?}");
    assert_eq!(report.failed, 0);
    assert_eq!(relay.outbox.pending_count().await.unwrap(), 0);

    // ---- 操作日志列表：能看到 system:role:add ----
    let list = app
        .clone()
        .oneshot(req(
            "GET",
            "/api/v1/monitor/oper-logs?title=system:role:add",
            &admin_token,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(list.status(), StatusCode::OK);
    let page = json_body(list).await;
    assert!(page["total"].as_u64().unwrap() >= 1, "{page}");
    let oper_id = page["items"][0]["oper_id"].as_i64().unwrap();

    // ---- 详情 ----
    let detail = app
        .clone()
        .oneshot(req(
            "GET",
            &format!("/api/v1/monitor/oper-logs/{oper_id}"),
            &admin_token,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(detail.status(), StatusCode::OK);
    assert_eq!(json_body(detail).await["title"], "system:role:add");

    // ---- 导出 ----
    let export = app
        .clone()
        .oneshot(req(
            "GET",
            "/api/v1/monitor/oper-logs/export",
            &admin_token,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(export.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(export.into_body(), 8 * 1024 * 1024)
        .await
        .unwrap();
    assert_eq!(&bytes[..2], b"PK", "操作日志导出应为 xlsx");

    // ---- 删除单条 ----
    let removed = app
        .clone()
        .oneshot(req(
            "DELETE",
            &format!("/api/v1/monitor/oper-logs/{oper_id}"),
            &admin_token,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(removed.status(), StatusCode::NO_CONTENT);
    let gone = app
        .clone()
        .oneshot(req(
            "GET",
            &format!("/api/v1/monitor/oper-logs/{oper_id}"),
            &admin_token,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(gone.status(), StatusCode::NOT_FOUND);

    // ---- 登录日志：登录一次后可见 ----
    let login = app
        .clone()
        .oneshot(req(
            "POST",
            "/auth/login",
            "",
            Some(serde_json::json!({ "username": "admin", "password": "Cicada@2026" })),
        ))
        .await
        .unwrap();
    assert_eq!(login.status(), StatusCode::OK);
    let _ = json_body(login).await;
    relay.drain().await;

    let login_logs = app
        .clone()
        .oneshot(req(
            "GET",
            "/api/v1/monitor/login-logs?user_name=admin",
            &admin_token,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(login_logs.status(), StatusCode::OK);
    let page = json_body(login_logs).await;
    assert!(page["total"].as_u64().unwrap() >= 1, "{page}");
    assert_eq!(page["items"][0]["status"], 0, "成功登录 status=0");

    let export = app
        .clone()
        .oneshot(req(
            "GET",
            "/api/v1/monitor/login-logs/export",
            &admin_token,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(export.status(), StatusCode::OK);

    // ---- 清空登录日志 ----
    let clean = app
        .clone()
        .oneshot(req(
            "DELETE",
            "/api/v1/monitor/login-logs",
            &admin_token,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(clean.status(), StatusCode::NO_CONTENT);
    let empty = app
        .clone()
        .oneshot(req("GET", "/api/v1/monitor/login-logs", &admin_token, None))
        .await
        .unwrap();
    assert_eq!(json_body(empty).await["total"].as_u64().unwrap(), 0);

    // ---- 权限守卫：只有 system:user:list 的用户看不了日志 ----
    let role = app
        .clone()
        .oneshot(req(
            "POST",
            "/api/v1/system/roles",
            &admin_token,
            Some(serde_json::json!({
                "role_name": "只读", "role_key": "only_list", "menu_ids": [1, 100, 1000]
            })),
        ))
        .await
        .unwrap();
    let role_id = json_body(role).await["role_id"].as_i64().unwrap();
    let user = app
        .clone()
        .oneshot(req(
            "POST",
            "/api/v1/system/users",
            &admin_token,
            Some(serde_json::json!({
                "user_name": "viewer", "nick_name": "Viewer", "password": "Viewer@123",
                "role_ids": [role_id]
            })),
        ))
        .await
        .unwrap();
    assert_eq!(user.status(), StatusCode::CREATED);
    let login = app
        .clone()
        .oneshot(req(
            "POST",
            "/auth/login",
            "",
            Some(serde_json::json!({ "username": "viewer", "password": "Viewer@123" })),
        ))
        .await
        .unwrap();
    let viewer_token = json_body(login).await["access_token"]
        .as_str()
        .unwrap()
        .to_string();

    for path in ["/api/v1/monitor/oper-logs", "/api/v1/monitor/login-logs"] {
        let denied = app
            .clone()
            .oneshot(req("GET", path, &viewer_token, None))
            .await
            .unwrap();
        assert_eq!(denied.status(), StatusCode::FORBIDDEN, "{path} 应 403");
    }
}

/// relay 的重试与放弃：没有处理器的事件不会被静默丢弃，也不会永久占住扫描窗口。
#[tokio::test]
#[ignore = "需要真实 PostgreSQL"]
async fn poison_events_retry_then_are_abandoned() {
    let (_state, _token, relay) = setup("web_monitor_outbox_test").await;
    relay
        .outbox
        .enqueue(
            1,
            cicada_ports::AGGREGATE_AUDIT,
            Some("nobody".to_string()),
            "audit.not_registered",
            serde_json::json!({ "hello": "world" }),
        )
        .await
        .unwrap();
    assert_eq!(relay.outbox.pending_count().await.unwrap(), 1);

    for _ in 0..5 {
        let report = relay.outbox.drain_once(&relay.events, 10, 5).await.unwrap();
        assert_eq!(
            (report.delivered, report.failed, report.abandoned),
            (0, 1, 0)
        );
        assert_eq!(
            relay.outbox.pending_count().await.unwrap(),
            1,
            "未达上限前应留在发件箱"
        );
    }

    let last = relay.outbox.drain_once(&relay.events, 10, 5).await.unwrap();
    assert_eq!((last.delivered, last.failed, last.abandoned), (0, 1, 1));
    assert_eq!(
        relay.outbox.pending_count().await.unwrap(),
        0,
        "超过 max_attempts 后盖章离场"
    );
}

/// 审计设计（P1）字段级变更留痕：更新接口产生的差异经发件箱落到 `sys_audit_log`，
/// 只含变化列，且只能经 `monitor:auditlog:list` 读回。
#[tokio::test]
#[ignore = "需要真实 PostgreSQL"]
async fn field_level_changes_land_in_sys_audit_log() {
    let (state, admin_token, relay) = setup("web_audit_log_test").await;
    let app = router(state);

    let created = app
        .clone()
        .oneshot(req(
            "POST",
            "/api/v1/system/roles",
            &admin_token,
            Some(serde_json::json!({
                "role_name": "留痕前", "role_key": "audit_track", "role_sort": 3
            })),
        ))
        .await
        .unwrap();
    assert_eq!(created.status(), StatusCode::CREATED);
    let role_id = json_body(created).await["role_id"].as_i64().unwrap();

    let updated = app
        .clone()
        .oneshot(req(
            "PUT",
            &format!("/api/v1/system/roles/{role_id}"),
            &admin_token,
            Some(serde_json::json!({
                "role_name": "留痕后", "role_key": "audit_track", "status": 1
            })),
        ))
        .await
        .unwrap();
    assert_eq!(updated.status(), StatusCode::OK);
    let _ = json_body(updated).await;

    let report = relay.drain().await;
    assert!(report.delivered >= 1, "{report:?}");

    let list = app
        .clone()
        .oneshot(req(
            "GET",
            &format!("/api/v1/monitor/audit-logs?table_name=sys_role&record_id={role_id}"),
            &admin_token,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(list.status(), StatusCode::OK);
    let page = json_body(list).await;
    assert_eq!(
        page["total"].as_u64(),
        Some(1),
        "只有这次更新该留痕：{page}"
    );
    let row = &page["items"][0];
    assert_eq!(row["action"], "update");
    assert_eq!(row["table_name"], "sys_role");
    assert_eq!(
        row["record_id"].as_str(),
        Some(role_id.to_string().as_str())
    );
    assert_eq!(
        row["changed"]["role_name"],
        serde_json::json!(["留痕前", "留痕后"])
    );
    assert_eq!(row["changed"]["status"], serde_json::json!([0, 1]));
    assert!(
        row["changed"].get("role_key").is_none(),
        "未变化的列不进留痕：{}",
        row["changed"]
    );
    for key in ["password", "secret", "token"] {
        assert!(
            row["changed"].get(key).is_none(),
            "凭据类字段绝不能出现在留痕里"
        );
    }
    assert!(row["trace_id"].is_string(), "留痕要能关联链路：{row}");

    // 按实体查历史：不存在的那条记录不该返回任何行。
    let none = app
        .oneshot(req(
            "GET",
            "/api/v1/monitor/audit-logs?table_name=sys_role&record_id=999999",
            &admin_token,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(json_body(none).await["total"].as_u64(), Some(0));
}

/// 审计日志的三项：UA、脱敏后的请求体、业务码。
///
/// 请求体里塞一个 `password` 字段——`RoleUpsert` 不认识它（serde 忽略未知字段），
/// 所以业务上无害，但它正好落在脱敏清单里：值必须变成 `***`，
/// 且原文一点都不能留在库里。这才是「脱敏后的请求参数」的实际验收。
#[tokio::test]
#[ignore = "需要真实 PostgreSQL"]
async fn audit_records_ua_masked_body_and_biz_code() {
    let (state, admin_token, relay) = setup("web_audit_context_test").await;
    let app = router(state);
    let ua = "Mozilla/5.0 (X11; Linux x86_64) cicada-audit-test";

    // 会话指纹会比对 (ip, UA)：换 UA 的请求要被当成未认证。所以要先用这个 UA
    // 重新登录一次，拿到的 token 才允许带着它写数据——这本身也是「UA 被记进审计」
    // 的前提：审计记的是这次请求真的用到的那个 UA。
    let login = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/auth/login")
                .header("content-type", "application/json")
                .header("user-agent", ua)
                .body(Body::from(
                    serde_json::json!({ "username": "admin", "password": "Cicada@2026" })
                        .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(login.status(), StatusCode::OK, "带 UA 登录必须成功");
    let ua_token = json_body(login).await["access_token"]
        .as_str()
        .unwrap()
        .to_string();

    // 请求体**分块且不声明 Content-Length**（等价于 HTTP/1.1 的 chunked 上传）：
    // 这类请求正是「先把体读干再放回去」采不到的那种——没有长度就没法保证读得回去。
    // 记录器改成与原流共用一个流之后，参数照样采得到，而 handler 收到的字节一块不少
    // （下面用返回体里的 role_name 验证重组没有丢字节）。
    let build = |body: serde_json::Value| {
        let text = body.to_string();
        let (head, tail) = text.split_at(text.len() / 2);
        Request::builder()
            .method("POST")
            .uri("/api/v1/system/roles")
            .header("authorization", format!("Bearer {ua_token}"))
            .header("content-type", "application/json")
            .header("user-agent", ua)
            .body(Body::from_stream(futures_util::stream::iter(vec![
                Ok::<_, std::io::Error>(axum::body::Bytes::from(head.to_string())),
                Ok(axum::body::Bytes::from(tail.to_string())),
            ])))
            .unwrap()
    };
    let first = app
        .clone()
        .oneshot(build(serde_json::json!({
            "role_name": "审计上下文", "role_key": "audit_ctx",
            "password": "s3cr3t-must-not-be-stored",
        })))
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::CREATED);
    assert_eq!(
        json_body(first).await["role_name"].as_str(),
        Some("审计上下文"),
        "分块请求体必须在 handler 侧原样重组"
    );

    // 同一个 role_key 再来一次：唯一约束冲突 → 409 + CICADA_SYS_3002。
    let second = app
        .clone()
        .oneshot(build(serde_json::json!({
            "role_name": "重复键", "role_key": "audit_ctx",
        })))
        .await
        .unwrap();
    assert_eq!(second.status(), StatusCode::CONFLICT);

    let report = relay.drain().await;
    assert!(report.delivered >= 2, "{report:?}");

    let list = app
        .clone()
        .oneshot(req(
            "GET",
            "/api/v1/monitor/oper-logs?title=system:role:add",
            &admin_token,
            None,
        ))
        .await
        .unwrap();
    let page = json_body(list).await;
    let items = page["items"].as_array().unwrap();
    assert!(items.len() >= 2, "两次写都该留痕：{page}");

    // UA：两条都带。
    assert!(
        items.iter().all(|i| i["user_agent"].as_str() == Some(ua)),
        "user_agent 没进审计：{page}"
    );
    // 业务码：失败那条带 3002，成功那条不带（业务码只描述这次失败）。
    let failed = items
        .iter()
        .find(|i| i["status"].as_i64() == Some(1))
        .expect("失败的一次写要记成 status=1");
    assert_eq!(
        failed["biz_code"].as_str(),
        Some("CICADA_SYS_3002"),
        "{failed}"
    );
    let ok = items
        .iter()
        .find(|i| i["status"].as_i64() == Some(0))
        .expect("成功的一次写要记成 status=0");
    assert!(ok["biz_code"].is_null(), "{ok}");

    // 请求体：JSON 原文可查，但凭据类字段只剩键名。按成功那条取——两次请求都带
    // `audit_ctx`（第二次正是因为它重复而失败），只有成功那条才带口令字段。
    let body = ok["req_body"].as_str().unwrap_or_else(|| panic!("{ok}"));
    assert!(body.contains("role_key"), "{body}");
    assert!(
        body.contains("\"password\":\"***\""),
        "password 必须被脱敏：{body}"
    );
    let raw = serde_json::to_string(&page).unwrap();
    assert!(
        !raw.contains("s3cr3t-must-not-be-stored"),
        "口令原文漏进了审计：{raw}"
    );
}
