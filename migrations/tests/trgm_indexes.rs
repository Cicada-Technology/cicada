//! 模糊检索列的 `pg_trgm` GIN 索引。真实 PostgreSQL，默认 ignore。
//!
//! 两件事分别可验：索引**在不在**（清单与库内对象一致），以及它**是不是真能用**
//! （规划器对 `ILIKE '%kw%'` 愿意走它）。「列清单来自 `crates/web` 的 fuzzy 过滤」这件事
//! 由 `FUZZY_COLUMNS` 与 HTTP 层的搜索测试共同兜住——见 `crates/web/tests/online_users.rs`。

use sea_orm_migration::MigratorTraitSelf;
use sea_orm_migration::sea_orm::{
    ConnectOptions, ConnectionTrait, Database, DatabaseBackend, DatabaseConnection, Statement,
};

use cicada_migrations::{FUZZY_COLUMNS, Migrator, trgm_index_name};
use sea_orm_migration::sea_orm::TransactionTrait;

/// 三个用例并行跑，各自一个 schema（同名会撞 `42P06 schema already exists`）。
const SCHEMA_EXIST: &str = "mig_trgm_exist_test";
const SCHEMA_PLAN: &str = "mig_trgm_plan_test";
const SCHEMA_LIKE: &str = "mig_trgm_like_test";

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

async fn scalar(conn: &DatabaseConnection, sql: &str) -> String {
    conn.query_one_raw(Statement::from_string(DatabaseBackend::Postgres, sql))
        .await
        .unwrap()
        .map(|r| r.try_get_by_index::<String>(0).unwrap_or_default())
        .unwrap_or_default()
}

#[tokio::test]
#[ignore = "需要真实 PostgreSQL"]
async fn every_fuzzy_column_has_a_trgm_gin_index() {
    let admin = connect(None).await;
    admin
        .execute_unprepared(&format!("DROP SCHEMA IF EXISTS {SCHEMA_EXIST} CASCADE"))
        .await
        .unwrap();
    admin
        .execute_unprepared(&format!("CREATE SCHEMA {SCHEMA_EXIST}"))
        .await
        .unwrap();
    let conn = connect(Some(SCHEMA_EXIST)).await;
    Migrator.up(&conn, None).await.unwrap();

    assert_eq!(
        scalar(
            &conn,
            "SELECT extname FROM pg_extension WHERE extname = 'pg_trgm'"
        )
        .await,
        "pg_trgm",
        "扩展没装上，索引就不可能可用"
    );

    for (table, column) in FUZZY_COLUMNS {
        let def = scalar(
            &conn,
            &format!(
                "SELECT COALESCE(indexdef, '<缺>') FROM pg_indexes \
                 WHERE schemaname = '{SCHEMA_EXIST}' AND tablename = '{table}' \
                   AND indexname = '{}'",
                trgm_index_name(table, column)
            ),
        )
        .await;
        assert!(
            def.contains("USING gin") && def.contains("gin_trgm_ops"),
            "{table}.{column} 应是 trgm GIN 索引，实际：{def}"
        );
    }
}

/// 索引不是「建了没人走」：`ILIKE '%kw%'` 在规划器允许顺序扫描之外时必须落到 GIN 上。
#[tokio::test]
#[ignore = "需要真实 PostgreSQL"]
async fn ilike_planner_uses_the_trgm_index() {
    let admin = connect(None).await;
    admin
        .execute_unprepared(&format!("DROP SCHEMA IF EXISTS {SCHEMA_PLAN} CASCADE"))
        .await
        .unwrap();
    admin
        .execute_unprepared(&format!("CREATE SCHEMA {SCHEMA_PLAN}"))
        .await
        .unwrap();
    let conn = connect(Some(SCHEMA_PLAN)).await;
    Migrator.up(&conn, None).await.unwrap();

    // 规模要够：几百行的表上 GIN 的固定开销打不过顺序扫描，规划器永远不选它，
    // 那样的「没走索引」跟索引可用性无关。60k 行是本用例的最低有效规模。
    conn.execute_unprepared(
        "INSERT INTO sys_user (tenant_id, user_name, nick_name, password, status, create_time) \
         SELECT 1, 'user_' || g || '_acct', 'U', 'x', 0, now() FROM generate_series(1, 60000) g",
    )
    .await
    .unwrap();
    conn.execute_unprepared("ANALYZE sys_user").await.unwrap();

    // 事务里关掉顺序扫描：这里要证明的是「这条 ILIKE 谓词能被 trgm GIN 服务」
    // （谓词进 Index Cond），而不是成本模型在两种扫描之间的偏好。
    let txn = conn.begin().await.unwrap();
    txn.execute_unprepared("SET LOCAL enable_seqscan = off")
        .await
        .unwrap();
    let rows = txn
        .query_all_raw(Statement::from_string(
            DatabaseBackend::Postgres,
            "EXPLAIN SELECT count(*) FROM sys_user WHERE user_name ILIKE '%1234_acct%'",
        ))
        .await
        .unwrap();
    let text = rows
        .iter()
        .map(|r| r.try_get_by_index::<String>(0).unwrap_or_default())
        .collect::<Vec<_>>()
        .join("\n");
    txn.execute_unprepared("ROLLBACK").await.ok();
    assert!(
        text.contains("sys_user_user_name_trgm"),
        "ILIKE 应能走 trgm GIN，实际计划：\n{text}"
    );
}

/// `ILIKE` 的大小写与通配符语义（分页与查询；应用侧的 `fuzzy()` 依赖这两条）。
#[tokio::test]
#[ignore = "需要真实 PostgreSQL"]
async fn ilike_is_case_insensitive_and_backslash_escapes_wildcards() {
    let admin = connect(None).await;
    admin
        .execute_unprepared(&format!("DROP SCHEMA IF EXISTS {SCHEMA_LIKE} CASCADE"))
        .await
        .unwrap();
    admin
        .execute_unprepared(&format!("CREATE SCHEMA {SCHEMA_LIKE}"))
        .await
        .unwrap();
    let conn = connect(Some(SCHEMA_LIKE)).await;
    Migrator.up(&conn, None).await.unwrap();

    conn.execute_unprepared(
        "INSERT INTO sys_user (tenant_id, user_name, nick_name, password, status, create_time) \
         VALUES (1, 'Admin', 'A', 'x', 0, now()), \
                (1, 'pct_50%', 'P', 'x', 0, now()), \
                (1, 'plain', 'L', 'x', 0, now())",
    )
    .await
    .unwrap();

    // 大小写不敏感：`%admin%` 命中 `Admin`（分页与查询要的就是 ILIKE）。
    assert_eq!(
        scalar(
            &conn,
            "SELECT count(*)::text FROM sys_user WHERE user_name ILIKE '%admin%'"
        )
        .await,
        "1"
    );
    // `fuzzy()` 把输入里的 `%` `_` 转义成字面量：`%pct\_50\%%` 只匹配 'pct_50%' 这一行。
    assert_eq!(
        scalar(
            &conn,
            r#"SELECT count(*)::text FROM sys_user WHERE user_name ILIKE '%pct\_50\%%'"#
        )
        .await,
        "1"
    );
    // 对照：不转义时 `%` 就是通配符，无关行也会被拖进来——正是 `fuzzy()` 挡掉的那类输入。
    assert_eq!(
        scalar(
            &conn,
            "SELECT count(*)::text FROM sys_user WHERE user_name ILIKE '%%%'"
        )
        .await,
        "3"
    );
}
