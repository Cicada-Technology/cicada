//! M2 基础域集成测试（真实 PostgreSQL + 内存缓存）：部门/岗位/字典/参数/公告 CRUD。
//!
//! 覆盖树的 `ancestors` 维护与防环、唯一键冲突、内置参数保护、字典类型删除的
//! 悬挂数据校验、公告已读幂等。

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
async fn dept_post_dict_config_notice_crud() {
    let (state, admin_token) = setup("web_system_base_test").await;
    let app = router(state);

    // ================= 部门 =================
    let root = app
        .clone()
        .oneshot(req(
            "POST",
            "/api/v1/system/depts",
            &admin_token,
            Some(serde_json::json!({ "parent_id": 0, "dept_name": "总公司" })),
        ))
        .await
        .unwrap();
    assert_eq!(root.status(), StatusCode::CREATED);
    let root = json_body(root).await;
    let root_id = root["dept_id"].as_i64().unwrap();
    assert_eq!(root["ancestors"], "0");

    let child = app
        .clone()
        .oneshot(req(
            "POST",
            "/api/v1/system/depts",
            &admin_token,
            Some(serde_json::json!({ "parent_id": root_id, "dept_name": "研发部" })),
        ))
        .await
        .unwrap();
    let child = json_body(child).await;
    let child_id = child["dept_id"].as_i64().unwrap();
    assert_eq!(child["ancestors"], format!("0,{root_id}"));

    let list = app
        .clone()
        .oneshot(req("GET", "/api/v1/system/depts", &admin_token, None))
        .await
        .unwrap();
    assert_eq!(list.status(), StatusCode::OK);
    let tree = json_body(list).await;
    let root_node = tree
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["dept_id"].as_i64() == Some(root_id))
        .unwrap();
    assert_eq!(root_node["children"].as_array().unwrap().len(), 1);

    // 把父节点挂到自己的下级下 → 422（防环）。
    let cycle = app
        .clone()
        .oneshot(req(
            "PUT",
            &format!("/api/v1/system/depts/{root_id}"),
            &admin_token,
            Some(serde_json::json!({ "parent_id": child_id, "dept_name": "总公司" })),
        ))
        .await
        .unwrap();
    assert_eq!(cycle.status(), StatusCode::UNPROCESSABLE_ENTITY);

    // 存在下级部门时删除 → 409。
    let del_root = app
        .clone()
        .oneshot(req(
            "DELETE",
            &format!("/api/v1/system/depts/{root_id}"),
            &admin_token,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(del_root.status(), StatusCode::CONFLICT);

    let del_child = app
        .clone()
        .oneshot(req(
            "DELETE",
            &format!("/api/v1/system/depts/{child_id}"),
            &admin_token,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(del_child.status(), StatusCode::NO_CONTENT);
    let del_root = app
        .clone()
        .oneshot(req(
            "DELETE",
            &format!("/api/v1/system/depts/{root_id}"),
            &admin_token,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(del_root.status(), StatusCode::NO_CONTENT);

    // ================= 岗位 =================
    let post = app
        .clone()
        .oneshot(req(
            "POST",
            "/api/v1/system/posts",
            &admin_token,
            Some(serde_json::json!({ "post_code": "dev", "post_name": "开发", "post_sort": 1 })),
        ))
        .await
        .unwrap();
    assert_eq!(post.status(), StatusCode::CREATED);
    let post_id = json_body(post).await["post_id"].as_i64().unwrap();

    let dup = app
        .clone()
        .oneshot(req(
            "POST",
            "/api/v1/system/posts",
            &admin_token,
            Some(serde_json::json!({ "post_code": "dev", "post_name": "重复" })),
        ))
        .await
        .unwrap();
    assert_eq!(dup.status(), StatusCode::CONFLICT);

    let list = app
        .clone()
        .oneshot(req(
            "GET",
            "/api/v1/system/posts?post_code=dev",
            &admin_token,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(json_body(list).await["total"].as_u64().unwrap(), 1);

    let upd = app
        .clone()
        .oneshot(req(
            "PUT",
            &format!("/api/v1/system/posts/{post_id}"),
            &admin_token,
            Some(serde_json::json!({ "post_code": "dev", "post_name": "研发工程师", "post_sort": 2 })),
        ))
        .await
        .unwrap();
    assert_eq!(upd.status(), StatusCode::OK);
    assert_eq!(json_body(upd).await["post_name"], "研发工程师");

    let del = app
        .clone()
        .oneshot(req(
            "DELETE",
            &format!("/api/v1/system/posts/{post_id}"),
            &admin_token,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(del.status(), StatusCode::NO_CONTENT);

    // ================= 字典 =================
    let dict_type = app
        .clone()
        .oneshot(req(
            "POST",
            "/api/v1/system/dict/types",
            &admin_token,
            Some(serde_json::json!({ "dict_name": "订单状态", "dict_type": "biz_order_status" })),
        ))
        .await
        .unwrap();
    assert_eq!(dict_type.status(), StatusCode::CREATED);
    let dict_id = json_body(dict_type).await["dict_id"].as_i64().unwrap();

    let dup_type = app
        .clone()
        .oneshot(req(
            "POST",
            "/api/v1/system/dict/types",
            &admin_token,
            Some(serde_json::json!({ "dict_name": "x", "dict_type": "biz_order_status" })),
        ))
        .await
        .unwrap();
    assert_eq!(dup_type.status(), StatusCode::CONFLICT);

    let data = app
        .clone()
        .oneshot(req(
            "POST",
            "/api/v1/system/dict/data",
            &admin_token,
            Some(serde_json::json!({
                "dict_label": "已支付", "dict_value": "2", "dict_type": "biz_order_status", "dict_sort": 1
            })),
        ))
        .await
        .unwrap();
    assert_eq!(data.status(), StatusCode::CREATED);
    let dict_code = json_body(data).await["dict_code"].as_i64().unwrap();

    // 类型下有数据 → 409。
    let del_type = app
        .clone()
        .oneshot(req(
            "DELETE",
            &format!("/api/v1/system/dict/types/{dict_id}"),
            &admin_token,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(del_type.status(), StatusCode::CONFLICT);

    let list_data = app
        .clone()
        .oneshot(req(
            "GET",
            "/api/v1/system/dict/data?dict_type=biz_order_status",
            &admin_token,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(json_body(list_data).await["total"].as_u64().unwrap(), 1);

    let del_data = app
        .clone()
        .oneshot(req(
            "DELETE",
            &format!("/api/v1/system/dict/data/{dict_code}"),
            &admin_token,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(del_data.status(), StatusCode::NO_CONTENT);

    // ---- 按类型取启用项：登录即可读（不要求 system:dict:list），且只出启用项 ----
    let enabled = app
        .clone()
        .oneshot(req(
            "POST",
            "/api/v1/system/dict/data",
            &admin_token,
            Some(serde_json::json!({
                "dict_label": "待支付", "dict_value": "1",
                "dict_type": "biz_order_status", "dict_sort": 1
            })),
        ))
        .await
        .unwrap();
    assert_eq!(enabled.status(), StatusCode::CREATED);
    let enabled_code = json_body(enabled).await["dict_code"].as_i64().unwrap();

    let disabled = app
        .clone()
        .oneshot(req(
            "POST",
            "/api/v1/system/dict/data",
            &admin_token,
            Some(serde_json::json!({
                "dict_label": "已关闭", "dict_value": "9",
                "dict_type": "biz_order_status", "dict_sort": 2, "status": 1
            })),
        ))
        .await
        .unwrap();
    assert_eq!(disabled.status(), StatusCode::CREATED);
    let disabled_code = json_body(disabled).await["dict_code"].as_i64().unwrap();

    let enabled_only = app
        .clone()
        .oneshot(req(
            "GET",
            "/api/v1/system/dict/data/by-type/biz_order_status",
            &admin_token,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(enabled_only.status(), StatusCode::OK);
    let rows = json_body(enabled_only).await;
    let rows = rows.as_array().unwrap();
    assert_eq!(rows.len(), 1, "停用项不应出现在下拉框数据里：{rows:?}");
    assert_eq!(rows[0]["dict_value"].as_str().unwrap(), "1");

    // 未知类型 → 空数组（而不是 404）：下拉框据此渲染空选项。
    let unknown = app
        .clone()
        .oneshot(req(
            "GET",
            "/api/v1/system/dict/data/by-type/not_a_real_type",
            &admin_token,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(unknown.status(), StatusCode::OK);
    assert!(json_body(unknown).await.as_array().unwrap().is_empty());

    let del_disabled = app
        .clone()
        .oneshot(req(
            "DELETE",
            &format!("/api/v1/system/dict/data/{disabled_code}"),
            &admin_token,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(del_disabled.status(), StatusCode::NO_CONTENT);

    let del_enabled = app
        .clone()
        .oneshot(req(
            "DELETE",
            &format!("/api/v1/system/dict/data/{enabled_code}"),
            &admin_token,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(del_enabled.status(), StatusCode::NO_CONTENT);

    let del_type = app
        .clone()
        .oneshot(req(
            "DELETE",
            &format!("/api/v1/system/dict/types/{dict_id}"),
            &admin_token,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(del_type.status(), StatusCode::NO_CONTENT);

    // ================= 参数 =================
    let config = app
        .clone()
        .oneshot(req(
            "POST",
            "/api/v1/system/configs",
            &admin_token,
            Some(serde_json::json!({
                "config_name": "业务开关", "config_key": "biz.feature.enabled", "config_value": "true"
            })),
        ))
        .await
        .unwrap();
    assert_eq!(config.status(), StatusCode::CREATED);
    let config_id = json_body(config).await["config_id"].as_i64().unwrap();

    let upd = app
        .clone()
        .oneshot(req(
            "PUT",
            &format!("/api/v1/system/configs/{config_id}"),
            &admin_token,
            Some(serde_json::json!({
                "config_name": "业务开关", "config_key": "biz.feature.enabled", "config_value": "false"
            })),
        ))
        .await
        .unwrap();
    assert_eq!(upd.status(), StatusCode::OK);
    assert_eq!(json_body(upd).await["config_value"], "false");

    // 种子内置参数（config_type = 'Y'，config_id = 1）不可删除。
    let del_builtin = app
        .clone()
        .oneshot(req(
            "DELETE",
            "/api/v1/system/configs/1",
            &admin_token,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(del_builtin.status(), StatusCode::FORBIDDEN);

    let del = app
        .clone()
        .oneshot(req(
            "DELETE",
            &format!("/api/v1/system/configs/{config_id}"),
            &admin_token,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(del.status(), StatusCode::NO_CONTENT);

    // ================= 公告 =================
    let notice = app
        .clone()
        .oneshot(req(
            "POST",
            "/api/v1/system/notices",
            &admin_token,
            Some(serde_json::json!({
                "notice_title": "系统维护通知", "notice_type": 1, "notice_content": "今晚 23:00 停机"
            })),
        ))
        .await
        .unwrap();
    assert_eq!(notice.status(), StatusCode::CREATED);
    let notice_id = json_body(notice).await["notice_id"].as_i64().unwrap();

    let list = app
        .clone()
        .oneshot(req("GET", "/api/v1/system/notices", &admin_token, None))
        .await
        .unwrap();
    assert!(json_body(list).await["total"].as_u64().unwrap() >= 1);

    // 已读标记幂等：两次都 204。
    for _ in 0..2 {
        let read = app
            .clone()
            .oneshot(req(
                "POST",
                &format!("/api/v1/system/notices/{notice_id}/read"),
                &admin_token,
                None,
            ))
            .await
            .unwrap();
        assert_eq!(read.status(), StatusCode::NO_CONTENT);
    }

    let del = app
        .clone()
        .oneshot(req(
            "DELETE",
            &format!("/api/v1/system/notices/{notice_id}"),
            &admin_token,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(del.status(), StatusCode::NO_CONTENT);
}

const XLSX_MIME: &str = "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet";

async fn assert_xlsx(resp: axum::response::Response, label: &str) {
    let status = resp.status();
    let ctype = resp
        .headers()
        .get("content-type")
        .map(|v| v.to_str().unwrap().to_string())
        .unwrap_or_default();
    let disposition = resp
        .headers()
        .get("content-disposition")
        .map(|v| v.to_str().unwrap().to_string())
        .unwrap_or_default();
    let bytes = axum::body::to_bytes(resp.into_body(), 8 * 1024 * 1024)
        .await
        .unwrap();
    assert_eq!(status, StatusCode::OK, "{label} 导出失败");
    assert_eq!(ctype, XLSX_MIME, "{label} 响应不是 xlsx");
    assert!(
        disposition.contains("attachment") && disposition.contains("filename*=UTF-8''"),
        "{label} 缺少附件头：{disposition}"
    );
    // xlsx 是 zip 包，前两字节固定为 "PK"。
    assert!(
        bytes.len() > 200,
        "{label} xlsx 体积异常（{}）",
        bytes.len()
    );
    assert_eq!(&bytes[..2], b"PK", "{label} 不是合法 xlsx（缺 zip 魔数）");
}

#[tokio::test]
#[ignore = "需要真实 PostgreSQL"]
async fn exports_return_xlsx_and_respect_perm() {
    let (state, admin_token) = setup("web_system_export_test").await;
    let app = router(state);

    // 静态段优先：`/users/export` 不能被 `/users/{user_id}` 抢走。
    for (path, label) in [
        ("/api/v1/system/users/export", "users"),
        ("/api/v1/system/roles/export", "roles"),
        ("/api/v1/system/posts/export", "posts"),
        ("/api/v1/system/dict/types/export", "dict_types"),
        ("/api/v1/system/dict/data/export", "dict_data"),
        ("/api/v1/system/configs/export", "configs"),
    ] {
        let resp = app
            .clone()
            .oneshot(req("GET", path, &admin_token, None))
            .await
            .unwrap();
        assert_xlsx(resp, label).await;
    }

    // 详情接口不受新增静态段影响。
    let detail = app
        .clone()
        .oneshot(req("GET", "/api/v1/system/users/1", &admin_token, None))
        .await
        .unwrap();
    assert_eq!(detail.status(), StatusCode::OK);

    // 无导出权限的用户：列表可读，导出 403。
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

    let list = app
        .clone()
        .oneshot(req("GET", "/api/v1/system/users", &viewer_token, None))
        .await
        .unwrap();
    assert_eq!(list.status(), StatusCode::OK);

    let denied = app
        .clone()
        .oneshot(req(
            "GET",
            "/api/v1/system/users/export",
            &viewer_token,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(
        denied.status(),
        StatusCode::FORBIDDEN,
        "缺少 system:user:export 应为 403"
    );
}

/// 缓存设计：字典项真的进了缓存，而「改了立刻可见」靠版本号自增，不是靠 TTL。
///
/// 中间那步用裸 SQL 删行——绕过接口就绕过失效，于是「还是老样子」正是
/// 这条断言要证明的事：读到的确实是缓存，而不是又查了一次库。
#[tokio::test]
#[ignore = "需要真实 PostgreSQL"]
async fn dict_reads_are_cached_and_writes_invalidate_by_version() {
    let (state, admin_token) = setup("web_dict_cache_test").await;
    let app = router(state.clone());

    let created = app
        .clone()
        .oneshot(req(
            "POST",
            "/api/v1/system/dict/data",
            &admin_token,
            Some(serde_json::json!({
                "dict_label": "缓存验证", "dict_value": "c1",
                "dict_type": "biz_cache_type", "dict_sort": 1
            })),
        ))
        .await
        .unwrap();
    assert_eq!(created.status(), StatusCode::CREATED);
    drop(json_body(created).await);

    async fn read(app: &axum::Router, token: &str) -> serde_json::Value {
        let resp = app
            .clone()
            .oneshot(req(
                "GET",
                "/api/v1/system/dict/data/by-type/biz_cache_type",
                token,
                None,
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        json_body(resp).await
    }

    // 写一次把版本推到 1；读一次把这一版填进缓存。
    let v1 = cicada_system::entry_key(cicada_system::CacheDomain::Dict, None, "biz_cache_type", 1);
    let rows = read(&app, &admin_token).await;
    assert_eq!(rows.as_array().unwrap().len(), 1, "{rows}");
    assert!(
        state.sys_cache.peek_raw(&v1).await.is_some(),
        "读过一次之后，v1 这个键应当已经落在缓存里"
    );

    // 绕过接口直接删行：没有失效可触发，所以读到的还是缓存里的那一条。
    state
        .db
        .conn()
        .execute_unprepared("DELETE FROM sys_dict_data WHERE dict_value = 'c1'")
        .await
        .unwrap();
    let stale = read(&app, &admin_token).await;
    assert_eq!(
        stale.as_array().unwrap().len(),
        1,
        "这一读应当来自缓存，而不是重新查库：{stale}"
    );

    // 再走接口写一条：版本跳到 2，旧键立刻不可达，新的读重新回源。
    let added = app
        .clone()
        .oneshot(req(
            "POST",
            "/api/v1/system/dict/data",
            &admin_token,
            Some(serde_json::json!({
                "dict_label": "新版本", "dict_value": "c2",
                "dict_type": "biz_cache_type", "dict_sort": 2
            })),
        ))
        .await
        .unwrap();
    assert_eq!(added.status(), StatusCode::CREATED);
    drop(json_body(added).await);

    let fresh = read(&app, &admin_token).await;
    let arr = fresh.as_array().unwrap();
    assert_eq!(arr.len(), 1, "c1 已被裸删，c2 必须可见：{fresh}");
    assert_eq!(arr[0]["dict_value"].as_str().unwrap(), "c2");
    assert!(
        state
            .sys_cache
            .peek_raw(&cicada_system::entry_key(
                cicada_system::CacheDomain::Dict,
                None,
                "biz_cache_type",
                2
            ))
            .await
            .is_some(),
        "版本跳到 2 之后，新的键位才该被填上"
    );
    // 旧键没有被删除——这是刻意的：整域失效靠版本号让旧键不可达，
    // 删除交给 TTL 回收，避免「清缓存」变成一次扫库。
    assert!(state.sys_cache.peek_raw(&v1).await.is_some());
}

/// 内容安全：公告正文是富文本，入库前必须过白名单清洗。
///
/// `rich_text` 的单测只能证明清洗函数正确，证明不了「接口写进去的内容真的过了它」——
/// 而后者才是这一条的意义：漏接一个调用点，等于没做。所以这里从 HTTP 面打进去、
/// 再从读回的详情里看落库形状。
#[tokio::test]
#[ignore = "需要真实 PostgreSQL"]
async fn notice_content_is_sanitized_before_storage() {
    let (state, admin_token) = setup("web_notice_sanitize_test").await;
    let app = router(state);

    let dirty = "<p onclick=\"steal()\">今晚 <strong>23:00</strong> 停机 \
                 <a href=\"javascript:alert(1)\">详情</a>\
                 <script>fetch('//evil')</script></p>";
    let created = app
        .clone()
        .oneshot(req(
            "POST",
            "/api/v1/system/notices",
            &admin_token,
            Some(serde_json::json!({
                "notice_title": "停机公告", "notice_type": 1, "notice_content": dirty
            })),
        ))
        .await
        .unwrap();
    assert_eq!(created.status(), StatusCode::CREATED);
    let notice_id = json_body(created).await["notice_id"].as_i64().unwrap();

    let detail = app
        .clone()
        .oneshot(req(
            "GET",
            &format!("/api/v1/system/notices/{notice_id}"),
            &admin_token,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(detail.status(), StatusCode::OK);
    let stored = json_body(detail).await["notice_content"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(
        stored.contains("<p>") && stored.contains("<strong>23:00</strong>"),
        "排版标签不该被洗掉：{stored}"
    );
    assert!(
        !stored.contains("onclick") && !stored.contains("<script"),
        "{stored}"
    );
    assert!(
        !stored.contains("javascript:"),
        "协议不合法的链接该整个去掉：{stored}"
    );

    // 更新路径同样要过：漏一处就等于这处能写脏数据。
    let updated = app
        .clone()
        .oneshot(req(
            "PUT",
            &format!("/api/v1/system/notices/{notice_id}"),
            &admin_token,
            Some(serde_json::json!({
                "notice_title": "停机公告", "notice_type": 1,
                "notice_content": "<img src=\"data:text/html,<script>alert(1)</script>\" onerror=\"x()\">"
            })),
        ))
        .await
        .unwrap();
    assert_eq!(updated.status(), StatusCode::OK);
    let stored = json_body(updated).await["notice_content"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(
        !stored.contains("data:") && !stored.contains("onerror"),
        "{stored}"
    );

    let del = app
        .clone()
        .oneshot(req(
            "DELETE",
            &format!("/api/v1/system/notices/{notice_id}"),
            &admin_token,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(del.status(), StatusCode::NO_CONTENT);
}
