//! 代码生成集成测试：候选表 → 导入 → 详情 → 编辑 → 预览 → 打包下载 → 删除。
//!
//! 用真实 PostgreSQL：在测试 schema 里建一张业务表，走完整 HTTP 面。
//! 生成的 Rust 是否可编译由 `crates/codegen/fixture` 常态保证（随 `cargo check --workspace`）。

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use sea_orm::{ConnectOptions, ConnectionTrait, Database};
use sea_orm_migration::MigratorTraitSelf;
use tower::ServiceExt;

use cicada_audit::InMemoryAuditSink;
use cicada_auth::{InMemorySessionStore, Session, SessionStore, TokenService, hash_refresh};
use cicada_infra::Db;
use cicada_infra::config::{Config, DbConfig, Secret};
use cicada_rbac::{AuthzCache, InMemoryAuthzStore};
use cicada_system::LocalDeptDirectory;
use cicada_web::state::AppState;

const JWT_SECRET: &str = "test-secret";

fn url() -> String {
    std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://cicada:2025%40hanmole@localhost:5432/cicada".to_string())
}

fn schema_url(schema: &str) -> String {
    format!("{}?options=-csearch_path%3D{schema}", url())
}

async fn setup(schema: &str) -> (AppState, String, Arc<InMemoryAuditSink>) {
    let admin = Database::connect(ConnectOptions::new(url())).await.unwrap();
    admin
        .execute_unprepared(&format!("DROP SCHEMA IF EXISTS {schema} CASCADE"))
        .await
        .unwrap();
    admin
        .execute_unprepared(&format!("CREATE SCHEMA {schema}"))
        .await
        .unwrap();

    let conn = Database::connect(ConnectOptions::new(schema_url(schema)))
        .await
        .unwrap();
    cicada_migrations::Migrator.up(&conn, None).await.unwrap();
    cicada_migrations::seed::seed(&conn).await.unwrap();
    cicada_migrations::init_admin::init_admin(&conn)
        .await
        .unwrap();
    conn.execute_unprepared(
        "CREATE TABLE biz_demo (
             demo_id bigserial PRIMARY KEY,
             tenant_id bigint NOT NULL,
             demo_name varchar(64) NOT NULL,
             amount numeric(12,2),
             remark text,
             deleted_at timestamptz,
             create_by varchar(64),
             create_time timestamptz NOT NULL DEFAULT now(),
             update_by varchar(64),
             update_time timestamptz
         )",
    )
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
    let mut config = Config::default();
    // 项目通常配置一组表名前缀作为命名剥离规则。
    config.codegen.strip_prefixes = vec!["biz_".to_string()];
    let state = AppState {
        dept_directory: Arc::new(LocalDeptDirectory::new(db.clone())),
        config,
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

fn get(uri: &str, token: &str) -> Request<Body> {
    Request::builder()
        .method("GET")
        .uri(uri)
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap()
}

fn json_req(method: &str, uri: &str, token: &str, body: serde_json::Value) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header("authorization", format!("Bearer {token}"))
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

async fn json_body(resp: axum::response::Response) -> serde_json::Value {
    let body = axum::body::to_bytes(resp.into_body(), 4 * 1024 * 1024)
        .await
        .unwrap();
    serde_json::from_slice(&body).unwrap_or(serde_json::Value::Null)
}

async fn import_demo(app: &axum::Router, token: &str) -> serde_json::Value {
    let resp = app
        .clone()
        .oneshot(json_req(
            "POST",
            "/api/v1/tool/gen/import",
            token,
            serde_json::json!({ "tables": ["biz_demo"] }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    json_body(resp).await
}

#[tokio::test]
async fn full_codegen_flow() {
    let (state, token, audit) = setup("web_codegen_flow").await;
    let app = cicada_web::router(state);

    // 1) 候选表：能看到 biz_demo，且可生成。
    let resp = app
        .clone()
        .oneshot(get("/api/v1/tool/gen/db/tables", &token))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = json_body(resp).await;
    let demo = body["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["table_name"] == "biz_demo")
        .expect("应能看到 biz_demo");
    assert!(demo["generatable"].as_bool().unwrap(), "{demo}");
    assert!(!demo["imported"].as_bool().unwrap());
    // 框架表出现在列表里但不可生成。
    let sys_user = body["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["table_name"] == "sys_user")
        .expect("应能看到 sys_user");
    assert!(!sys_user["generatable"].as_bool().unwrap());

    // 2) 导入。
    let result = import_demo(&app, &token).await;
    assert_eq!(result["imported"][0], "biz_demo", "{result}");
    assert!(result["failed"].as_array().unwrap().is_empty());

    // 3) 列表 + 详情。
    let resp = app
        .clone()
        .oneshot(get("/api/v1/tool/gen/tables", &token))
        .await
        .unwrap();
    let page = json_body(resp).await;
    assert_eq!(page["total"].as_u64().unwrap(), 1);
    let table = &page["items"][0];
    let table_id = table["table_id"].as_i64().unwrap();
    assert_eq!(table["business_name"], "demo");
    assert_eq!(table["class_name"], "Demo");
    assert!(table["columns"].as_array().unwrap().len() >= 10);
    let pk = table["columns"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["column_name"] == "demo_id")
        .unwrap();
    assert!(pk["is_pk"].as_bool().unwrap());
    assert!(pk["is_increment"].as_bool().unwrap());

    // 4) 在线编辑：改模块名 + 把 remark 设为查询项。
    let resp = app
        .clone()
        .oneshot(json_req(
            "PUT",
            &format!("/api/v1/tool/gen/tables/{table_id}"),
            &token,
            serde_json::json!({
                "module_name": "tool_demo",
                "function_author": "tester",
                "columns": [{ "column_name": "remark", "is_query": true, "query_type": "LIKE" }]
            }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let updated = json_body(resp).await;
    assert_eq!(updated["module_name"], "tool_demo");
    assert_eq!(updated["function_author"], "tester");

    // 非法命名被拒（路径穿越的入口）。
    let resp = app
        .clone()
        .oneshot(json_req(
            "PUT",
            &format!("/api/v1/tool/gen/tables/{table_id}"),
            &token,
            serde_json::json!({ "module_name": "../../etc" }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);

    // 5) 预览：路径随模块名变化，内容非空且无占位。
    let resp = app
        .clone()
        .oneshot(get(
            &format!("/api/v1/tool/gen/tables/{table_id}/preview"),
            &token,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let preview = json_body(resp).await;
    let files = preview["files"].as_array().unwrap();
    assert!(files.len() >= 8, "预览应包含全部模板：{files:?}");
    let handler = files
        .iter()
        .find(|f| f["name"] == "handler")
        .expect("handler");
    assert!(
        handler["path"]
            .as_str()
            .unwrap()
            .contains("src/tool_demo/demo/handler.rs")
    );
    assert!(
        handler["content"]
            .as_str()
            .unwrap()
            .contains("pub fn routes")
    );
    assert!(!handler["content"].as_str().unwrap().contains("todo!()"));

    // 6) 打包下载：zip 魔数 + 附件头。
    let resp = app
        .clone()
        .oneshot(get(
            &format!("/api/v1/tool/gen/tables/{table_id}/download"),
            &token,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        resp.headers().get("content-type").unwrap(),
        "application/zip"
    );
    assert!(
        resp.headers()
            .get("content-disposition")
            .unwrap()
            .to_str()
            .unwrap()
            .contains("demo.zip")
    );
    let bytes = axum::body::to_bytes(resp.into_body(), 4 * 1024 * 1024)
        .await
        .unwrap();
    assert_eq!(&bytes[..2], b"PK");

    // 7) 审计：导入与编辑各留一条。
    let opers = audit.opers();
    assert!(
        opers.iter().any(|o| o.title == "tool:gen:import"),
        "导入应写审计：{opers:?}"
    );
    assert!(opers.iter().any(|o| o.title == "tool:gen:edit"));

    // 8) 删除后 404。
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/api/v1/tool/gen/tables/{table_id}"))
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    let resp = app
        .clone()
        .oneshot(get(&format!("/api/v1/tool/gen/tables/{table_id}"), &token))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn rejects_framework_and_unknown_tables() {
    let (state, token, _audit) = setup("web_codegen_reject").await;
    let app = cicada_web::router(state);

    let resp = app
        .clone()
        .oneshot(json_req(
            "POST",
            "/api/v1/tool/gen/import",
            &token,
            serde_json::json!({ "tables": ["sys_user", "no_such_table"] }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = json_body(resp).await;
    assert!(body["imported"].as_array().unwrap().is_empty());
    let failed = body["failed"].as_array().unwrap();
    assert_eq!(failed.len(), 2, "{body}");
    assert!(failed.iter().any(|f| f["table_name"] == "sys_user"));

    // 空清单被拒。
    let resp = app
        .oneshot(json_req(
            "POST",
            "/api/v1/tool/gen/import",
            &token,
            serde_json::json!({ "tables": [] }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn tree_category_flow() {
    let schema = "web_codegen_tree";
    let (state, token, _audit) = setup(schema).await;
    let app = cicada_web::router(state);

    // 建一张树表：`parent_id` 非空（根=0），标签列 `cat_name`。
    let conn = Database::connect(ConnectOptions::new(schema_url(schema)))
        .await
        .unwrap();
    conn.execute_unprepared(
        "CREATE TABLE biz_category (
             cat_id bigserial PRIMARY KEY,
             tenant_id bigint NOT NULL,
             parent_id bigint NOT NULL DEFAULT 0,
             cat_name varchar(64) NOT NULL,
             sort_order int4 NOT NULL DEFAULT 0,
             deleted_at timestamptz,
             create_by varchar(64),
             create_time timestamptz NOT NULL DEFAULT now()
         )",
    )
    .await
    .unwrap();

    // 1) 以 tree 分类导入。
    let resp = app
        .clone()
        .oneshot(json_req(
            "POST",
            "/api/v1/tool/gen/import",
            &token,
            serde_json::json!({ "tables": ["biz_category"], "tpl_category": "tree" }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let result = json_body(resp).await;
    assert_eq!(result["imported"][0], "biz_category", "{result}");

    // 2) 元数据里父键被移出查询项（前端走树接口定位）。
    let resp = app
        .clone()
        .oneshot(get("/api/v1/tool/gen/tables", &token))
        .await
        .unwrap();
    let page = json_body(resp).await;
    let table = page["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["table_name"] == "biz_category")
        .expect("biz_category");
    assert_eq!(table["tpl_category"], "tree");
    let table_id = table["table_id"].as_i64().unwrap();
    let parent = table["columns"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["column_name"] == "parent_id")
        .expect("parent_id");
    assert!(!parent["is_query"].as_bool().unwrap());

    // 3) 预览：handler 出树接口与组装函数，vue 用 row-key + tree-props + tree-select。
    let resp = app
        .clone()
        .oneshot(get(
            &format!("/api/v1/tool/gen/tables/{table_id}/preview"),
            &token,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let preview = json_body(resp).await;
    let files = preview["files"].as_array().unwrap();
    let handler = files
        .iter()
        .find(|f| f["name"] == "handler")
        .expect("handler");
    let handler = handler["content"].as_str().unwrap();
    assert!(handler.contains("pub async fn tree_models"), "{handler}");
    assert!(handler.contains("fn assemble_tree"));
    assert!(handler.contains("\"/api/v1/business/category/tree\""));

    let vue = files.iter().find(|f| f["name"] == "vue").expect("vue");
    let vue = vue["content"].as_str().unwrap();
    assert!(vue.contains("row-key=\"cat_id\""));
    assert!(vue.contains("el-tree-select"));
    assert!(vue.contains("treeCategory"));

    // 4) 下载 zip 仍可用。
    let resp = app
        .clone()
        .oneshot(get(
            &format!("/api/v1/tool/gen/tables/{table_id}/download"),
            &token,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), 8 * 1024 * 1024)
        .await
        .unwrap();
    assert_eq!(&bytes[..2], b"PK");
}
