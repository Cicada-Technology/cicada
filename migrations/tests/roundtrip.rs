//! 迁移往返测试：空库 up → down → 空库。默认 ignore，需真实 PostgreSQL。

use sea_orm_migration::MigratorTraitSelf;
use sea_orm_migration::sea_orm::{
    ConnectOptions, ConnectionTrait, Database, DatabaseBackend, DatabaseConnection, Statement,
};

use cicada_migrations::Migrator;

const SCHEMA: &str = "mig_rt_test";

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

#[tokio::test]
#[ignore = "需要真实 PostgreSQL"]
async fn migrations_roundtrip_up_down() {
    let admin = connect(None).await;
    admin
        .execute_raw(Statement::from_string(
            DatabaseBackend::Postgres,
            "CREATE SCHEMA IF NOT EXISTS mig_rt_test",
        ))
        .await
        .unwrap();

    let db = connect(Some(SCHEMA)).await;

    // 空库 → up
    Migrator.up(&db, None).await.unwrap();
    let after_up = table_count(&db).await;
    assert!(after_up > 0, "up 后应有业务表，实际 {after_up}");

    // up → down
    Migrator.down(&db, None).await.unwrap();
    let after_down = table_count(&db).await;
    assert!(after_down <= 1, "down 后业务表应清空，剩余 {after_down}");

    admin
        .execute_raw(Statement::from_string(
            DatabaseBackend::Postgres,
            "DROP SCHEMA mig_rt_test CASCADE",
        ))
        .await
        .unwrap();
}

async fn table_count(db: &DatabaseConnection) -> i64 {
    let stmt = Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "SELECT count(*) FROM information_schema.tables WHERE table_schema = $1 AND table_name <> 'seaql_migrations'",
        [SCHEMA.into()],
    );
    let rows = db.query_all_raw(stmt).await.unwrap();
    rows[0].try_get_by_index::<i64>(0).unwrap()
}
