//! 审计 relay 的落库路径必须带租户上下文（投递 × RLS 兜底）。
//! 真实 PostgreSQL，默认 ignore。

use sea_orm::{
    ConnectOptions, ConnectionTrait, Database, DatabaseBackend, DatabaseConnection, Statement,
    TransactionTrait,
};
use sea_orm_migration::MigratorTraitSelf;
use serde_json::json;

use cicada_infra::Db;
use cicada_infra::config::{DbConfig, Secret};
use cicada_infra::rls;
use cicada_ports::{AGGREGATE_AUDIT, DomainEvent, EVENT_OPER_LOG, EventHandler};
use cicada_system::SysLogStore;

const SCHEMA: &str = "sys_log_store_rls_test";

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

fn oper_event(id: i64, tenant: i64) -> DomainEvent {
    DomainEvent {
        id,
        tenant_id: tenant,
        aggregate: AGGREGATE_AUDIT.to_string(),
        aggregate_id: None,
        event_type: EVENT_OPER_LOG.to_string(),
        payload: json!({
            "tenant_id": tenant,
            "title": "system:user:delete",
            "business_type": 3,
            "method": "System.Controllers.UserController.Delete",
            "request_method": "DELETE",
            "operator": "admin",
            "oper_url": "/api/v1/system/users/7",
            "oper_ip": "10.0.0.1",
            "oper_param": "[7]",
            "status": 0,
            "error_msg": null,
            "cost_ms": 12
        }),
        created_at: chrono::Utc::now()
            .format("%Y-%m-%d %H:%M:%S%.f+00")
            .to_string(),
        attempts: 0,
    }
}

/// 带 `app.tenant_id` 读父表：策略生效，只看得到本租户。
/// 必须同一事务——`SET LOCAL` 随事务消失，而连接池不会把会话变量带给下一次取连接。
async fn count_for(conn: &DatabaseConnection, tenant: i64) -> i64 {
    let txn = conn.begin().await.unwrap();
    txn.execute_unprepared(&format!(
        "SELECT set_config('app.tenant_id', '{tenant}', true)"
    ))
    .await
    .unwrap();
    let row = txn
        .query_one_raw(Statement::from_string(
            DatabaseBackend::Postgres,
            "SELECT count(*) FROM sys_oper_log",
        ))
        .await
        .unwrap();
    let count = row
        .and_then(|r| r.try_get_by_index::<i64>(0).ok())
        .unwrap_or(0);
    txn.commit().await.unwrap();
    count
}

#[tokio::test]
#[ignore = "需要真实 PostgreSQL"]
async fn relay_delivers_into_rls_protected_log_tables() {
    let admin = connect(None).await;
    admin
        .execute_unprepared(&format!("DROP SCHEMA IF EXISTS {SCHEMA} CASCADE"))
        .await
        .unwrap();
    admin
        .execute_unprepared(&format!("CREATE SCHEMA {SCHEMA}"))
        .await
        .unwrap();
    let conn = connect(Some(SCHEMA)).await;
    cicada_migrations::Migrator.up(&conn, None).await.unwrap();

    // 数据权限兜底开关：日志表开启 RLS（ENABLE + FORCE + fail-closed 策略）。
    rls::enable(&conn, &["sys_oper_log".to_string()])
        .await
        .unwrap();

    let db = Db::connect(&DbConfig {
        url: Secret::new(url()),
        max_connections: 3,
        min_connections: 0,
        acquire_timeout_secs: 5,
        slow_query_ms: 200,
        schema: SCHEMA.to_string(),
    })
    .await
    .unwrap();
    let store = SysLogStore::new(db.clone());

    store.handle(&oper_event(1, 1)).await.unwrap();
    store.handle(&oper_event(2, 2)).await.unwrap();

    // 两个租户各一条；策略没把跨租户投递挡掉，也没让它们互相看见。
    assert_eq!(count_for(&conn, 1).await, 1, "租户 1 应看到自己那一条");
    assert_eq!(count_for(&conn, 2).await, 1, "租户 2 应看到自己那一条");

    // 反证：同一条语句不带租户上下文就该被 WITH CHECK 拒掉——
    // 这正是 relay 必须走 begin_scoped 的原因，否则开了 RLS 的环境会静默丢审计。
    let unscoped = db
        .conn()
        .execute_unprepared(
            "INSERT INTO sys_oper_log (tenant_id, title, business_type, method, request_method, \
                                        oper_url, status, oper_time, cost_ms) \
             VALUES (1, 'no-context', 1, 'M', 'POST', '/x', 0, now(), 1)",
        )
        .await;
    assert!(
        unscoped.is_err(),
        "没有 app.tenant_id 时策略应拒绝写入（fail-closed）"
    );
}
