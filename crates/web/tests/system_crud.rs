//! 系统管理模块集成测试（真实 PostgreSQL + 内存缓存）。
//!
//! 覆盖：角色 CRUD、角色改菜单后**权限缓存按版本号整体失效**（改权限即时生效）、
//! 菜单 CRUD、用户 CRUD、改密踢下线、内置账号保护。

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

/// 一次请求的状态码 + JSON 体（断言里两个都要看：状态说明语义，体说明内容）。
async fn status_and_json(resp: axum::response::Response) -> (StatusCode, serde_json::Value) {
    let status = resp.status();
    (status, json_body(resp).await)
}

async fn json_body(resp: axum::response::Response) -> serde_json::Value {
    let body = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
        .await
        .unwrap();
    serde_json::from_slice(&body).unwrap()
}

#[tokio::test]
#[ignore = "需要真实 PostgreSQL"]
async fn system_modules_and_cache_invalidation() {
    let (state, admin_token) = setup("web_system_test").await;
    let app = router(state);

    // ---- 角色列表：含内置 admin ----
    let roles = app
        .clone()
        .oneshot(req("GET", "/api/v1/system/roles", &admin_token, None))
        .await
        .unwrap();
    assert_eq!(roles.status(), StatusCode::OK);
    let roles_json = json_body(roles).await;
    assert!(roles_json["total"].as_u64().unwrap() >= 1);

    // ---- 新建角色，授予 menu 1000（system:user:list）----
    let create_role = app
        .clone()
        .oneshot(req(
            "POST",
            "/api/v1/system/roles",
            &admin_token,
            Some(serde_json::json!({
                "role_name": "只读用户管理员",
                "role_key": "user_viewer",
                "data_scope": 1,
                "menu_ids": [1, 100, 1000],
                "status": 0
            })),
        ))
        .await
        .unwrap();
    let status = create_role.status();
    let role = json_body(create_role).await;
    assert_eq!(status, StatusCode::CREATED, "{role}");
    let role_id = role["role_id"].as_i64().unwrap();

    // ---- 读回已授权菜单 id：前端「权限分配」提交时整体替换，读不到原授权就会清空它 ----
    let granted = app
        .clone()
        .oneshot(req(
            "GET",
            &format!("/api/v1/system/roles/{role_id}/menus"),
            &admin_token,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(granted.status(), StatusCode::OK);
    let mut granted_ids: Vec<i64> = json_body(granted)
        .await
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_i64().unwrap())
        .collect();
    granted_ids.sort_unstable();
    assert_eq!(granted_ids, vec![1, 100, 1000]);

    // 不存在的角色 → 404（不是空数组：角色不存在与角色无授权必须可区分）。
    let missing = app
        .clone()
        .oneshot(req(
            "GET",
            "/api/v1/system/roles/99999999/menus",
            &admin_token,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);

    // 重复 role_key → 409
    let dup = app
        .clone()
        .oneshot(req(
            "POST",
            "/api/v1/system/roles",
            &admin_token,
            Some(serde_json::json!({ "role_name": "x", "role_key": "user_viewer" })),
        ))
        .await
        .unwrap();
    assert_eq!(dup.status(), StatusCode::CONFLICT);

    // ---- 新建用户 alice，绑定该角色 ----
    let create_user = app
        .clone()
        .oneshot(req(
            "POST",
            "/api/v1/system/users",
            &admin_token,
            Some(serde_json::json!({
                "user_name": "alice",
                "nick_name": "Alice",
                "password": "Alice@123",
                "email": "Alice@Example.com",
                "role_ids": [role_id]
            })),
        ))
        .await
        .unwrap();
    let status = create_user.status();
    let user = json_body(create_user).await;
    assert_eq!(status, StatusCode::CREATED, "{user}");
    let alice_id = user["user_id"].as_i64().unwrap();
    assert_eq!(user["role_ids"], serde_json::json!([role_id]));

    // ---- alice 登录 ----
    let login = app
        .clone()
        .oneshot(req(
            "POST",
            "/auth/login",
            "",
            Some(serde_json::json!({ "username": "alice", "password": "Alice@123" })),
        ))
        .await
        .unwrap();
    assert_eq!(login.status(), StatusCode::OK);
    let alice_token = json_body(login).await["access_token"]
        .as_str()
        .unwrap()
        .to_string();

    // alice 有 system:user:list → 可列用户；无 system:role:list → 403
    let alice_users = app
        .clone()
        .oneshot(req("GET", "/api/v1/system/users", &alice_token, None))
        .await
        .unwrap();
    assert_eq!(alice_users.status(), StatusCode::OK);
    let alice_roles = app
        .clone()
        .oneshot(req("GET", "/api/v1/system/roles", &alice_token, None))
        .await
        .unwrap();
    assert_eq!(alice_roles.status(), StatusCode::FORBIDDEN);

    // ---- 关键：改角色去掉 menu 1000 → 权限缓存按版本号整体失效，alice 立即失去 list ----
    let update_role = app
        .clone()
        .oneshot(req(
            "PUT",
            &format!("/api/v1/system/roles/{role_id}"),
            &admin_token,
            Some(serde_json::json!({
                "role_name": "只读用户管理员",
                "role_key": "user_viewer",
                "data_scope": 1,
                "menu_ids": [1]
            })),
        ))
        .await
        .unwrap();
    assert_eq!(update_role.status(), StatusCode::OK);

    let alice_after = app
        .clone()
        .oneshot(req("GET", "/api/v1/system/users", &alice_token, None))
        .await
        .unwrap();
    assert_eq!(
        alice_after.status(),
        StatusCode::FORBIDDEN,
        "角色菜单变更后未过期缓存必须失效"
    );

    // ---- 用户改密：新密码可登录、旧密码 422；会话被踢 ----
    let reset = app
        .clone()
        .oneshot(req(
            "PUT",
            &format!("/api/v1/system/users/{alice_id}/password"),
            &admin_token,
            Some(serde_json::json!({ "password": "NewAlice@456" })),
        ))
        .await
        .unwrap();
    assert_eq!(reset.status(), StatusCode::NO_CONTENT);

    let old_pw = app
        .clone()
        .oneshot(req(
            "POST",
            "/auth/login",
            "",
            Some(serde_json::json!({ "username": "alice", "password": "Alice@123" })),
        ))
        .await
        .unwrap();
    assert_eq!(old_pw.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let new_pw = app
        .clone()
        .oneshot(req(
            "POST",
            "/auth/login",
            "",
            Some(serde_json::json!({ "username": "alice", "password": "NewAlice@456" })),
        ))
        .await
        .unwrap();
    assert_eq!(new_pw.status(), StatusCode::OK);

    // alice 旧 token 已被改密踢下线
    let kicked = app
        .clone()
        .oneshot(req("GET", "/auth/profile", &alice_token, None))
        .await
        .unwrap();
    assert_eq!(kicked.status(), StatusCode::UNAUTHORIZED);

    // ---- 菜单 CRUD ----
    let created_menu = app
        .clone()
        .oneshot(req(
            "POST",
            "/api/v1/system/menus",
            &admin_token,
            Some(serde_json::json!({
                "menu_name": "测试目录", "menu_type": "M", "path": "biz", "parent_id": 0, "order_num": 99
            })),
        ))
        .await
        .unwrap();
    assert_eq!(created_menu.status(), StatusCode::CREATED);
    let menu_id = json_body(created_menu).await["menu_id"].as_i64().unwrap();

    let updated_menu = app
        .clone()
        .oneshot(req(
            "PUT",
            &format!("/api/v1/system/menus/{menu_id}"),
            &admin_token,
            Some(serde_json::json!({
                "menu_name": "测试目录2", "menu_type": "M", "path": "biz2", "parent_id": 0, "order_num": 98
            })),
        ))
        .await
        .unwrap();
    assert_eq!(updated_menu.status(), StatusCode::OK);

    let tree = app
        .clone()
        .oneshot(req("GET", "/api/v1/system/menus", &admin_token, None))
        .await
        .unwrap();
    assert_eq!(tree.status(), StatusCode::OK);

    let del_menu = app
        .clone()
        .oneshot(req(
            "DELETE",
            &format!("/api/v1/system/menus/{menu_id}"),
            &admin_token,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(del_menu.status(), StatusCode::NO_CONTENT);

    // ---- 内置账号保护 ----
    let del_admin = app
        .clone()
        .oneshot(req("DELETE", "/api/v1/system/users/1", &admin_token, None))
        .await
        .unwrap();
    assert_eq!(del_admin.status(), StatusCode::FORBIDDEN);

    // ---- 删除 alice → 登录失败；alice 身份（含邮箱镜像）随建号写入 ----
    let del_user = app
        .clone()
        .oneshot(req(
            "DELETE",
            &format!("/api/v1/system/users/{alice_id}"),
            &admin_token,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(del_user.status(), StatusCode::NO_CONTENT);
    let gone = app
        .clone()
        .oneshot(req(
            "POST",
            "/auth/login",
            "",
            Some(serde_json::json!({ "username": "alice", "password": "NewAlice@456" })),
        ))
        .await
        .unwrap();
    assert_eq!(gone.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

/// 口令规则真的生效（此前 `auth.password.min_length` 没有读者，
/// 重置密码只查 `len < 6`，比规则松且不查复杂度）。
#[tokio::test]
#[ignore = "需要真实 PostgreSQL"]
async fn password_policy_is_enforced_and_config_driven() {
    let (state, token) = setup("web_password_policy").await;
    let app = router(state.clone());

    // 弱口令：缺大写与数字。
    let resp = app
        .clone()
        .oneshot(req(
            "POST",
            "/api/v1/system/users",
            &token,
            Some(serde_json::json!({
                "user_name": "weakling", "nick_name": "W", "password": "abcdefgh",
            })),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let body = json_body(resp).await;
    assert_eq!(body["code"].as_str().unwrap(), "CICADA_VALIDATION_4001");
    let errors = body["errors"].as_array().unwrap();
    // 一个字段一条错误，但缺的两项都要在消息里说全（贴回输入框后不能再猜）。
    assert_eq!(errors.len(), 1, "{body}");
    let message = errors[0]["message"].as_str().unwrap_or_default();
    assert!(
        message.contains("缺大写字母") && message.contains("缺数字"),
        "违规项要说全：{message}"
    );
    assert!(
        errors
            .iter()
            .all(|e| e["field"].as_str() == Some("password")),
        "{body}"
    );

    // 达标的口令能建号，并且重置也走同一份规则。
    let created = app
        .clone()
        .oneshot(req(
            "POST",
            "/api/v1/system/users",
            &token,
            Some(serde_json::json!({
                "user_name": "stronger", "nick_name": "S", "password": "Cicada@2026",
            })),
        ))
        .await
        .unwrap();
    assert_eq!(created.status(), StatusCode::CREATED);
    let user_id = json_body(created).await["user_id"].as_i64().unwrap();

    let resp = app
        .clone()
        .oneshot(req(
            "PUT",
            &format!("/api/v1/system/users/{user_id}/password"),
            &token,
            Some(serde_json::json!({ "password": "Ab1" })),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let body = json_body(resp).await;
    assert!(
        body["detail"]
            .as_str()
            .unwrap_or_default()
            .contains("至少 8 位"),
        "提示要带上阈值：{body}"
    );

    // 阈值来自配置：把下限调到 12 后，刚才合规的口令也应因长度被拒。
    let mut strict = state.clone();
    strict.config.auth.password.min_length = 12;
    let app = router(strict);
    let resp = app
        .oneshot(req(
            "PUT",
            &format!("/api/v1/system/users/{user_id}/password"),
            &token,
            Some(serde_json::json!({ "password": "Cicada@2026" })),
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::UNPROCESSABLE_ENTITY,
        "min_length 必须真的被读到"
    );
    let body = json_body(resp).await;
    assert!(
        body["detail"]
            .as_str()
            .unwrap_or_default()
            .contains("至少 12 位"),
        "{body}"
    );
}

/// 建模规范乐观锁：四个带 `version` 的可变实体（用户 / 角色 / 部门 / 岗位）都要
/// 「带对版本才改得动、带旧版本明确冲突、不带版本仍可改」。
/// 测试分层把「乐观锁冲突」列为关键并发行为，必须有专门测试——静默覆盖是这一节
/// 明确禁止的行为，而它恰恰是最难靠 review 发现的那类退化。
#[tokio::test]
#[ignore = "需要真实 PostgreSQL"]
async fn optimistic_lock_governs_every_versioned_entity() {
    let (state, token) = setup("web_optimistic_lock").await;

    // (标签, 建号 URI, 建号体, 取 id 的字段, 改动用的 UPDATE 体生成器)
    // (标签, 建号 URI, 建号体, id 字段, 生成 (改动用的 URI, 请求体))
    type Case = (
        &'static str,
        &'static str,
        serde_json::Value,
        &'static str,
        fn(&str, i64) -> (String, serde_json::Value),
    );
    let cases: Vec<Case> = vec![
        (
            "user",
            "/api/v1/system/users",
            serde_json::json!({
                "user_name": "cas_user",
                "nick_name": "第一次的名字",
                "password": "Cicada@123",
            }),
            "user_id",
            |_: &str, id: i64| {
                (
                    format!("/api/v1/system/users/{id}"),
                    serde_json::json!({ "nick_name": "改过的名字" }),
                )
            },
        ),
        (
            "role",
            "/api/v1/system/roles",
            serde_json::json!({ "role_name": "CAS 角色", "role_key": "cas_role" }),
            "role_id",
            |_: &str, id: i64| {
                (
                    format!("/api/v1/system/roles/{id}"),
                    serde_json::json!({ "role_name": "改过的角色", "role_key": "cas_role" }),
                )
            },
        ),
        (
            "dept",
            "/api/v1/system/depts",
            serde_json::json!({ "parent_id": 0, "dept_name": "CAS 部门", "order_num": 1, "status": 0 }),
            "dept_id",
            |_: &str, id: i64| {
                (
                    format!("/api/v1/system/depts/{id}"),
                    serde_json::json!({ "parent_id": 0, "dept_name": "改过的部门", "order_num": 1, "status": 0 }),
                )
            },
        ),
        (
            "post",
            "/api/v1/system/posts",
            serde_json::json!({ "post_code": "cas_post", "post_name": "CAS 岗位", "status": 0 }),
            "post_id",
            |_: &str, id: i64| {
                (
                    format!("/api/v1/system/posts/{id}"),
                    serde_json::json!({ "post_code": "cas_post", "post_name": "改过的岗位", "status": 0 }),
                )
            },
        ),
    ];

    for (label, create_uri, create_body, id_field, make_update) in cases {
        let app = router(state.clone());
        let created = json_body(
            app.clone()
                .oneshot(req("POST", create_uri, &token, Some(create_body)))
                .await
                .unwrap(),
        )
        .await;
        assert!(
            created[id_field].as_i64().is_some(),
            "{label} 建号失败：{created}"
        );
        let id = created[id_field].as_i64().unwrap();
        assert_eq!(
            created["version"].as_i64(),
            Some(0),
            "{label} 新建版本应为 0"
        );

        // 1) 带正确版本 → 200，且版本推进到 1。
        let (uri, mut body) = make_update(label, id);
        body["version"] = serde_json::json!(0);
        let ok = json_body(
            app.clone()
                .oneshot(req("PUT", &uri, &token, Some(body.clone())))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(
            ok["version"].as_i64(),
            Some(1),
            "{label} CAS 成功后版本应 +1：{ok}"
        );

        // 2) 拿同一个旧版本再改 → 409 + 明确的乐观锁业务码（不是静默覆盖，也不是 500）。
        let stale = app
            .clone()
            .oneshot(req("PUT", &uri, &token, Some(body)))
            .await
            .unwrap();
        let status = stale.status();
        let stale_json = json_body(stale).await;
        assert_eq!(
            status,
            StatusCode::CONFLICT,
            "{label} 旧版本必须被拒：{stale_json}"
        );
        assert_eq!(
            stale_json["code"].as_str(),
            Some("CICADA_SYS_3003"),
            "{label} 应报业务码目录的乐观锁码：{stale_json}"
        );

        // 3) 库里那一行没被旧请求写坏：读回来还是第一次改的名字。
        let read = json_body(
            app.clone()
                .oneshot(req("GET", &uri, &token, None))
                .await
                .unwrap(),
        )
        .await;
        let name_key = if label == "user" {
            "nick_name"
        } else if label == "dept" {
            "dept_name"
        } else if label == "post" {
            "post_name"
        } else {
            "role_name"
        };
        assert_eq!(
            read[name_key].as_str(),
            Some(match label {
                "user" => "改过的名字",
                "role" => "改过的角色",
                "dept" => "改过的部门",
                _ => "改过的岗位",
            }),
            "{label} 被旧版本的请求改坏了（静默覆盖）：{read}"
        );

        // 4) 兼容存量客户端：不带 version 仍可改（后写覆盖），但版本照样推进。
        let (uri2, body2) = make_update(label, id);
        assert_eq!(
            app.clone()
                .oneshot(req("PUT", &uri2, &token, Some(body2)))
                .await
                .unwrap()
                .status(),
            StatusCode::OK,
            "{label} 不带 version 的存量客户端不该被 409 挡住"
        );
        assert_eq!(
            json_body(
                app.clone()
                    .oneshot(req("GET", &uri2, &token, None))
                    .await
                    .unwrap()
            )
            .await["version"]
                .as_i64(),
            Some(2),
            "{label} 不带版本的写入也要推进 version"
        );
    }
}

/// 排序白名单在真实 HTTP 面上的行为：显式排序**真的换序**、默认次序不变、
/// 白名单外的名字被忽略而不是报错或执行。
///
/// 这条测试是为「接线顺序」而写的：`.sorted_by()` 若排在默认排序之后，
/// 而默认排序以唯一主键开头，那么显式排序永远只是平局裁决——传了等于没传。
/// 纯单元测试（`sort::resolve`）看不见这件事，只有端到端能看见。
#[tokio::test]
#[ignore = "需要真实 PostgreSQL"]
async fn sort_whitelist_reorders_lists_and_rejects_unlisted_names() {
    let (state, token) = setup("web_list_sort").await;
    let app = router(state.clone());

    // 造三个岗位：post_sort 故意乱序，且让 post_name 与 post_code 给出不同的排序结果。
    for (code, name, sort) in [
        ("c_code", "Alpha 岗", 3),
        ("a_code", "Beta 岗", 1),
        ("b_code", "Zeta 岗", 2),
    ] {
        let (status, body) = status_and_json(
            app.clone()
                .oneshot(req(
                    "POST",
                    "/api/v1/system/posts",
                    &token,
                    Some(serde_json::json!({
                        "post_code": code, "post_name": name, "post_sort": sort, "status": 0,
                    })),
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
    }

    let names_of = |items: &serde_json::Value| -> Vec<String> {
        items["items"]
            .as_array()
            .expect("列表必须是 {items,...}")
            .iter()
            .map(|r| r["post_name"].as_str().unwrap_or_default().to_string())
            .collect()
    };

    let asc = json_body(
        app.clone()
            .oneshot(req(
                "GET",
                "/api/v1/system/posts?sort_by=post_name&sort_order=asc&page_size=50",
                &token,
                None,
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(
        names_of(&asc),
        ["Alpha 岗", "Beta 岗", "Zeta 岗"],
        "按名字升序：{asc}"
    );

    let desc = json_body(
        app.clone()
            .oneshot(req(
                "GET",
                "/api/v1/system/posts?sort_by=post_name&sort_order=desc&page_size=50",
                &token,
                None,
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(
        names_of(&desc),
        ["Zeta 岗", "Beta 岗", "Alpha 岗"],
        "按名字降序：{desc}"
    );

    // 换一个白名单列：post_sort 升序给出的是第三种次序，证明用的是那一列而不是名字。
    let by_sort = json_body(
        app.clone()
            .oneshot(req(
                "GET",
                "/api/v1/system/posts?sort_by=post_sort&sort_order=asc&page_size=50",
                &token,
                None,
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(
        names_of(&by_sort),
        ["Beta 岗", "Zeta 岗", "Alpha 岗"],
        "按 post_sort 升序：{by_sort}"
    );

    // 不传排序 → 该接口的默认次序（post_sort → post_id）原样保留。
    let plain = json_body(
        app.clone()
            .oneshot(req(
                "GET",
                "/api/v1/system/posts?page_size=50",
                &token,
                None,
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(
        names_of(&plain),
        names_of(&by_sort),
        "默认排序应与 post_sort 升序一致：{plain}"
    );

    // 白名单外：200 + 忽略（既不是 422，也不可能执行客户端给的标识符）。
    for hostile in [
        "version",
        "remark",
        "1; DROP TABLE sys_post",
        "post_name DESC, post_id",
    ] {
        // 这些字符串要出现在 query 里，必须先百分号编码——否则是 URI 非法字符，
        // 测的就变成「URI 解析失败」而不是「白名单忽略」。
        let encoded = hostile
            .replace(' ', "%20")
            .replace(';', "%3B")
            .replace(',', "%2C");
        let (status, body) = status_and_json(
            app.clone()
                .oneshot(req(
                    "GET",
                    &format!("/api/v1/system/posts?page_size=50&sort_by={encoded}&sort_order=asc"),
                    &token,
                    None,
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::OK,
            "{hostile} 应被忽略而不是报错：{body}"
        );
        assert_eq!(
            names_of(&body),
            names_of(&plain),
            "{hostile} 不该改变次序（白名单失效）：{body}"
        );
    }

    // 用户列表也走一遍：确认接线不只在一张表上生效（不同 handler、不同的默认排序）。
    let users_desc = json_body(
        app.oneshot(req(
            "GET",
            "/api/v1/system/users?sort_by=user_name&sort_order=desc&page_size=50",
            &token,
            None,
        ))
        .await
        .unwrap(),
    )
    .await;
    let user_names: Vec<String> = users_desc["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|u| u["user_name"].as_str().unwrap_or_default().to_string())
        .collect();
    let mut expect = user_names.clone();
    expect.sort_by(|a, b| b.cmp(a));
    assert_eq!(
        user_names, expect,
        "用户列表按 user_name 降序没生效：{users_desc}"
    );
}

/// 「创建 `201` + `Location`」的运行时证据：头存在、且指的那个 URI **真的能 GET 到**。
///
/// 只断言「有 Location 头」是不够的——一个拼错前缀的 URI 同样有头，但客户端照着走会 404。
/// 所以每条都拿返回的 Location 直接再发一次 GET，要求 200。
#[tokio::test]
#[ignore = "需要真实 PostgreSQL"]
async fn create_endpoints_return_a_resolvable_location() {
    let (state, token) = setup("web_created_location").await;
    let app = router(state.clone());

    // (建号 URI, 建号体, Location 前缀)
    let cases: Vec<(&str, serde_json::Value, &str)> = vec![
        (
            "/api/v1/system/users",
            serde_json::json!({"user_name":"loc_user","nick_name":"定位用户","password":"Cicada@123"}),
            "/api/v1/system/users/",
        ),
        (
            "/api/v1/system/roles",
            serde_json::json!({"role_name":"定位角色","role_key":"loc_role"}),
            "/api/v1/system/roles/",
        ),
        (
            "/api/v1/system/posts",
            serde_json::json!({"post_code":"loc_post","post_name":"定位岗位","post_sort":1,"status":0}),
            "/api/v1/system/posts/",
        ),
        (
            "/api/v1/system/configs",
            serde_json::json!({"config_name":"定位参数","config_key":"sys.loc","config_value":"1","config_type":"N"}),
            "/api/v1/system/configs/",
        ),
        (
            "/api/v1/system/notices",
            serde_json::json!({"notice_title":"定位公告","notice_type":1,"notice_content":"正文","status":0}),
            "/api/v1/system/notices/",
        ),
        (
            "/api/v1/system/dict/types",
            serde_json::json!({"dict_name":"定位字典","dict_type":"sys_loc","status":0}),
            "/api/v1/system/dict/types/",
        ),
        (
            "/api/v1/monitor/jobs",
            serde_json::json!({
                "job_name": "定位任务", "job_group": "DEFAULT",
                "invoke_target": "cicada:noop", "cron_expression": "0 0 3 * * ?",
            }),
            "/api/v1/monitor/jobs/",
        ),
    ];

    for (create_uri, body, prefix) in cases {
        let resp = app
            .clone()
            .oneshot(req("POST", create_uri, &token, Some(body)))
            .await
            .unwrap();
        let status = resp.status();
        let location = resp
            .headers()
            .get(axum::http::header::LOCATION)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_string();
        let created_body = json_body(resp).await;
        assert_eq!(
            status,
            StatusCode::CREATED,
            "{create_uri} 应 201：{created_body}"
        );
        assert!(
            location.starts_with(prefix),
            "{create_uri} 的 Location 是 {location:?}，应以 {prefix:?} 开头"
        );

        // Location 指向的就是刚建出来的那一行：GET 它必须 200，且 id 与响应体一致。
        let got = app
            .clone()
            .oneshot(req("GET", &location, &token, None))
            .await
            .unwrap();
        let got_status = got.status();
        let got_body = json_body(got).await;
        assert_eq!(
            got_status,
            StatusCode::OK,
            "GET {location} 失败：{got_body}"
        );
    }
}

/// 同一个契约的静态闸门：**任何**新增的 201 都必须走 `response::created`。
///
/// 运行时测试只覆盖写到的那几个接口；漏掉一个接口不会失败，而「忘了带 Location」这件事
/// 恰恰只在没人用的时候才会留下。扫源码断言「除 `response.rs` 之外没有裸的
/// `StatusCode::CREATED`」，让下一个新增接口要么走 helper、要么在这里失败。
#[test]
fn no_create_endpoint_returns_a_bare_201() {
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut offenders = Vec::new();
    collect_bare_created(&src, &mut offenders);
    assert!(
        offenders.is_empty(),
        "这些位置返回了没有 Location 的 201（应改用 crate::response::created）：{offenders:?}"
    );
}

fn collect_bare_created(dir: &std::path::Path, out: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_bare_created(&path, out);
            continue;
        }
        if path.extension().and_then(|e| e.to_str()) != Some("rs") {
            continue;
        }
        // response.rs 是这个 helper 自己的定义处，天然允许。
        if path.file_name().is_some_and(|n| n == "response.rs") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        // `#[cfg(test)]` 之后的都是测试自己的夹具与断言（router.rs 里就有两处），
        // 不是对外契约的一部分；这条闸门只管生产代码。
        let mut in_tests = false;
        for (idx, line) in text.lines().enumerate() {
            if line.trim_start().starts_with("#[cfg(test)]") {
                in_tests = true;
            }
            let t = line.trim();
            if t.starts_with("//") || in_tests {
                continue;
            }
            if line.contains("StatusCode::CREATED") && !line.contains("created(") {
                out.push(format!("{}:{}", path.display(), idx + 1));
            }
        }
    }
}

/// 数据层约定「`page_size` 上限 200；**超限报错而不是静默截断**」——在一个真实列表接口上验。
///
/// 只测单元测试不够：这条契约讲的是 HTTP 面的行为，25 个列表接口共用同一个解析函数，
/// 所以钉住一个代表 + 断言业务码，就足够让「改回 clamp」立刻红。
#[tokio::test]
#[ignore = "需要真实 PostgreSQL"]
async fn oversized_page_size_is_a_validation_error_not_a_silent_truncation() {
    let (state, token) = setup("web_page_limit").await;
    let app = router(state.clone());

    for path in [
        "/api/v1/system/users?page_size=5000",
        "/api/v1/system/roles?page_size=201",
        "/api/v1/system/posts?page_size=0",
        "/api/v1/system/users?page=0",
    ] {
        let (status, body) = status_and_json(
            app.clone()
                .oneshot(req("GET", path, &token, None))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{path} -> {body}");
        let text = serde_json::to_string(&body).unwrap();
        assert!(
            text.contains("CICADA_VALIDATION_4004"),
            "{path} 应当报分页上限：{text}"
        );
    }

    // 边界：正好 200 合法；默认值也合法。
    for path in ["/api/v1/system/users?page_size=200", "/api/v1/system/users"] {
        let (status, body) = status_and_json(
            app.clone()
                .oneshot(req("GET", path, &token, None))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{path} -> {body}");
        let items = body["items"].as_array().unwrap();
        let size = body["page_size"].as_u64().unwrap();
        assert!(items.len() <= size as usize, "返回条数不该越过请求：{path}");
        // 不传时按数据层约定的默认 20；传 200 时原样回显 200（没有被改写）。
        assert_eq!(
            size,
            if path.contains("page_size") { 200 } else { 20 },
            "{path} -> {body}"
        );
    }
}
