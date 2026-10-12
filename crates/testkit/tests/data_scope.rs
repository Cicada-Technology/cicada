//! 数据权限端到端组件测试（真实 PostgreSQL）。
//!
//! 链路：`system` 实体 + `LocalDeptDirectory` + `rbac::compute_data_scope` +
//! `kernel::Query` 的 typestate 作用域过滤，一次跑通并断言结果集。

use sea_orm::{
    ConnectOptions, ConnectionTrait, Database, DatabaseBackend, DatabaseConnection, DbBackend,
    Statement, Value,
};
use sea_orm_migration::MigratorTraitSelf;

use cicada_kernel::{DeptId, Query, TenantId, UserId};
use cicada_rbac::compute_data_scope;
use cicada_system::entity::{UserColumn, UserEntity};
use cicada_system::{LocalDeptDirectory, load_grant_snapshot};

const SCHEMA: &str = "sys_scope_test";

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

async fn exec(db: &DatabaseConnection, sql: &str, values: Vec<Value>) {
    db.execute_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        sql,
        values,
    ))
    .await
    .unwrap();
}

async fn seed(db: &DatabaseConnection) {
    let now: Value = chrono::Utc::now().into();
    // (SQL, 是否带 create_time 占位符)
    let statements: &[(&str, bool)] = &[
        // 部门树：100(根) -> 110,120；110 -> 111
        (
            "INSERT INTO sys_dept (dept_id, tenant_id, parent_id, ancestors, dept_name, order_num, status, version, create_time) \
             VALUES (100, 1, 0, '0', '总公司', 1, 0, 0, $1)",
            true,
        ),
        (
            "INSERT INTO sys_dept (dept_id, tenant_id, parent_id, ancestors, dept_name, order_num, status, version, create_time) \
             VALUES (110, 1, 100, '0,100', '研发部', 1, 0, 0, $1)",
            true,
        ),
        (
            "INSERT INTO sys_dept (dept_id, tenant_id, parent_id, ancestors, dept_name, order_num, status, version, create_time) \
             VALUES (111, 1, 110, '0,100,110', '前端组', 1, 0, 0, $1)",
            true,
        ),
        (
            "INSERT INTO sys_dept (dept_id, tenant_id, parent_id, ancestors, dept_name, order_num, status, version, create_time) \
             VALUES (120, 1, 100, '0,100', '销售部', 1, 0, 0, $1)",
            true,
        ),
        // 用户
        (
            "INSERT INTO sys_user (user_id, tenant_id, user_name, nick_name, dept_id, password, status, version, create_time) \
             VALUES (1, 1, 'alice', 'Alice', 110, 'x', 0, 0, $1)",
            true,
        ),
        (
            "INSERT INTO sys_user (user_id, tenant_id, user_name, nick_name, dept_id, password, status, version, create_time) \
             VALUES (2, 1, 'bob', 'Bob', 111, 'x', 0, 0, $1)",
            true,
        ),
        (
            "INSERT INTO sys_user (user_id, tenant_id, user_name, nick_name, dept_id, password, status, version, create_time) \
             VALUES (3, 1, 'carol', 'Carol', 120, 'x', 0, 0, $1)",
            true,
        ),
        (
            "INSERT INTO sys_user (user_id, tenant_id, user_name, nick_name, dept_id, password, status, version, create_time) \
             VALUES (4, 1, 'dave', 'Dave', NULL, 'x', 0, 0, $1)",
            true,
        ),
        // 角色：10 = DeptAndChild(4)，11 = Custom(2) 自定部门 120
        (
            "INSERT INTO sys_role (role_id, tenant_id, role_name, role_key, role_sort, data_scope, status, version, create_time) \
             VALUES (10, 1, '部门负责人', 'dept_lead', 1, 4, 0, 0, $1)",
            true,
        ),
        (
            "INSERT INTO sys_role (role_id, tenant_id, role_name, role_key, role_sort, data_scope, status, version, create_time) \
             VALUES (11, 1, '销售', 'sales', 2, 2, 0, 0, $1)",
            true,
        ),
        (
            "INSERT INTO sys_user_role (user_id, role_id) VALUES (1, 10)",
            false,
        ),
        (
            "INSERT INTO sys_user_role (user_id, role_id) VALUES (3, 11)",
            false,
        ),
        (
            "INSERT INTO sys_role_dept (role_id, dept_id) VALUES (11, 120)",
            false,
        ),
    ];
    for (sql, with_time) in statements {
        let values = if *with_time {
            vec![now.clone()]
        } else {
            Vec::new()
        };
        exec(db, sql, values).await;
    }
}

async fn local_dept_dir() -> LocalDeptDirectory {
    let cfg = cicada_infra::config::DbConfig {
        url: cicada_infra::config::Secret::new(url()),
        max_connections: 5,
        min_connections: 0,
        acquire_timeout_secs: 5,
        slow_query_ms: 200,
        schema: SCHEMA.to_string(),
    };
    LocalDeptDirectory::new(cicada_infra::Db::connect(&cfg).await.unwrap())
}

#[tokio::test]
#[ignore = "需要真实 PostgreSQL"]
async fn dept_and_child_scope_filters_users() {
    let admin = connect(None).await;
    admin
        .execute_raw(Statement::from_string(
            DatabaseBackend::Postgres,
            format!("DROP SCHEMA IF EXISTS {SCHEMA} CASCADE"),
        ))
        .await
        .unwrap();
    admin
        .execute_raw(Statement::from_string(
            DatabaseBackend::Postgres,
            format!("CREATE SCHEMA {SCHEMA}"),
        ))
        .await
        .unwrap();

    let db = connect(Some(SCHEMA)).await;
    cicada_migrations::Migrator.up(&db, None).await.unwrap();
    seed(&db).await;

    // alice 的角色 10 是 DeptAndChild，本部门 110 → 展开成 [110, 111]。
    let snapshot = load_grant_snapshot(&db, TenantId::new(1), UserId::new(1)).await;
    let dept_dir = local_dept_dir().await;
    let data_scope = compute_data_scope(
        TenantId::new(1),
        UserId::new(1),
        snapshot.user_dept,
        &snapshot.roles,
        &dept_dir,
    )
    .await;
    assert_eq!(
        data_scope.dept_ids,
        vec![DeptId::new(110), DeptId::new(111)]
    );

    let page = Query::<UserEntity>::open()
        .scoped(&cicada_kernel::Scope {
            tenant_id: TenantId::new(1),
            user_id: Some(UserId::new(1)),
            data_scope,
        })
        .order_by(UserColumn::UserId)
        .fetch_page(&db, 1, 100)
        .await
        .unwrap();

    let names: Vec<String> = page.items.into_iter().map(|u| u.user_name).collect();
    assert_eq!(names, vec!["alice".to_string(), "bob".to_string()]);

    // carol 的角色 11 是 Custom(120)：只看到 120 部门的 carol。
    let snapshot = load_grant_snapshot(&db, TenantId::new(1), UserId::new(3)).await;
    let data_scope = compute_data_scope(
        TenantId::new(1),
        UserId::new(3),
        snapshot.user_dept,
        &snapshot.roles,
        &dept_dir,
    )
    .await;
    let page = Query::<UserEntity>::open()
        .scoped(&cicada_kernel::Scope {
            tenant_id: TenantId::new(1),
            user_id: Some(UserId::new(3)),
            data_scope,
        })
        .fetch_page(&db, 1, 100)
        .await
        .unwrap();
    let names: Vec<String> = page.items.into_iter().map(|u| u.user_name).collect();
    assert_eq!(names, vec!["carol".to_string()]);

    admin
        .execute_raw(Statement::from_string(
            DatabaseBackend::Postgres,
            format!("DROP SCHEMA {SCHEMA} CASCADE"),
        ))
        .await
        .unwrap();
}
