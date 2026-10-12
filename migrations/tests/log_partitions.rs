//! 日志表改造成按月分区的往返验证。真实 PostgreSQL，默认 ignore。
//!
//! 关键风险点是「存量数据比预铺窗口更早」：迁移若只按 `back_months` 铺分区，
//! 搬运更早的行会直接失败。这里刻意插入 20 个月前的行，再跑完整 up 来守住这条。

use sea_orm_migration::MigratorTraitSelf;
use sea_orm_migration::sea_orm::{
    ConnectOptions, ConnectionTrait, Database, DatabaseBackend, DatabaseConnection, Statement,
};

use cicada_migrations::Migrator;

// 两个用例各自一个 schema：同一条链路上会互相 DROP，共用就会打架。
const PARTITIONS_SCHEMA: &str = "mig_partitions_test";
const FN_SCHEMA: &str = "mig_partitions_fn_test";
/// 框架迁移条数（`framework_migrations()` 的长度）；最后一条是分区改造。
/// 000003 已并入 000001、000009 不再占用列表，所以条数是 10。
const MIGRATIONS_BEFORE_PARTITIONS: u32 = 10;

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

async fn scalar_text(conn: &DatabaseConnection, sql: &str) -> String {
    conn.query_all_raw(Statement::from_string(DatabaseBackend::Postgres, sql))
        .await
        .unwrap()
        .first()
        .and_then(|r| r.try_get_by_index::<String>(0).ok())
        .unwrap_or_default()
}

async fn scalar_i64(conn: &DatabaseConnection, sql: &str) -> i64 {
    conn.query_all_raw(Statement::from_string(DatabaseBackend::Postgres, sql))
        .await
        .unwrap()
        .first()
        .and_then(|r| r.try_get_by_index::<i64>(0).ok())
        .unwrap_or(0)
}

#[tokio::test]
#[ignore = "需要真实 PostgreSQL"]
async fn existing_rows_survive_the_partition_conversion() {
    let admin = connect(None).await;
    admin
        .execute_unprepared(&format!(
            "DROP SCHEMA IF EXISTS {PARTITIONS_SCHEMA} CASCADE"
        ))
        .await
        .unwrap();
    admin
        .execute_unprepared(&format!("CREATE SCHEMA {PARTITIONS_SCHEMA}"))
        .await
        .unwrap();
    let db = connect(Some(PARTITIONS_SCHEMA)).await;

    // 先跑到分区改造之前，塞入「20 个月前」与「今天」两类存量行。
    Migrator
        .up(&db, Some(MIGRATIONS_BEFORE_PARTITIONS))
        .await
        .unwrap();
    db.execute_unprepared(
        "INSERT INTO sys_oper_log
             (tenant_id, title, business_type, method, request_method, oper_url, status,
              oper_time, cost_ms)
         VALUES
             (1, 'old', 1, 'POST', 'POST', '/x', 0, now() - interval '20 months', 1),
             (1, 'new', 1, 'POST', 'POST', '/x', 0, now(), 2)",
    )
    .await
    .unwrap();
    db.execute_unprepared(
        "INSERT INTO sys_logininfor (tenant_id, user_name, status, login_time)
         VALUES (1, 'admin', 0, now() - interval '20 months'), (1, 'admin', 0, now())",
    )
    .await
    .unwrap();
    let max_oper = scalar_i64(&db, "SELECT max(oper_id) FROM sys_oper_log").await;

    // 分区改造迁移。
    Migrator.up(&db, None).await.unwrap();

    for table in [
        "sys_oper_log",
        "sys_logininfor",
        "sys_job_log",
        "sys_audit_log",
    ] {
        assert_eq!(
            scalar_text(
                &db,
                &format!(
                    "SELECT relkind::text FROM pg_class WHERE oid = '{PARTITIONS_SCHEMA}.{table}'::regclass"
                )
            )
            .await,
            "p",
            "{table} 应已是分区表"
        );
        assert!(
            scalar_i64(
                &db,
                &format!(
                    "SELECT count(*) FROM pg_inherits i
                         JOIN pg_class ch ON ch.oid = i.inhrelid
                         WHERE i.inhparent = '{PARTITIONS_SCHEMA}.{table}'::regclass
                           AND ch.relnamespace = '{PARTITIONS_SCHEMA}'::regnamespace"
                )
            )
            .await
                > 12,
            "{table} 的月度分区应覆盖存量月份 + 前后窗口"
        );
    }

    // 存量一行不丢，且旧月份各自落到自己的分区里。
    assert_eq!(
        scalar_i64(&db, "SELECT count(*) FROM sys_oper_log").await,
        2
    );
    assert_eq!(
        scalar_i64(&db, "SELECT count(*) FROM sys_logininfor").await,
        2
    );
    assert_eq!(
        scalar_i64(
            &db,
            "SELECT count(*) FROM sys_oper_log
                 WHERE oper_time < now() - interval '19 months'"
        )
        .await,
        1
    );

    // 新写入照常自增，且不撞已搬运过来的主键。
    db.execute_unprepared(
        "INSERT INTO sys_oper_log
             (tenant_id, title, business_type, method, request_method, oper_url, status,
              oper_time, cost_ms)
         VALUES (1, 'after', 1, 'POST', 'POST', '/x', 0, now(), 3)",
    )
    .await
    .unwrap();
    let new_max = scalar_i64(&db, "SELECT max(oper_id) FROM sys_oper_log").await;
    assert!(
        new_max > max_oper,
        "序列没接上：max 仍是 {new_max}（改造前 {max_oper}）"
    );

    // down：回滚整条链（含分区退化为普通表的分支），最后不该留下业务表。
    Migrator.down(&db, None).await.unwrap();
    assert!(
        scalar_i64(
            &db,
            "SELECT count(*) FROM information_schema.tables
                 WHERE table_schema = current_schema() AND table_name <> 'seaql_migrations'"
        )
        .await
            <= 1,
        "down 之后业务表应清空"
    );

    admin
        .execute_unprepared(&format!("DROP SCHEMA {PARTITIONS_SCHEMA} CASCADE"))
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "需要真实 PostgreSQL"]
async fn ensure_function_is_idempotent_and_extends_forward() {
    let admin = connect(None).await;
    admin
        .execute_unprepared(&format!("DROP SCHEMA IF EXISTS {FN_SCHEMA} CASCADE"))
        .await
        .unwrap();
    admin
        .execute_unprepared(&format!("CREATE SCHEMA {FN_SCHEMA}"))
        .await
        .unwrap();
    let db = connect(Some(FN_SCHEMA)).await;
    Migrator.up(&db, None).await.unwrap();

    let before = scalar_i64(
        &db,
        "SELECT count(*) FROM pg_inherits i
             JOIN pg_class ch ON ch.oid = i.inhrelid
             WHERE i.inhparent = 'sys_oper_log'::regclass
               AND ch.relnamespace = current_schema()::regnamespace",
    )
    .await;
    // 再调一次不应产生新分区（幂等），这是维护任务每月重复跑的前提。
    assert_eq!(
        scalar_i64(
            &db,
            "SELECT cicada_ensure_monthly_partitions('sys_oper_log', 'oper_time', 12, 24)"
        )
        .await,
        0
    );
    let after = scalar_i64(
        &db,
        "SELECT count(*) FROM pg_inherits i
             JOIN pg_class ch ON ch.oid = i.inhrelid
             WHERE i.inhparent = 'sys_oper_log'::regclass
               AND ch.relnamespace = current_schema()::regnamespace",
    )
    .await;
    assert_eq!(before, after);

    // 扩窗：把跨度拉到 60 个月后，59 个月后的分区必须存在（幂等 + 向前扩展两件事一起验）。
    scalar_i64(
        &db,
        "SELECT cicada_ensure_monthly_partitions('sys_oper_log', 'oper_time', 12, 60)",
    )
    .await;
    for offset in [11i32, 59] {
        assert_eq!(
            scalar_i64(
                &db,
                &format!(
                    "SELECT count(*) FROM pg_class
                         WHERE relnamespace = '{FN_SCHEMA}'::regnamespace
                           AND relname = 'sys_oper_log_'
                               || to_char(now() + interval '{offset} months', 'YYYYMM')"
                )
            )
            .await,
            1,
            "{offset} 个月后的分区应存在"
        );
    }
    // 再调一次同一个跨度：不该再有新分区，这是维护任务可以每月重复跑的前提。
    assert_eq!(
        scalar_i64(
            &db,
            "SELECT cicada_ensure_monthly_partitions('sys_oper_log', 'oper_time', 12, 60)",
        )
        .await,
        0,
        "同跨度重复调用必须是 0"
    );

    admin
        .execute_unprepared(&format!("DROP SCHEMA {FN_SCHEMA} CASCADE"))
        .await
        .unwrap();
}
