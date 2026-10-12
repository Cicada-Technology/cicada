//! M2 补齐能力集成测试（真实 PostgreSQL）：用户导入 / 登录锁定与解锁 / 文件上传下载 /
//! 10 万行导出内存稳定。

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
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

    let state = AppState {
        dept_directory: Arc::new(LocalDeptDirectory::new(db.clone())),
        config: Config::default(),
        db: db.clone(),
        tokens,
        sessions,
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
        authz: Arc::new(AuthzCache::new(Arc::new(InMemoryAuthzStore::new()), 60)),
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

fn bearer(method: &str, uri: &str, token: &str) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap()
}

/// 匿名请求：短时下载链接的意义就在这里——客户端不必（也没法）带 Authorization。
fn anonymous(method: &str, uri: &str) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .body(Body::empty())
        .unwrap()
}

/// 组装一个 multipart/form-data 请求体。
fn multipart(
    uri: &str,
    token: &str,
    filename: &str,
    content_type: &str,
    bytes: &[u8],
) -> Request<Body> {
    let boundary = "----cicadaBoundary7MA4YWxkTrZu0gW";
    let mut body = format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{filename}\"\r\nContent-Type: {content_type}\r\n\r\n"
    )
    .into_bytes();
    body.extend_from_slice(bytes);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    Request::builder()
        .method("POST")
        .uri(uri)
        .header("authorization", format!("Bearer {token}"))
        .header(
            "content-type",
            format!("multipart/form-data; boundary={boundary}"),
        )
        .body(Body::from(body))
        .unwrap()
}

async fn json_body(resp: axum::response::Response) -> serde_json::Value {
    let body = axum::body::to_bytes(resp.into_body(), 64 * 1024 * 1024)
        .await
        .unwrap();
    serde_json::from_slice(&body).unwrap()
}

fn rss_kb() -> u64 {
    let s = std::fs::read_to_string("/proc/self/statm").unwrap_or_default();
    let pages: u64 = s
        .split_whitespace()
        .nth(1)
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    pages * 4
}

/// 生成一个带表头 + 数据行的导入 xlsx。
fn build_import_xlsx(rows: &[[&str; 8]]) -> Vec<u8> {
    let mut wb = rust_xlsxwriter::Workbook::new();
    let ws = wb.add_worksheet();
    for (c, name) in [
        "登录名称",
        "用户昵称",
        "部门编号",
        "邮箱",
        "手机号码",
        "状态",
        "角色编号",
        "备注",
    ]
    .iter()
    .enumerate()
    {
        ws.write_string(0, c as u16, *name).unwrap();
    }
    for (r, row) in rows.iter().enumerate() {
        for (c, value) in row.iter().enumerate() {
            ws.write_string(r as u32 + 1, c as u16, *value).unwrap();
        }
    }
    wb.save_to_buffer().unwrap()
}

#[tokio::test]
#[ignore = "需要真实 PostgreSQL"]
async fn user_import_template_and_import() {
    let (state, token) = setup("web_m2_import_test").await;
    let app = router(state);

    // 模板下载
    let tpl = app
        .clone()
        .oneshot(bearer(
            "GET",
            "/api/v1/system/users/import-template",
            &token,
        ))
        .await
        .unwrap();
    assert_eq!(tpl.status(), StatusCode::OK);
    assert_eq!(
        tpl.headers().get(header::CONTENT_TYPE).unwrap(),
        "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"
    );

    // 导入：1 行成功 + 1 行缺昵称（fail_fast 下不落库）
    let xlsx = build_import_xlsx(&[
        [
            "imported",
            "导入用户",
            "",
            "imported@example.com",
            "13700000000",
            "正常",
            "",
            "导入测试",
        ],
        ["badrow", "", "", "", "", "正常", "", ""],
    ]);
    let resp = app
        .clone()
        .oneshot(multipart(
            "/api/v1/system/users/import",
            &token,
            "users.xlsx",
            "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
            &xlsx,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let report = json_body(resp).await;
    assert_eq!(report["total"], 2);
    assert_eq!(report["success"], 0, "fail_fast 校验失败不应落库：{report}");
    assert_eq!(report["failed"], 1);
    // 失败行定位到「行号 + 字段 + 原因」
    assert_eq!(report["errors"][0]["row"], 3);
    assert_eq!(report["errors"][0]["field"], "用户昵称");

    // partial 模式：合法行落库
    let resp = app
        .clone()
        .oneshot(multipart(
            "/api/v1/system/users/import?mode=partial",
            &token,
            "users.xlsx",
            "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
            &xlsx,
        ))
        .await
        .unwrap();
    let report = json_body(resp).await;
    assert_eq!(report["success"], 1, "{report}");

    let list = app
        .clone()
        .oneshot(bearer(
            "GET",
            "/api/v1/system/users?user_name=imported",
            &token,
        ))
        .await
        .unwrap();
    let list = json_body(list).await;
    assert_eq!(list["total"], 1, "{list}");
    assert_eq!(list["items"][0]["user_name"], "imported");
}

#[tokio::test]
#[ignore = "需要真实 PostgreSQL"]
async fn login_lockout_then_unlock() {
    let (state, token) = setup("web_m2_lockout_test").await;
    let app = router(state);

    let login = |pw: &str| {
        let body = serde_json::json!({ "username": "admin", "password": pw });
        Request::builder()
            .method("POST")
            .uri("/auth/login")
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap()
    };

    // 默认策略 max_retry = 5：前 4 次失败返回「账号或密码错误」，第 5 次即锁定。
    for i in 0..4 {
        let resp = app.clone().oneshot(login("wrong-password")).await.unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::UNPROCESSABLE_ENTITY,
            "第 {} 次失败应为 422",
            i + 1
        );
    }
    let trip = app.clone().oneshot(login("wrong-password")).await.unwrap();
    assert_eq!(trip.status(), StatusCode::LOCKED, "第 5 次失败应触发锁定");

    // 锁定期间即使密码正确也拒绝。
    let locked = app.clone().oneshot(login(ADMIN_PASSWORD)).await.unwrap();
    assert_eq!(locked.status(), StatusCode::LOCKED);

    // 管理员解锁
    let unlock = app
        .clone()
        .oneshot(bearer(
            "PUT",
            "/api/v1/monitor/login-logs/unlock/admin",
            &token,
        ))
        .await
        .unwrap();
    assert_eq!(unlock.status(), StatusCode::NO_CONTENT);

    // 解锁后正常登录
    let ok = app.clone().oneshot(login(ADMIN_PASSWORD)).await.unwrap();
    assert_eq!(ok.status(), StatusCode::OK, "解锁后应能登录");
}

#[tokio::test]
#[ignore = "需要真实 PostgreSQL"]
async fn file_upload_download_delete() {
    let (state, token) = setup("web_m2_file_test").await;
    let app = router(state);

    // 最小 PNG 签名 + 载荷 → magic number 判定为 image/png
    let mut png = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
    png.extend_from_slice(b"cicada-file-body");

    let up = app
        .clone()
        .oneshot(multipart(
            "/api/v1/system/files",
            &token,
            "头像.png",
            "image/png",
            &png,
        ))
        .await
        .unwrap();
    assert_eq!(up.status(), StatusCode::CREATED);
    let vo = json_body(up).await;
    let file_id = vo["file_id"].as_i64().unwrap();
    assert_eq!(vo["mime"], "image/png");
    assert_eq!(vo["size"], png.len() as i64);
    assert_eq!(vo["storage"], "memory");

    // 下载：字节一致，且不暴露物理路径
    let down = app
        .clone()
        .oneshot(bearer(
            "GET",
            &format!("/api/v1/system/files/{file_id}/content"),
            &token,
        ))
        .await
        .unwrap();
    assert_eq!(down.status(), StatusCode::OK);
    assert_eq!(
        down.headers().get(header::CONTENT_TYPE).unwrap(),
        "image/png"
    );
    let bytes = axum::body::to_bytes(down.into_body(), 1024 * 1024)
        .await
        .unwrap();
    assert_eq!(bytes.as_ref(), png.as_slice());

    // 文件存储的第二条下载路径：先鉴权换取短时链接，再用链接脱离鉴权取内容。
    // 本地/内存驱动给不出预签名 URL，因此这条走一次性 token（kind 说明来源）。
    let issued = app
        .clone()
        .oneshot(bearer(
            "POST",
            &format!("/api/v1/system/files/{file_id}/download-url"),
            &token,
        ))
        .await
        .unwrap();
    assert_eq!(issued.status(), StatusCode::OK);
    let body = json_body(issued).await;
    assert_eq!(body["kind"], "one_time_token", "{body}");
    assert!(body["expires_in"].as_u64().unwrap() > 0);
    let url = body["url"].as_str().unwrap().to_string();
    assert!(url.starts_with("/api/v1/files/dl/"), "{url}");
    // 链接里只有那 32 位 token：路径、桶名、原始名都不许出现（文件存储「禁止暴露物理路径」）。
    assert_eq!(
        url.len(),
        "/api/v1/files/dl/".len() + 32,
        "链接除了 token 不该带别的东西：{url}"
    );
    assert!(!url.contains(".png"), "链接里不该有文件名：{url}");

    // 不带 Authorization：凭证本身就是授权，这正是它能放进 <img src> 的原因。
    let via_token = app.clone().oneshot(anonymous("GET", &url)).await.unwrap();
    assert_eq!(via_token.status(), StatusCode::OK);
    let served = axum::body::to_bytes(via_token.into_body(), 1024 * 1024)
        .await
        .unwrap();
    assert_eq!(served.as_ref(), png.as_slice());

    // 一次性：同一个 token 再用一次就该 404（否则短时链接退化成永久口令）。
    let reused = app.clone().oneshot(anonymous("GET", &url)).await.unwrap();
    assert_eq!(reused.status(), StatusCode::NOT_FOUND);
    // 未知 token 同样 404，不区分「不存在」与「已用过」。
    let bogus = app
        .clone()
        .oneshot(anonymous(
            "GET",
            "/api/v1/files/dl/00000000000000000000000000000000",
        ))
        .await
        .unwrap();
    assert_eq!(bogus.status(), StatusCode::NOT_FOUND);
    // 未鉴权地问签发接口：仍然 401——签发这一步不许绕过权限与数据范围。
    let anonymous_issue = app
        .clone()
        .oneshot(anonymous(
            "POST",
            &format!("/api/v1/system/files/{file_id}/download-url"),
        ))
        .await
        .unwrap();
    assert_eq!(anonymous_issue.status(), StatusCode::UNAUTHORIZED);

    // 软删后不可再取
    let del = app
        .clone()
        .oneshot(bearer(
            "DELETE",
            &format!("/api/v1/system/files/{file_id}"),
            &token,
        ))
        .await
        .unwrap();
    assert_eq!(del.status(), StatusCode::NO_CONTENT);
    let gone = app
        .clone()
        .oneshot(bearer(
            "GET",
            &format!("/api/v1/system/files/{file_id}"),
            &token,
        ))
        .await
        .unwrap();
    assert_eq!(gone.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
#[ignore = "需要真实 PostgreSQL；插入 10 万行较慢"]
async fn export_100k_rows_memory_stable() {
    let (state, token) = setup("web_m2_export_test").await;
    let db = state.db.clone();
    // 直接一条 SQL 灌 10 万行，避免逐行插入的测试开销。
    db.conn()
        .execute_unprepared(
            "INSERT INTO sys_user (tenant_id, user_name, nick_name, password, status, version, create_time) \
             SELECT 1, 'bulk_' || g, 'Bulk ' || g, 'x', 0, 0, now() FROM generate_series(1, 100000) g",
        )
        .await
        .unwrap();

    let app = router(state);
    let before = rss_kb();
    let resp = app
        .clone()
        .oneshot(bearer("GET", "/api/v1/system/users/export", &token))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), 128 * 1024 * 1024)
        .await
        .unwrap();
    let after = rss_kb();

    // 结果正确：表头 + 10 万行（导出上限）。
    let mut wb: calamine::Xlsx<std::io::Cursor<&[u8]>> =
        calamine::Reader::new(std::io::Cursor::new(bytes.as_ref())).unwrap();
    use calamine::Reader as _;
    let range = wb.worksheet_range("用户").unwrap();
    assert_eq!(range.rows().count(), 100_001, "表头 + 10 万数据行");

    // 内存稳定：constant_memory 逐行刷盘，RSS 增量远小于行数级增长。
    let growth_mb = (after.saturating_sub(before)) / 1024;
    assert!(growth_mb < 200, "导出 10 万行 RSS 增长过大：{growth_mb}MB");
}

/// 把错误响应拆成 `(status, problem 体)`，并校验统一响应的硬约束：
/// 内容类型必须是 `application/problem+json`，`status` 与 HTTP 状态一致。
async fn problem_body(resp: axum::response::Response) -> (StatusCode, serde_json::Value) {
    let content_type = resp
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let status = resp.status();
    let body = json_body(resp).await;
    assert_eq!(content_type, "application/problem+json", "{body}");
    assert_eq!(body["status"], status.as_u16(), "{body}");
    (status, body)
}

/// 统一响应 / 业务码目录：错误体统一为 RFC 9457 problem+json，并带业务码。
#[tokio::test]
#[ignore = "需要真实 PostgreSQL"]
async fn error_bodies_are_rfc9457_problem_json() {
    let (state, token) = setup("web_m2_problem_contract").await;
    let app = router(state);

    // 401 未认证 → CICADA_AUTH_1001
    let anon = Request::builder()
        .uri("/auth/profile")
        .body(Body::empty())
        .unwrap();
    let (status, body) = problem_body(app.clone().oneshot(anon).await.unwrap()).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
    assert_eq!(body["code"], "CICADA_AUTH_1001");
    assert_eq!(body["type"], "https://cicada.dev/errors/auth/1001");
    assert_eq!(body["title"], "未认证");

    // 422 账号或密码错误 → CICADA_AUTH_1002；无字段级错误时 `errors` 不出现。
    let wrong = Request::builder()
        .method("POST")
        .uri("/auth/login")
        .header("content-type", "application/json")
        .body(Body::from(
            serde_json::json!({ "username": "admin", "password": "nope" }).to_string(),
        ))
        .unwrap();
    let (status, body) = problem_body(app.clone().oneshot(wrong).await.unwrap()).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["code"], "CICADA_AUTH_1002");
    assert_eq!(body["detail"], "账号或密码错误");
    assert!(body.get("errors").is_none(), "{body}");

    // 422 字段级校验 → CICADA_VALIDATION_4001 + errors[]（前端表单贴回字段）
    let bad_key = Request::builder()
        .method("POST")
        .uri("/api/v1/system/api-keys")
        .header("authorization", format!("Bearer {token}"))
        .header("content-type", "application/json")
        .body(Body::from(
            serde_json::json!({ "name": "  ", "scopes": [], "expires_at": null }).to_string(),
        ))
        .unwrap();
    let (status, body) = problem_body(app.clone().oneshot(bad_key).await.unwrap()).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["code"], "CICADA_VALIDATION_4001");
    assert_eq!(body["errors"][0]["field"], "name", "{body}");
    assert!(
        body["errors"][0]["message"]
            .as_str()
            .unwrap()
            .contains("凭证名称"),
        "{body}"
    );

    // 422 提取器拒绝（缺必填字段）默认是 text/plain，兜底层必须改写成 problem+json。
    let malformed = Request::builder()
        .method("POST")
        .uri("/api/v1/system/api-keys")
        .header("authorization", format!("Bearer {token}"))
        .header("content-type", "application/json")
        .body(Body::from(r#"{"name":"only-name"}"#))
        .unwrap();
    let (status, body) = problem_body(app.clone().oneshot(malformed).await.unwrap()).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["code"], "CICADA_VALIDATION_4001");
    assert_eq!(body["instance"], "/api/v1/system/api-keys");

    // 404 未匹配路由由 router 的 fallback 给 problem+json（不是 axum 默认空体）。
    let nowhere = Request::builder()
        .uri("/api/v1/does-not-exist")
        .header("accept-language", "en-US")
        .body(Body::empty())
        .unwrap();
    let (status, body) = problem_body(app.oneshot(nowhere).await.unwrap()).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(body["code"], "CICADA_SYS_3001");
    assert_eq!(body["instance"], "/api/v1/does-not-exist");
    assert_eq!(body["detail"], "The requested resource does not exist");
}
