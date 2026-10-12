//! 接口清单里此前「只有承诺、没有 handler」的接口（真实 PostgreSQL）。
//!
//! 覆盖：角色启停 / 角色数据范围 / 角色用户分配、菜单详情与排序与角色勾选树、
//! 部门树与排序、公告已读名单与顶栏公告。

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
    let body = axum::body::to_bytes(resp.into_body(), 4 * 1024 * 1024)
        .await
        .unwrap();
    if body.is_empty() {
        return serde_json::Value::Null;
    }
    serde_json::from_slice(&body).unwrap_or(serde_json::Value::String(
        String::from_utf8_lossy(&body).into_owned(),
    ))
}

async fn status_of(resp: axum::response::Response) -> (StatusCode, serde_json::Value) {
    let code = resp.status();
    (code, json_body(resp).await)
}

/// 建一个可改的角色（内置 `admin` 角色不允许停用，用它反证）。
async fn new_role(app: &axum::Router, token: &str, key: &str) -> i64 {
    let resp = app
        .clone()
        .oneshot(req(
            "POST",
            "/api/v1/system/roles",
            token,
            Some(serde_json::json!({
                "role_name": key,
                "role_key": key,
                "role_sort": 3,
                "data_scope": 1,
                "status": 0,
            })),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    json_body(resp).await["role_id"].as_i64().unwrap()
}

#[tokio::test]
#[ignore = "需要真实 PostgreSQL"]
async fn role_status_and_data_scope_endpoints_work() {
    let (state, token) = setup("web_e_role_status").await;
    let app = router(state.clone());
    let role_id = new_role(&app, &token, "auditor").await;

    // 启停。
    let (code, body) = status_of(
        app.clone()
            .oneshot(req(
                "PUT",
                &format!("/api/v1/system/roles/{role_id}/status"),
                &token,
                Some(serde_json::json!({ "status": 1 })),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(code, StatusCode::OK, "{body}");
    assert_eq!(body["status"].as_i64().unwrap(), 1);

    let (code, _) = status_of(
        app.clone()
            .oneshot(req(
                "PUT",
                &format!("/api/v1/system/roles/{role_id}/status"),
                &token,
                Some(serde_json::json!({ "status": 7 })),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(code, StatusCode::UNPROCESSABLE_ENTITY);

    // 内置超级角色不可停用——停用它等于把自己锁在门外。
    let admin_role = app
        .clone()
        .oneshot(req("GET", "/api/v1/system/roles", &token, None))
        .await
        .unwrap();
    let list = json_body(admin_role).await;
    let builtin = list["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["role_key"].as_str() == Some("admin"))
        .expect("init_admin 应建出 admin 角色");
    let builtin_id = builtin["role_id"].as_i64().unwrap();
    let (code, _) = status_of(
        app.clone()
            .oneshot(req(
                "PUT",
                &format!("/api/v1/system/roles/{builtin_id}/status"),
                &token,
                Some(serde_json::json!({ "status": 1 })),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(code, StatusCode::FORBIDDEN);

    // 数据范围：自定（2）要带部门，且真的写进 sys_role_dept。
    let dept_id = app
        .clone()
        .oneshot(req(
            "POST",
            "/api/v1/system/depts",
            &token,
            Some(serde_json::json!({
                "parent_id": 0, "dept_name": "范围部", "order_num": 1,
            })),
        ))
        .await
        .unwrap();
    let dept_id = json_body(dept_id).await["dept_id"].as_i64().unwrap();

    let (code, body) = status_of(
        app.clone()
            .oneshot(req(
                "PUT",
                &format!("/api/v1/system/roles/{role_id}/data-scope"),
                &token,
                Some(serde_json::json!({ "data_scope": 2, "dept_ids": [dept_id] })),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(code, StatusCode::OK, "{body}");
    assert_eq!(body["data_scope"].as_i64().unwrap(), 2);

    let rows = state
        .db
        .conn()
        .query_all_raw(sea_orm::Statement::from_string(
            sea_orm::DatabaseBackend::Postgres,
            format!("SELECT dept_id FROM sys_role_dept WHERE role_id = {role_id}"),
        ))
        .await
        .unwrap();
    assert_eq!(rows.len(), 1, "自定范围应落一条 sys_role_dept");

    let (_, body) = status_of(
        app.clone()
            .oneshot(req(
                "PUT",
                &format!("/api/v1/system/roles/{role_id}/data-scope"),
                &token,
                Some(serde_json::json!({ "data_scope": 2, "dept_ids": [] })),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert!(body.get("code").is_some(), "422 应是 problem+json：{body}");
}

#[tokio::test]
#[ignore = "需要真实 PostgreSQL"]
async fn role_users_endpoints_allocate_and_cancel() {
    let (state, token) = setup("web_e_role_users").await;
    let app = router(state.clone());
    let role_id = new_role(&app, &token, "reviewer").await;

    // 再建一个用户当分配对象。
    let created = app
        .clone()
        .oneshot(req(
            "POST",
            "/api/v1/system/users",
            &token,
            Some(serde_json::json!({
                "user_name": "carol",
                "nick_name": "Car",
                "password": "Cicada@2026",
                "status": 0,
            })),
        ))
        .await
        .unwrap();
    assert_eq!(created.status(), StatusCode::CREATED);
    let carol = json_body(created).await["user_id"].as_i64().unwrap();

    // 列表带 belonged：admin 已属于内置角色，但不属于 reviewer。
    let list = json_body(
        app.clone()
            .oneshot(req(
                "GET",
                &format!("/api/v1/system/roles/{role_id}/users?user_name=carol"),
                &token,
                None,
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(list["total"].as_u64().unwrap(), 1, "模糊检索应命中 carol");
    assert!(
        !list["items"][0]["belonged"].as_bool().unwrap(),
        "新用户不该已经属于这个角色"
    );

    // 分配。
    let (code, body) = status_of(
        app.clone()
            .oneshot(req(
                "PUT",
                &format!("/api/v1/system/roles/{role_id}/users"),
                &token,
                Some(serde_json::json!({ "action": "allocate", "user_ids": [carol] })),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(code, StatusCode::OK, "{body}");
    assert_eq!(body["changed"].as_u64().unwrap(), 1);

    // 幂等：重复分配不再计数。
    let (_, body) = status_of(
        app.clone()
            .oneshot(req(
                "PUT",
                &format!("/api/v1/system/roles/{role_id}/users"),
                &token,
                Some(serde_json::json!({ "action": "allocate", "user_ids": [carol] })),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(body["changed"].as_u64().unwrap(), 0);

    let list = json_body(
        app.clone()
            .oneshot(req(
                "GET",
                &format!("/api/v1/system/roles/{role_id}/users?belonged=true"),
                &token,
                None,
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(list["items"][0]["user_name"].as_str().unwrap(), "carol");

    // 跨租户用户混进来时整批拒绝（这里用不存在的 id 代表「不可见」）。
    let (code, _) = status_of(
        app.clone()
            .oneshot(req(
                "PUT",
                &format!("/api/v1/system/roles/{role_id}/users"),
                &token,
                Some(serde_json::json!({ "action": "allocate", "user_ids": [carol, 999_999] })),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(code, StatusCode::UNPROCESSABLE_ENTITY);

    // 取消分配。
    let (_, body) = status_of(
        app.clone()
            .oneshot(req(
                "PUT",
                &format!("/api/v1/system/roles/{role_id}/users"),
                &token,
                Some(serde_json::json!({ "action": "cancel", "user_ids": [carol] })),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(body["changed"].as_u64().unwrap(), 1);
    assert_eq!(
        state
            .db
            .conn()
            .query_all_raw(sea_orm::Statement::from_string(
                sea_orm::DatabaseBackend::Postgres,
                format!(
                    "SELECT 1 FROM sys_user_role WHERE role_id = {role_id} AND user_id = {carol}"
                )
            ))
            .await
            .unwrap()
            .len(),
        0
    );
}

#[tokio::test]
#[ignore = "需要真实 PostgreSQL"]
async fn menu_detail_sort_and_role_tree_work() {
    let (state, token) = setup("web_e_menus").await;
    let app = router(state.clone());

    let tree = json_body(
        app.clone()
            .oneshot(req("GET", "/api/v1/system/menus/tree", &token, None))
            .await
            .unwrap(),
    )
    .await;
    let tree = tree.as_array().unwrap();
    assert!(!tree.is_empty(), "种子菜单应能按树返回");
    let parent_id = tree[0]["menu_id"].as_i64().unwrap();

    let child = json_body(
        app.clone()
            .oneshot(req(
                "POST",
                "/api/v1/system/menus",
                &token,
                Some(serde_json::json!({
                    "menu_name": "排序子菜单",
                    "parent_id": parent_id,
                    "order_num": 9,
                    "menu_type": "C",
                })),
            ))
            .await
            .unwrap(),
    )
    .await;
    let child_id = child["menu_id"].as_i64().unwrap();

    // 详情。
    let (code, detail) = status_of(
        app.clone()
            .oneshot(req(
                "GET",
                &format!("/api/v1/system/menus/{child_id}"),
                &token,
                None,
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(code, StatusCode::OK, "{detail}");
    assert_eq!(detail["menu_name"].as_str().unwrap(), "排序子菜单");

    // 排序。
    let (code, sorted) = status_of(
        app.clone()
            .oneshot(req(
                "PUT",
                &format!("/api/v1/system/menus/{child_id}/sort"),
                &token,
                Some(serde_json::json!({ "order_num": 1 })),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(code, StatusCode::OK, "{sorted}");
    assert_eq!(sorted["order_num"].as_i64().unwrap(), 1);

    // 防环：把自己的父节点设成自己的子菜单。
    let grandchild = json_body(
        app.clone()
            .oneshot(req(
                "POST",
                "/api/v1/system/menus",
                &token,
                Some(serde_json::json!({
                    "menu_name": "孙菜单", "parent_id": child_id, "order_num": 1, "menu_type": "C",
                })),
            ))
            .await
            .unwrap(),
    )
    .await;
    let grand_id = grandchild["menu_id"].as_i64().unwrap();
    let (code, _) = status_of(
        app.clone()
            .oneshot(req(
                "PUT",
                &format!("/api/v1/system/menus/{child_id}/sort"),
                &token,
                Some(serde_json::json!({ "parent_id": grand_id, "order_num": 2 })),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(code, StatusCode::UNPROCESSABLE_ENTITY, "父子互换必须被拒");
    let (code, _) = status_of(
        app.clone()
            .oneshot(req(
                "PUT",
                &format!("/api/v1/system/menus/{child_id}/sort"),
                &token,
                Some(serde_json::json!({ "parent_id": child_id, "order_num": 2 })),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(code, StatusCode::UNPROCESSABLE_ENTITY, "父节点不能是自己");

    // 角色勾选树：树 + 已勾中的菜单 id。
    let role_id = new_role(&app, &token, "menu_user").await;
    let tree = json_body(
        app.clone()
            .oneshot(req(
                "GET",
                &format!("/api/v1/system/menus/role-tree/{role_id}"),
                &token,
                None,
            ))
            .await
            .unwrap(),
    )
    .await;
    assert!(!tree["tree"].as_array().unwrap().is_empty());
    assert_eq!(
        tree["checked"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|v| v.as_i64() == Some(child_id))
            .count(),
        0,
        "新角色一条都没勾"
    );

    // 勾上再读回来。
    let (code, _) = status_of(
        app.clone()
            .oneshot(req(
                "PUT",
                &format!("/api/v1/system/roles/{role_id}"),
                &token,
                Some(serde_json::json!({
                    "role_name": "menu_user", "role_key": "menu_user", "menu_ids": [child_id],
                })),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(code, StatusCode::OK);
    let tree = json_body(
        app.clone()
            .oneshot(req(
                "GET",
                &format!("/api/v1/system/menus/role-tree/{role_id}"),
                &token,
                None,
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(tree["checked"].as_array().unwrap().len(), 1);

    // 不存在的角色：404，不泄露「这个 id 在别的租户」。
    let (code, _) = status_of(
        app.clone()
            .oneshot(req(
                "GET",
                "/api/v1/system/menus/role-tree/99999",
                &token,
                None,
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(code, StatusCode::NOT_FOUND);
}

#[tokio::test]
#[ignore = "需要真实 PostgreSQL"]
async fn dept_tree_and_sort_work() {
    let (state, token) = setup("web_e_depts").await;
    let app = router(state.clone());

    let tree = json_body(
        app.clone()
            .oneshot(req("GET", "/api/v1/system/depts/tree", &token, None))
            .await
            .unwrap(),
    )
    .await;
    assert!(tree.as_array().unwrap().is_empty(), "全新租户没有部门");

    let root = json_body(
        app.clone()
            .oneshot(req(
                "POST",
                "/api/v1/system/depts",
                &token,
                Some(serde_json::json!({
                    "parent_id": 0, "dept_name": "总部", "order_num": 5,
                })),
            ))
            .await
            .unwrap(),
    )
    .await;
    let root_id = root["dept_id"].as_i64().unwrap();
    let tree = json_body(
        app.clone()
            .oneshot(req("GET", "/api/v1/system/depts/tree", &token, None))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(tree.as_array().unwrap().len(), 1);

    let (code, body) = status_of(
        app.clone()
            .oneshot(req(
                "PUT",
                &format!("/api/v1/system/depts/{root_id}/sort"),
                &token,
                Some(serde_json::json!({ "order_num": 2 })),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(code, StatusCode::OK, "{body}");
    assert_eq!(body["order_num"].as_i64().unwrap(), 2);
    let (code, _) = status_of(
        app.clone()
            .oneshot(req(
                "PUT",
                &format!("/api/v1/system/depts/{root_id}/sort"),
                &token,
                Some(serde_json::json!({ "order_num": -1 })),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(code, StatusCode::UNPROCESSABLE_ENTITY);
    let (code, _) = status_of(
        app.oneshot(req(
            "PUT",
            "/api/v1/system/depts/99999/sort",
            &token,
            Some(serde_json::json!({ "order_num": 1 })),
        ))
        .await
        .unwrap(),
    )
    .await;
    assert_eq!(code, StatusCode::NOT_FOUND);
}

#[tokio::test]
#[ignore = "需要真实 PostgreSQL"]
async fn notice_readers_and_top_work() {
    let (state, token) = setup("web_e_notices").await;
    let app = router(state.clone());

    let notice = json_body(
        app.clone()
            .oneshot(req(
                "POST",
                "/api/v1/system/notices",
                &token,
                Some(serde_json::json!({
                    "notice_title": "停机公告", "notice_type": 1, "notice_content": "今晚",
                    "status": 0,
                })),
            ))
            .await
            .unwrap(),
    )
    .await;
    let notice_id = notice["notice_id"].as_i64().unwrap();

    // 顶栏只需登录：带已读标记。
    let top = json_body(
        app.clone()
            .oneshot(req("GET", "/api/v1/system/notices/top", &token, None))
            .await
            .unwrap(),
    )
    .await;
    let top = top.as_array().unwrap();
    assert!(!top.is_empty());
    let mine = top
        .iter()
        .find(|n| n["notice_id"].as_i64() == Some(notice_id))
        .unwrap();
    assert!(!mine["read"].as_bool().unwrap());

    // 标记已读后，顶栏与名单都要看见。
    let (code, _) = status_of(
        app.clone()
            .oneshot(req(
                "POST",
                &format!("/api/v1/system/notices/{notice_id}/read"),
                &token,
                None,
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(code, StatusCode::NO_CONTENT);

    let top = json_body(
        app.clone()
            .oneshot(req(
                "GET",
                "/api/v1/system/notices/top?limit=1",
                &token,
                None,
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(top.as_array().unwrap().len(), 1);
    assert!(top[0]["read"].as_bool().unwrap());

    let readers = json_body(
        app.clone()
            .oneshot(req(
                "GET",
                &format!("/api/v1/system/notices/{notice_id}/readers"),
                &token,
                None,
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(readers["total"].as_u64().unwrap(), 1);
    assert_eq!(readers["items"][0]["user_name"].as_str().unwrap(), "admin");

    // 未认证没有身份可标：401。
    let (code, _) = status_of(
        app.oneshot(req("GET", "/api/v1/system/notices/top", "", None))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(code, StatusCode::UNAUTHORIZED);
}
