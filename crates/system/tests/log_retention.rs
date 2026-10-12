//! 日志分区归档与删除（保留期可配，到期归档到对象存储而不是直接删除）。
//! 真实 PostgreSQL，默认 ignore。

use sea_orm::{
    ConnectOptions, ConnectionTrait, Database, DatabaseBackend, DatabaseConnection, Statement,
};
use sea_orm_migration::MigratorTraitSelf;

use cicada_infra::Db;
use cicada_infra::config::{DbConfig, Secret};
use cicada_ports::InMemoryArchive;
use cicada_system::LogRetention;

const SCHEMA: &str = "sys_log_retention_test";

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

async fn count(conn: &DatabaseConnection, sql: &str) -> i64 {
    conn.query_all_raw(Statement::from_string(DatabaseBackend::Postgres, sql))
        .await
        .unwrap()
        .first()
        .and_then(|r| r.try_get_by_index::<i64>(0).ok())
        .unwrap_or(0)
}

#[tokio::test]
#[ignore = "需要真实 PostgreSQL"]
async fn expired_partitions_are_archived_before_being_dropped() {
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
    let retention = LogRetention::new(db.clone());

    // 「8 个月前」整月已过 180 天保留期；当月这条不该被碰。
    db.conn()
        .execute_unprepared(
            "INSERT INTO sys_oper_log
                 (tenant_id, title, business_type, method, request_method, oper_url, status,
                  oper_time, cost_ms)
             VALUES
                 (1, 'old-1', 1, 'POST', 'POST', '/x', 0, now() - interval '8 months', 1),
                 (1, 'old-2', 1, 'POST', 'POST', '/x', 0, now() - interval '8 months', 2),
                 (1, 'fresh', 1, 'POST', 'POST', '/x', 0, now(), 3)",
        )
        .await
        .unwrap();

    let expired = retention.expired_partitions(180).await.unwrap();
    assert_eq!(expired.len(), 1, "只应命中 8 个月前那一个月：{expired:?}");
    assert_eq!(expired[0].table, "sys_oper_log");
    assert_eq!(expired[0].rows, 2);

    // 归档关掉时：只报告，一行都不能少。
    let dry = InMemoryArchive::new();
    let report = retention.run(180, 24, false, &dry).await.unwrap();
    assert_eq!(report.skipped, 1);
    assert!(report.archived.is_empty() && report.dropped.is_empty());
    assert_eq!(
        count(&conn, "SELECT count(*) FROM sys_oper_log").await,
        3,
        "archive=false 不该删数据"
    );

    // 打开归档：先落对象存储，再删分区。
    let archive = InMemoryArchive::new();
    let run = retention.run(180, 24, true, &archive).await.unwrap();
    assert_eq!(run.archived, run.dropped);
    assert_eq!(run.archived.len(), 1, "{run:?}");
    assert!(run.bytes > 0);
    let key = format!("logs/sys_oper_log/{}.ndjson", run.archived[0]);
    assert_eq!(
        archive.keys(),
        vec![key.clone()],
        "归档键应按 logs/<表>/<分区>"
    );
    let body = String::from_utf8(archive.get(&key).unwrap()).unwrap();
    assert_eq!(body.lines().count(), 2, "两行存量各一条 JSON：{body}");
    assert!(body.contains("old-1") && body.contains("old-2"), "{body}");
    assert!(!body.contains("fresh"), "未过期的行不该被卷进归档");

    assert_eq!(
        count(&conn, "SELECT count(*) FROM sys_oper_log").await,
        1,
        "过期分区整块摘掉，只留当月那行"
    );
    assert_eq!(
        count(
            &conn,
            "SELECT count(*) FROM sys_oper_log WHERE title = 'fresh'"
        )
        .await,
        1
    );

    // 再跑一轮：已经没有到期分区，且窗口延伸是幂等的。
    let again = retention.run(180, 24, true, &archive).await.unwrap();
    assert!(again.archived.is_empty(), "{again:?}");

    admin
        .execute_unprepared(&format!("DROP SCHEMA {SCHEMA} CASCADE"))
        .await
        .unwrap();
}
