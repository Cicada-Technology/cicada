//! RLS 兜底验证（ 出口条件「两租户数据完全隔离（含 RLS 验证）」）。
//!
//! 真实 PostgreSQL：开 RLS 后，「没有租户上下文」与「跨租户」两类访问都应看到 0 行，
//! 写入被 `WITH CHECK` 拒绝；关掉后恢复。默认 ignore。

use cicada_infra::config::{DbConfig, Secret};
use cicada_infra::db::Db;
use cicada_infra::rls;
use sea_orm::{
    ConnectOptions, ConnectionTrait, Database, DatabaseBackend, DatabaseConnection, Statement,
};
use sea_orm_migration::MigratorTraitSelf;

const SCHEMA: &str = "rls_test";

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

async fn scalar_i64<C: ConnectionTrait>(conn: &C, sql: &str) -> i64 {
    let rows = conn
        .query_all_raw(Statement::from_string(DatabaseBackend::Postgres, sql))
        .await
        .unwrap();
    rows[0].try_get_by_index::<i64>(0).unwrap()
}

/// 只有租户上下文、没有具体用户的 `Scope`（系统任务形态）。
fn tenant_scope(tenant: i64) -> cicada_kernel::Scope {
    cicada_kernel::Scope {
        tenant_id: cicada_kernel::TenantId::new(tenant),
        user_id: None,
        data_scope: cicada_kernel::DataScope::none(),
    }
}

async fn setup() -> (DatabaseConnection, Db) {
    let admin = connect(None).await;
    admin
        .execute_unprepared(&format!("DROP SCHEMA IF EXISTS {SCHEMA} CASCADE"))
        .await
        .unwrap();
    admin
        .execute_unprepared(&format!("CREATE SCHEMA {SCHEMA}"))
        .await
        .unwrap();

    let db = connect(Some(SCHEMA)).await;
    cicada_migrations::Migrator.up(&db, None).await.unwrap();
    cicada_migrations::seed::seed(&db).await.unwrap();
    cicada_migrations::init_admin::init_admin(&db)
        .await
        .unwrap();

    // 第二个租户 + 一个属于它的用户。
    db.execute_unprepared(
        "INSERT INTO sys_tenant (tenant_id, tenant_name, status, create_time) \
         VALUES (2, '第二租户', 0, now())",
    )
    .await
    .unwrap();
    db.execute_unprepared(
        "INSERT INTO sys_user (tenant_id, user_name, nick_name, password, status, version, create_time) \
         VALUES (2, 'bob', 'Bob', 'x', 0, 0, now())",
    )
    .await
    .unwrap();
    assert_eq!(scalar_i64(&db, "SELECT count(*) FROM sys_user").await, 2);

    let pool = Db::connect(&DbConfig {
        url: Secret::new(url()),
        max_connections: 5,
        min_connections: 0,
        acquire_timeout_secs: 5,
        slow_query_ms: 200,
        schema: SCHEMA.to_string(),
    })
    .await
    .unwrap();

    (db, pool)
}

/// 在受控事务里跑一条计数查询：`WHERE` 由 RLS 决定，不加任何应用层条件。
async fn count_in_scope(pool: &Db, tenant: i64) -> i64 {
    let scoped = pool.begin_scoped(&tenant_scope(tenant)).await.unwrap();
    let n = scalar_i64(&scoped, "SELECT count(*) FROM sys_user").await;
    scoped.rollback().await.unwrap();
    n
}

#[tokio::test]
#[ignore = "需要真实 PostgreSQL"]
async fn rls_isolates_tenants_and_fails_closed() {
    let (raw, pool) = setup().await;

    rls::enable(&raw, &["sys_user".to_string()]).await.unwrap();
    assert_eq!(rls::enabled_tables(&raw).await.unwrap(), vec!["sys_user"]);

    // 1) fail-closed：没有租户上下文（`Db::conn()` 直连、未注入 GUC）看不到任何行。
    assert_eq!(scalar_i64(&raw, "SELECT count(*) FROM sys_user").await, 0);

    // 2) 租户 1 只看得到租户 1 的行，租户 2 只看得到租户 2 的行。
    assert_eq!(count_in_scope(&pool, 1).await, 1);
    assert_eq!(count_in_scope(&pool, 2).await, 1);

    // 3) 应用层即使漏了归属条件，跨租户过滤/改写仍被数据库拦下。
    let scoped = pool.begin_scoped(&tenant_scope(1)).await.unwrap();
    assert_eq!(
        scalar_i64(&scoped, "SELECT count(*) FROM sys_user").await,
        1
    );
    // 试图把租户 2 的用户改名：RLS 下根本匹配不到行。
    let res = scoped
        .execute_unprepared("UPDATE sys_user SET nick_name = 'hacked' WHERE tenant_id = 2")
        .await
        .unwrap();
    assert_eq!(res.rows_affected(), 0);
    scoped.rollback().await.unwrap();

    // 4) 关掉 RLS 后恢复「无上下文可见」。
    rls::disable(&raw, &["sys_user".to_string()]).await.unwrap();
    assert!(rls::enabled_tables(&raw).await.unwrap().is_empty());
    assert_eq!(scalar_i64(&raw, "SELECT count(*) FROM sys_user").await, 2);

    // 5) 白名单之外的标识符不许开启（防止把自己关在门外）。
    let err = rls::enable(&raw, &["sys_tenant".to_string()]).await;
    assert!(err.is_err());
}
